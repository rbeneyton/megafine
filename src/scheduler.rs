use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use flume::{Receiver, Sender, bounded, unbounded};
use tracing::{debug, warn};

use crate::command::Command;
use crate::display::{DisplayMessage, spawn_display, term_width};
use crate::executor::{allowed_cpus, partition, pin_thread};
use crate::format::{CounterRow, Relative, auto_unit, format_bytes, format_time, render_counters};
use crate::measurement::{BenchmarkResult, Execution};
use crate::options::Options;

/// Minimum delay between live counter updates sent to the display.
const COUNTER_REFRESH: Duration = Duration::from_millis(100);

/// A command's immutable identity, shared with every task that runs it.
struct CmdSpec {
    label: Box<str>,
    command_line: Box<str>,
    prepare: Option<Box<str>>,
    conclude: Option<Box<str>>,
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

enum Job {
    Run(Task),
    Suicide,
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
    job_tx: &Sender<Job>,
    result_rx: &Receiver<RunReport>,
) -> Option<(Vec<f64>, Vec<u64>)> {
    let spec = Arc::new(CmdSpec {
        label: Box::from("/bin/true"),
        command_line: Box::from("/bin/true"),
        prepare: None,
        conclude: None,
    });
    for _ in 0..n {
        let task = Task {
            cmd_idx: 0,
            warmup: false,
            calibration: true,
            run_id: 0,
            spec: spec.clone(),
        };
        if job_tx.send(Job::Run(task)).is_err() {
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
    job_tx: &Sender<Job>,
    result_rx: &Receiver<RunReport>,
) -> Option<Baseline> {
    calibration_round(jobs, job_tx, result_rx)?; // warmup, discarded
    let (times, rss) = calibration_round(jobs, job_tx, result_rx)?;
    if times.is_empty() {
        return None;
    }
    Some(Baseline {
        time: crate::stats::mean(&times),
        rss: rss.iter().sum::<u64>() / rss.len() as u64,
    })
}

/// Per-command scheduling state, owned and mutated only by the pump.
struct CmdState {
    spec: Arc<CmdSpec>,
    warmup_remaining: u64,
    warmup_in_flight: u64,
    /// Timed runs still to dispatch; `None` means infinite (until Ctrl-C).
    timed_remaining: Option<u64>,
    in_flight: u64,
    measurements: Vec<Execution>,
    sum: f64,
    sum_sq: f64,
    max_rss: u64,
    last_update: Instant,
    completed: bool,
}

/// Run a single task on a worker: optional (unmeasured) prepare, then the
/// measured command, then optional (unmeasured) conclude. A non-zero exit
/// from any of them surfaces as an `Err`.
fn run_task(options: &Options, task: &Task) -> Result<Execution> {
    let run_id = (!task.calibration).then_some(task.run_id);
    if let Some(prepare) = &task.spec.prepare {
        options
            .execute(prepare, false, run_id)
            .context("the prepare command failed")?;
    }
    let execution = options.execute(&task.spec.command_line, options.region, run_id)?;
    if let Some(conclude) = &task.spec.conclude {
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

/// Render every command's live counter as one aligned block and send it to the
/// display, so all rows share a unit and line up (see `render_counters`).
fn publish_counters(states: &[CmdState], options: &Options, display_tx: &Sender<DisplayMessage>) {
    // (count, mean, std) of every command's timed runs so far.
    let summaries: Vec<(u64, f64, Option<f64>)> = states
        .iter()
        .map(|s| {
            let count = s.measurements.len() as u64;
            let n = count as f64;
            let mean = if count > 0 { s.sum / n } else { 0.0 };
            let std = if count >= 2 {
                Some((((s.sum_sq - s.sum * s.sum / n) / (n - 1.0)).max(0.0)).sqrt())
            } else {
                None
            };
            (count, mean, std)
        })
        .collect();
    let (_, ref_mean, ref_std) = summaries[options.reference];

    let rows: Vec<CounterRow> = states
        .iter()
        .zip(&summaries)
        .enumerate()
        .map(|(i, (s, &(count, mean, std)))| {
            // Live counterpart of the final ranking, against the reference's
            // current mean; absent until both sides have a measurement.
            let relative = if states.len() < 2 {
                None
            } else if i == options.reference {
                Some(Relative::Reference)
            } else if count > 0 && ref_mean > 0.0 {
                let stddev = match (std, ref_std) {
                    (Some(std), Some(ref_std)) => {
                        Some(crate::stats::ratio_stddev(mean, std, ref_mean, ref_std))
                    }
                    _ => None,
                };
                Some(Relative::Ratio {
                    ratio: mean / ref_mean,
                    stddev,
                })
            } else {
                None
            };
            CounterRow {
                label: &s.spec.label,
                count,
                mean,
                std,
                peak_rss: s.max_rss,
                relative,
            }
        })
        .collect();
    // The display draws each counter as "   {msg}" then trims to one less than
    // the width, so reserve the 3-space indent and that final column.
    let budget = term_width().saturating_sub(4);
    let lines = render_counters(&rows, options.time_unit, budget);
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

    let (job_tx, job_rx) = bounded::<Job>(jobs);
    let (result_tx, result_rx) = unbounded::<RunReport>();
    let (display_tx, display_rx) = unbounded::<DisplayMessage>();
    let display = spawn_display(jobs, command_labels, display_rx);
    let display_canary = &AtomicUsize::new(jobs);

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
                    loop {
                        match job_rx.recv() {
                            Ok(Job::Run(task)) => {
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
                            Ok(Job::Suicide) => {
                                if display_canary.fetch_sub(1, Ordering::SeqCst) == 1 {
                                    let _ = display_tx.send(DisplayMessage::Done);
                                }
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                });
            }
            // }}}
            // {{{ Calibrate measurement
            let baseline = if options.calibrate {
                calibrate(jobs, &job_tx, &result_rx)
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
            let mut states: Vec<_> = commands
                .into_iter()
                .map(|command| {
                    let spec = Arc::new(CmdSpec {
                        label: Box::from(command.label()),
                        command_line: Box::from(command.line),
                        prepare: options.prepare.as_deref().map(Box::from),
                        conclude: options.conclude.as_deref().map(Box::from),
                    });
                    CmdState {
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
                    }
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
                    if job_tx.send(Job::Run(task)).is_err() {
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
                            if let Some(base) = &baseline {
                                let count = s.measurements.len();
                                if count >= 2 {
                                    let mean = s.sum / count as f64;
                                    if mean < base.time {
                                        aborted = true;
                                        abort_error = Some(anyhow!(
                                            "'{}' mean {} is below the /bin/true floor {} : measurement too low to be precise (dominated by spawn/measurement overhead). You can remove this check by using --no-calibrate cli options.",
                                            s.spec.label,
                                            format_time(mean, auto_unit(mean)),
                                            format_time(base.time, auto_unit(base.time)),
                                        ));
                                    } else if s.max_rss < base.rss {
                                        aborted = true;
                                        abort_error = Some(anyhow!(
                                            "'{}' peak RSS {} is below the /bin/true floor {} : measurement dominated by megafine's own resident set. You can remove this check by using --no-calibrate cli options.",
                                            s.spec.label,
                                            format_bytes(s.max_rss),
                                            format_bytes(base.rss),
                                        ));
                                    }
                                }
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
                    publish_counters(&states, options, &display_tx);
                }

                // Finalize a command once it stops producing work and drains.
                if !aborted
                    && !states[i].completed
                    && done_dispatching(&states[i], intr)
                    && states[i].in_flight == 0
                {
                    states[i].completed = true;
                    publish_counters(&states, options, &display_tx);
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

            // Stop the workers; the scope joins them when this closure returns.
            for _ in 0..jobs {
                let _ = job_tx.send(Job::Suicide);
            }

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
