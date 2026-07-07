use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use flume::{Receiver, Sender, bounded, unbounded};
use tracing::{debug, warn};

use crate::command::Command;
use crate::display::{DisplayMessage, spawn_display, term_width};
use crate::executor::{allowed_cpus, partition, pin_thread};
use crate::format::{
    CounterRow, PerfCell, Relative, auto_unit, format_bytes, format_duration, format_time,
    render_counters,
};
use crate::measurement::{BenchmarkResult, Execution};
use crate::options::{Invocation, Options};
use crate::stats::Estimator;

/// Minimum delay between live counter updates sent to the display.
const COUNTER_REFRESH: Duration = Duration::from_millis(100);

/// A command's immutable identity, shared with every task that runs it.
struct CmdSpec {
    label: Box<str>,
    inv: Invocation,
}

struct Task {
    cmd_idx: usize,
    warmup: bool,
    /// A `/bin/true` calibration probe rather than a real command run.
    calibration: bool,
    /// Unique incrementing id, exposed as `MEGAFINE_RUN_ID` (unused when
    /// `calibration`).
    run_id: u64,
    spec: Arc<CmdSpec>,
}

struct RunReport {
    cmd_idx: usize,
    warmup: bool,
    outcome: Result<Execution>,
}

/// The measurement noise floor: mean wall-clock (s) and peak RSS (bytes) of an
/// empty command (`/bin/true`), below which a real measurement is dominated by
/// megafine's own spawn/measurement overhead.
struct Baseline {
    time: f64,
    rss: u64,
}

/// Dispatch `n` concurrent `/bin/true` runs (same path as real commands) and
/// collect their successful `(time, peak_rss)` results. `None` only on a broken
/// channel; an empty vec means `/bin/true` itself never succeeded.
fn calibration_round(
    n: usize,
    spec: &Arc<CmdSpec>,
    job_tx: &Sender<Task>,
    result_rx: &Receiver<RunReport>,
) -> Option<(Vec<f64>, Vec<u64>)> {
    for _ in 0..n {
        let task = Task {
            cmd_idx: 0,
            warmup: false,
            calibration: true,
            run_id: 0,
            spec: spec.clone(),
        };
        if job_tx.send(task).is_err() {
            return None;
        }
    }
    let mut times = Vec::with_capacity(n);
    let mut rss = Vec::with_capacity(n);
    for _ in 0..n {
        let report = result_rx.recv().ok()?;
        if let Ok(r) = report.outcome {
            times.push(r.wall_clock);
            rss.push(r.max_rss);
        }
    }
    Some((times, rss))
}

/// Average `/bin/true` over one warm round (a discarded cold round runs first, so
/// the floor reflects warm steady state). `None` if `/bin/true` can't run.
fn calibrate(
    jobs: usize,
    spec: &Arc<CmdSpec>,
    job_tx: &Sender<Task>,
    result_rx: &Receiver<RunReport>,
) -> Option<Baseline> {
    calibration_round(jobs, spec, job_tx, result_rx)?; // warmup, discarded
    let (times, rss) = calibration_round(jobs, spec, job_tx, result_rx)?;
    if times.is_empty() {
        return None;
    }
    Some(Baseline {
        time: crate::stats::mean(&times),
        rss: rss.iter().sum::<u64>() / rss.len() as u64,
    })
}

/// Whole-run progress for the rate-based ETA: tracked only when --runs is
/// given, so the total number of tasks (warmup + timed, every command) is
/// known upfront.
struct Progress {
    total: u64,
    done: u64,
    started: Instant,
}

/// Per-command scheduling state, owned and mutated only by the pump.
struct CmdState {
    spec: Arc<CmdSpec>,
    warmup_remaining: u64,
    warmup_in_flight: u64,
    /// Timed runs still to dispatch; `None` means infinite (until Ctrl-C or
    /// --target precision).
    timed_remaining: Option<u64>,
    in_flight: u64,
    measurements: Vec<Execution>,
    /// Running sum and sum of squares of the `measurements` wall-clock times,
    /// for an O(1) mean and stddev.
    sum: f64,
    sum_sq: f64,
    max_rss: u64,
    last_update: Instant,
    completed: bool,
}

/// Run a single task on a worker: optional (unmeasured) prepare, then the
/// measured command, then optional (unmeasured) conclude. A non-zero exit
/// from any of them surfaces as an `Err` (see `execute` for the
/// --ignore-failure exception on the measured command).
fn run_task(options: &Options, task: &Task) -> Result<Execution> {
    let run_id = (!task.calibration).then_some(task.run_id);
    if !task.calibration
        && let Some(prepare) = &options.prepare
    {
        options
            .execute(prepare, false, run_id)
            .context("the prepare command failed")?;
    }
    let execution = options.execute(&task.spec.inv, !task.calibration, run_id)?;
    if !task.calibration
        && let Some(conclude) = &options.conclude
    {
        options
            .execute(conclude, false, run_id)
            .context("the conclude command failed")?;
    }
    Ok(execution)
}

/// Pick the next command to schedule, round-robin from `rr`. Warmup runs of a
/// command are exhausted (and drained) before its timed runs start.
fn pick(states: &[CmdState], rr: usize) -> Option<(usize, bool)> {
    let n = states.len();
    for offset in 0..n {
        let i = (rr + offset) % n;
        let s = &states[i];
        if s.completed {
            continue;
        }
        if s.warmup_remaining > 0 {
            return Some((i, true));
        }
        if s.warmup_in_flight > 0 {
            continue; // wait for warmups to finish before timing this command
        }
        match s.timed_remaining {
            Some(0) => continue, // all dispatched, draining in-flight
            _ => return Some((i, false)),
        }
    }
    None
}

fn done_dispatching(s: &CmdState, interrupted: bool) -> bool {
    interrupted
        || (s.warmup_remaining == 0 && s.warmup_in_flight == 0 && s.timed_remaining == Some(0))
}

/// Minimum timed runs before --target may stop a command.
const TARGET_MIN_RUNS: usize = 10;

/// Whether the 95% CI half-width of the mean is within ±`target` percent of
/// it, i.e. the command has been measured precisely enough to stop (--target).
fn target_reached(s: &CmdState, target: f64) -> bool {
    let n = s.measurements.len();
    if n < TARGET_MIN_RUNS {
        return false;
    }
    let mean = s.sum / n as f64;
    let variance = (s.sum_sq - s.sum * s.sum / n as f64) / (n - 1) as f64;
    1.96 * variance.max(0.0).sqrt() / (n as f64).sqrt() < target / 100.0 * mean
}

/// Compare a command's aggregates against the calibrated measurement floor,
/// once at least two timed runs are in. `Some` is the error aborting the run.
fn check_floor(s: &CmdState, base: &Baseline, precision: usize) -> Option<anyhow::Error> {
    let count = s.measurements.len();
    if count < 2 {
        return None;
    }
    let mean = s.sum / count as f64;
    if mean < base.time {
        return Some(anyhow!(
            "'{}' mean {} is below the /bin/true floor {} : measurement too low to be precise (dominated by spawn/measurement overhead). You can remove this check by using --no-calibrate cli options.",
            s.spec.label,
            format_time(mean, auto_unit(mean), precision),
            format_time(base.time, auto_unit(base.time), precision),
        ));
    }
    if s.max_rss < base.rss {
        return Some(anyhow!(
            "'{}' peak RSS {} is below the /bin/true floor {} : measurement dominated by megafine's own resident set. You can remove this check by using --no-calibrate cli options.",
            s.spec.label,
            format_bytes(s.max_rss),
            format_bytes(base.rss),
        ));
    }
    None
}

/// Render every command's live counter as one aligned block and send it to the
/// display, so all rows share a unit and line up (see `render_counters`).
fn publish_counters(
    states: &[CmdState],
    progress: Option<&Progress>,
    options: &Options,
    counters: &mut Vec<f64>,
    display_tx: &Sender<DisplayMessage>,
) {
    // (count, center, std) of one command's timed runs so far. `counters` is a
    // reused buffer for the wall-clock times, sorted only when the estimator
    // needs order.
    let summary = |counters: &mut Vec<f64>, s: &CmdState| {
        counters.clear();
        counters.extend(s.measurements.iter().map(|x| x.wall_clock));
        if matches!(options.estimator, Estimator::Percentile(_)) {
            counters.sort_unstable_by(f64::total_cmp);
        }
        let (_, std) = crate::stats::mean_stddev(counters);
        (
            s.measurements.len() as u64,
            options.estimator.value(counters),
            std,
        )
    };
    let (_, ref_center, ref_std) = summary(counters, &states[options.reference]);

    let rows: Vec<CounterRow> = states
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (count, center, std) = summary(counters, s);
            // Live counterpart of the final ranking, against the reference's
            // current center; absent until both sides have a measurement.
            let relative = if states.len() < 2 {
                None
            } else if i == options.reference {
                Some(Relative::Reference)
            } else if count > 0 && ref_center > 0.0 {
                let stddev = match (std, ref_std) {
                    (Some(std), Some(ref_std)) => {
                        Some(crate::stats::ratio_stddev(center, std, ref_center, ref_std))
                    }
                    _ => None,
                };
                Some(Relative::Ratio {
                    ratio: center / ref_center,
                    stddev,
                })
            } else {
                None
            };
            // Mean counter values, once every run carries counters.
            let perf =
                (count > 0 && s.measurements.iter().all(|e| e.counters.is_some())).then(|| {
                    let (mut instr, mut cycles, mut cache, mut branch) = (0.0f64, 0.0, 0.0, 0.0);
                    for e in &s.measurements {
                        let c = e.counters.as_ref().unwrap();
                        instr += c.instructions as f64;
                        cycles += c.cycles as f64;
                        cache += c.cache_misses as f64;
                        branch += c.branch_misses as f64;
                    }
                    let n = s.measurements.len() as f64;
                    PerfCell {
                        instr: instr / n,
                        ipc: if cycles > 0.0 { instr / cycles } else { 0.0 },
                        cache_misses: cache / n,
                        branch_misses: branch / n,
                    }
                });
            CounterRow {
                label: &s.spec.label,
                count,
                center,
                std,
                peak_rss: s.max_rss,
                perf,
                relative,
            }
        })
        .collect();
    // The display draws each counter as "   {msg}" then trims to one less than
    // the width, so reserve the 3-space indent and that final column.
    let budget = term_width().saturating_sub(4);
    let mut lines = render_counters(&rows, options.time_unit, options.precision, budget);
    // Rate-based ETA: elapsed wall time scaled by the tasks still to run. The
    // task rate absorbs hooks, warmups and parallelism, which the per-run
    // measurements don't see.
    if let Some(p) = progress
        && p.done > 0
        && p.done < p.total
    {
        let remaining =
            p.started.elapsed().as_secs_f64() * (p.total - p.done) as f64 / p.done as f64;
        lines.push(format!("ETA ~{}", format_duration(remaining)));
    }
    let _ = display_tx.send(DisplayMessage::Counters(lines));
}

/// Run all benchmarks concurrently, keeping every worker busy by dispatching
/// runs across commands (and multiple concurrent runs of the same command).
/// Returns the collected results (command order) plus the error that aborted
/// the run, if any, so the measurements gathered before a failure can still be
/// reported. `Err` is reserved for failures before any run starts.
pub fn run_benchmarks(
    commands: Vec<Command>,
    options: &Options,
    interrupted: Arc<AtomicBool>,
) -> Result<(Vec<BenchmarkResult>, Option<anyhow::Error>), anyhow::Error> {
    let jobs = options.jobs;
    debug!(jobs, commands = commands.len(), "starting scheduler");

    // Disjoint CPU set per worker, so concurrent jobs cannot contend and skew
    // each other's timings. Empty when pinning is off; indexed by worker id.
    let groups = if options.pin {
        let cpus = allowed_cpus()?;
        if jobs > cpus.len().saturating_sub(options.pin_reserved) {
            bail!(
                "cannot pin {jobs} jobs to {} available CPU(s) ({} allowed, {} reserved); \
                 reduce --jobs/--pin-reserved or pass --no-pin",
                cpus.len().saturating_sub(options.pin_reserved),
                cpus.len(),
                options.pin_reserved,
            );
        }
        let (reserved, groups) = partition(&cpus, jobs, options.pin_reserved);
        debug!(?groups, "pinned workers to CPUs");
        if !reserved.is_empty() {
            match pin_thread(&reserved) {
                Err(e) => warn!(error = %e, "could not pin main/display threads to reserved CPUs"),
                Ok(()) => debug!(?reserved, "pinned main/display threads to reserved CPUs"),
            }
        }
        groups
    } else {
        Vec::new()
    };
    let groups = &groups;

    // Installed after the main thread is pinned, so the handler is also isolated
    {
        let interrupted = interrupted.clone();
        ctrlc::set_handler(move || interrupted.store(true, Ordering::SeqCst))
            .context("failed to install Ctrl-C handler")?;
    }

    let command_labels: Vec<String> = commands.iter().map(|c| c.label().to_string()).collect();

    // Parse every command line once, before any thread (or --setup) runs, so a
    // malformed command aborts upfront instead of at its first execution.
    let specs: Vec<Arc<CmdSpec>> = commands
        .iter()
        .map(|command| {
            Ok(Arc::new(CmdSpec {
                label: Box::from(command.label()),
                inv: options.invocation(command.line)?,
            }))
        })
        .collect::<Result<_>>()?;

    let (job_tx, job_rx) = bounded::<Task>(jobs);
    let (result_tx, result_rx) = unbounded::<RunReport>();
    let (display_tx, display_rx) = unbounded::<DisplayMessage>();
    let display = spawn_display(
        jobs,
        command_labels,
        display_rx,
        matches!(options.output, crate::options::OutputMode::Inherit),
    );

    let outcome = std::thread::scope(
        move |scope| -> Result<(Vec<BenchmarkResult>, Option<anyhow::Error>), anyhow::Error> {
            // {{{ Create workers
            for w in 0..jobs {
                let job_rx = job_rx.clone();
                let result_tx = result_tx.clone();
                let display_tx = display_tx.clone();
                let cpus = groups.get(w).map(Vec::as_slice);
                scope.spawn(move || {
                    // Pin once: spawned commands inherit this thread's affinity.
                    if let Some(cpus) = cpus
                        && let Err(e) = pin_thread(cpus)
                    {
                        // A subset of our own CPUs should always be settable;
                        // warn and continue unpinned rather than abort the run.
                        warn!(worker = w, error = %e, "could not pin worker to its CPUs");
                    }
                    while let Ok(task) = job_rx.recv() {
                        let _ = display_tx.send(if task.calibration {
                            DisplayMessage::Calibrate(w)
                        } else {
                            DisplayMessage::Start(w, task.cmd_idx)
                        });
                        let outcome = run_task(options, &task);
                        let _ = display_tx.send(DisplayMessage::Idle(w));
                        let _ = result_tx.send(RunReport {
                            cmd_idx: task.cmd_idx,
                            warmup: task.warmup,
                            outcome,
                        });
                    }
                });
            }
            // }}}
            // {{{ Calibrate measurement
            let baseline = if options.calibrate {
                let spec = Arc::new(CmdSpec {
                    label: Box::from("/bin/true"),
                    inv: options.invocation("/bin/true")?,
                });
                calibrate(jobs, &spec, &job_tx, &result_rx)
            } else {
                None
            };
            if let Some(b) = &baseline {
                debug!(
                    floor_time = b.time,
                    floor_rss = b.rss,
                    "calibrated measurement floor"
                );
            }
            let timed_remaining = if baseline.is_some() {
                options.runs.map(|r| r.max(2))
            } else {
                options.runs
            };
            // }}}
            // {{{ Results vec
            let mut states: Vec<_> = specs
                .into_iter()
                .map(|spec| CmdState {
                    spec,
                    warmup_remaining: options.warmup,
                    warmup_in_flight: 0,
                    timed_remaining,
                    in_flight: 0,
                    measurements: Vec::new(),
                    sum: 0.0,
                    sum_sq: 0.0,
                    max_rss: 0,
                    last_update: Instant::now() - COUNTER_REFRESH,
                    completed: false,
                })
                .collect();
            // }}}

            let mut aborted = false;
            let mut abort_error: Option<anyhow::Error> = None;

            // {{{ setup step
            if let Some(setup) = &options.setup {
                for _ in states.iter() {
                    // no '?' operator here for unicity
                    if let Err(e) = options.execute(setup, false, None) {
                        aborted = true;
                        abort_error = Some(e.context("the setup command failed"));
                        break;
                    }
                }
            }
            // }}}
            // {{{ action function
            let fill = |states: &mut [CmdState],
                        rr: &mut usize,
                        in_flight_total: &mut usize,
                        next_run_id: &mut u64,
                        stop: bool|
             -> bool {
                if stop {
                    return true;
                }
                while *in_flight_total < jobs {
                    let Some((i, warmup)) = pick(states, *rr) else {
                        break;
                    };
                    *rr = (i + 1) % states.len();
                    let s = &mut states[i];
                    let task = Task {
                        cmd_idx: i,
                        warmup,
                        calibration: false,
                        run_id: *next_run_id,
                        spec: s.spec.clone(),
                    };
                    *next_run_id += 1;
                    if warmup {
                        s.warmup_remaining -= 1;
                        s.warmup_in_flight += 1;
                    } else if let Some(r) = s.timed_remaining {
                        s.timed_remaining = Some(r - 1);
                    }
                    s.in_flight += 1;
                    if job_tx.send(task).is_err() {
                        return false;
                    }
                    *in_flight_total += 1;
                }
                true
            };
            // }}}
            // {{{ boostrap+loop
            let mut in_flight_total = 0usize;
            let mut rr = 0usize;
            let mut next_run_id = 0u64;
            // Reused by publish_counters for each row's wall-clock times.
            let mut counters: Vec<f64> = Vec::new();
            let mut progress = timed_remaining.map(|runs| Progress {
                total: states.len() as u64 * (options.warmup + runs),
                done: 0,
                started: Instant::now(),
            });

            fill(
                &mut states,
                &mut rr,
                &mut in_flight_total,
                &mut next_run_id,
                aborted || interrupted.load(Ordering::Relaxed),
            );

            while in_flight_total > 0 {
                let Ok(report) = result_rx.recv() else {
                    break;
                };
                in_flight_total -= 1;
                let intr = interrupted.load(Ordering::Relaxed);
                if intr {
                    progress = None; // the remaining runs won't happen: drop the ETA
                } else if let Some(p) = &mut progress {
                    p.done += 1;
                }

                let i = report.cmd_idx;
                let mut publish = false;
                {
                    let s = &mut states[i];
                    s.in_flight -= 1;
                    if report.warmup {
                        s.warmup_in_flight -= 1;
                    }

                    match report.outcome {
                        Err(e) if !intr && !aborted => {
                            aborted = true;
                            abort_error = Some(e);
                        }
                        Ok(r) if !report.warmup && !intr && !aborted => {
                            s.sum += r.wall_clock;
                            s.sum_sq += r.wall_clock * r.wall_clock;
                            s.max_rss = s.max_rss.max(r.max_rss);
                            s.measurements.push(r);
                            if let Some(base) = &baseline
                                && let Some(e) = check_floor(s, base, options.precision)
                            {
                                aborted = true;
                                abort_error = Some(e);
                            }
                            if let Some(target) = options.target
                                && s.timed_remaining != Some(0)
                                && target_reached(s, target)
                            {
                                // Stop dispatching; the drain/completed
                                // machinery finishes the command as usual.
                                s.timed_remaining = Some(0);
                            }
                            if s.last_update.elapsed() >= COUNTER_REFRESH {
                                s.last_update = Instant::now();
                                publish = true;
                            }
                        }
                        _ => {}
                    }
                }
                if publish {
                    publish_counters(
                        &states,
                        progress.as_ref(),
                        options,
                        &mut counters,
                        &display_tx,
                    );
                }

                // Finalize a command once it stops producing work and drains.
                if !aborted
                    && !states[i].completed
                    && done_dispatching(&states[i], intr)
                    && states[i].in_flight == 0
                {
                    states[i].completed = true;
                    publish_counters(
                        &states,
                        progress.as_ref(),
                        options,
                        &mut counters,
                        &display_tx,
                    );
                    if let Some(cleanup) = &options.cleanup
                        && let Err(e) = options.execute(cleanup, false, None)
                    {
                        aborted = true;
                        abort_error = Some(e.context("the cleanup command failed"));
                    }
                }

                if !fill(
                    &mut states,
                    &mut rr,
                    &mut in_flight_total,
                    &mut next_run_id,
                    aborted || intr,
                ) {
                    break;
                }
            }
            // }}}

            // Stop the workers: dropping the last job sender disconnects the
            // channel, so every worker's recv() errors and its thread exits;
            // the scope joins them when this closure returns.
            drop(job_tx);

            let mut results = Vec::with_capacity(states.len());
            for s in states {
                if !s.measurements.is_empty() {
                    results.push(BenchmarkResult {
                        label: s.spec.label.to_string(),
                        measurements: s.measurements,
                    });
                }
            }

            let error = match abort_error {
                Some(e) if !interrupted.load(Ordering::SeqCst) => Some(e),
                _ => None,
            };
            Ok((results, error))
        },
    );

    let _ = display.join();
    outcome
}
