mod cli;
mod command;
mod display;
mod executor;
mod format;
mod measurement;
mod options;
mod parameter;
mod perf;
mod scheduler;
mod stats;

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail};
use clap::Parser;
use colored::Colorize;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::format::{auto_unit, format_bytes, format_count, format_time, relative_cell, truncate};
use crate::measurement::{BenchmarkResult, NormBenchmark, compute};
use crate::options::Options;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let mut cli = Cli::parse();
    if let Some(shell) = cli.completions {
        let mut cmd = <Cli as clap::CommandFactory>::command();
        clap_complete::generate(shell, &mut cmd, "megafine", &mut std::io::stdout());
        return Ok(());
    }
    if cli.commands == ["-"] {
        cli.commands = command::from_stdin()?;
    }
    if !cli.parameter_list.is_empty() || !cli.parameter_scan.is_empty() {
        (cli.commands, cli.command_name) = parameter::expand(&cli)?;
    }
    let cli = cli;

    let options = Options::from_cli(&cli)?;
    if options.counters {
        perf::probe()?;
    }
    let interrupted = Arc::new(AtomicBool::new(false));

    let commands = command::from_cli(&cli);
    let (results, error) = scheduler::run_benchmarks(commands, &options, interrupted)?;
    // The reference command may have produced no measurements (e.g. Ctrl-C or an
    // abort left fewer results than commands); keep compute() from indexing out
    // of range, without masking a pending run error with that diagnostic.
    let reference_missing = !results.is_empty() && options.reference >= results.len();
    if reference_missing && error.is_none() {
        bail!(
            "--reference {} has no measurements (only {} command(s) produced results)",
            options.reference + 1,
            results.len()
        );
    }
    if options.raw {
        // On a failed run, keep stdout empty: partial ratios could be mistaken
        // for complete results by the scripts consuming them.
        if error.is_none() {
            print_raw(&results, &options)?;
        }
    } else {
        print_results(&results, &options);
        if !reference_missing {
            print_ranks(&results, &options);
        }
    }

    match error {
        Some(e) => {
            // A failed command's exit code becomes megafine's exit code, so
            // wrapping scripts see the same failure as running it directly.
            if let Some(f) = e.downcast_ref::<executor::CommandFailed>() {
                eprintln!("Error: {e:?}");
                std::process::exit(f.code);
            }
            Err(e)
        }
        None => Ok(()),
    }
}

/// Print one ratio per line (command order, relative to the `reference`-th
/// command), and nothing else, so stdout can be consumed directly by scripts.
fn print_raw(results: &[BenchmarkResult], options: &Options) -> Result<()> {
    if results.len() < 2 {
        bail!("--raw needs measurements for at least 2 commands (run interrupted too early?)");
    }
    let relative = compute(results, options.reference, options.estimator)
        .context("could not compute relative speed (a benchmark time is zero)")?;
    for item in &relative {
        println!("{:.6}", item.ratio);
    }
    Ok(())
}

/// Columns available for stdout: the terminal width, or unbounded when stdout
/// isn't a terminal (piped output should not be truncated).
fn out_cols() -> usize {
    if std::io::stdout().is_terminal() {
        display::term_width()
    } else {
        usize::MAX
    }
}

fn print_results(results: &[BenchmarkResult], options: &Options) {
    let cols = out_cols();
    for (idx, result) in results.iter().enumerate() {
        let prefix = format!("Benchmark {}: ", idx + 1);
        let label = truncate(&result.label, cols.saturating_sub(prefix.chars().count()));
        println!(
            "{} {}: {}",
            "Benchmark".bold(),
            (idx + 1).to_string().bold(),
            label
        );

        let mut times = result.times(|e| e.wall_clock);
        if times.is_empty() {
            println!("  {}", "no measurements collected".yellow());
            println!();
            continue;
        }
        times.sort_unstable_by(f64::total_cmp);

        // σ stays the sample stddev whatever the estimator (it describes the
        // data's spread, not the estimator's uncertainty).
        let center = options.estimator.value(&times);
        let (_, stddev) = stats::mean_stddev(&times);
        let unit = options.time_unit.unwrap_or_else(|| auto_unit(center));
        let precision = options.precision;
        let center_of = |mut v: Vec<f64>| {
            v.sort_unstable_by(f64::total_cmp);
            options.estimator.value(&v)
        };
        let user = center_of(result.times(|e| e.time_user));
        let system = center_of(result.times(|e| e.time_system));
        let peak = result
            .measurements
            .iter()
            .map(|x| x.max_rss)
            .max()
            .unwrap_or(0);

        match stddev {
            Some(stddev) => {
                println!(
                    "  Time ({} ± {}):  {} ± {}    [User: {}, System: {}, Peak: {}]",
                    options.estimator.to_string().green().bold(),
                    "σ".green(),
                    format_time(center, unit, precision).green().bold(),
                    format_time(stddev, unit, precision).green(),
                    format_time(user, unit, precision).blue(),
                    format_time(system, unit, precision).blue(),
                    format_bytes(peak).blue(),
                );
                println!(
                    "  Range ({} … {}):  {} … {}    {} runs",
                    "min".cyan(),
                    "max".purple(),
                    format_time(stats::min(&times), unit, precision).cyan(),
                    format_time(stats::max(&times), unit, precision).purple(),
                    times.len(),
                );
            }
            None => {
                println!(
                    "  Time ({}):  {}    [User: {}, System: {}, Peak: {}]   {} run",
                    "abs".green().bold(),
                    format_time(center, unit, precision).green().bold(),
                    format_time(user, unit, precision).blue(),
                    format_time(system, unit, precision).blue(),
                    format_bytes(peak).blue(),
                    times.len(),
                );
            }
        }

        println!(
            "  Faults ({}/{}):  {} / {}    CtxSw ({}/{}):  {} / {}",
            "maj".cyan(),
            "min".purple(),
            format_count(center_of(result.times(|e| e.major_faults as f64))).blue(),
            format_count(center_of(result.times(|e| e.minor_faults as f64))).blue(),
            "vol".cyan(),
            "invol".purple(),
            format_count(center_of(result.times(|e| e.vol_ctx_switches as f64))).blue(),
            format_count(center_of(result.times(|e| e.invol_ctx_switches as f64))).blue(),
        );

        if result.measurements.iter().all(|e| e.counters.is_some()) {
            let counter = |get: fn(&perf::PerfCounts) -> u64| {
                center_of(result.times(|e| get(e.counters.as_ref().unwrap()) as f64))
            };
            let instructions = counter(|c| c.instructions);
            let cycles = counter(|c| c.cycles);
            let ipc = if cycles > 0.0 {
                instructions / cycles
            } else {
                0.0
            };
            println!(
                "  Counters:  {} instr, {} cycles, IPC {}, {} cache-miss, {} branch-miss",
                format_count(instructions).blue(),
                format_count(cycles).blue(),
                format!("{ipc:.2}").blue(),
                format_count(counter(|c| c.cache_misses)).blue(),
                format_count(counter(|c| c.branch_misses)).blue(),
            );
        }

        println!();
    }
}

fn print_ranks(results: &[BenchmarkResult], options: &Options) {
    if results.len() < 2 {
        return;
    }

    let Some(relative) = compute(results, options.reference, options.estimator) else {
        eprintln!(
            "{}: could not compute relative speed (a benchmark time is zero)",
            "Note".red()
        );
        return;
    };

    // Rank by central time (1 = fastest) while keeping the rows in command order.
    let mut order: Vec<usize> = (0..relative.len()).collect();
    order.sort_by(|&a, &b| relative[a].center.total_cmp(&relative[b].center));
    let mut rank = vec![0usize; relative.len()];
    for (pos, &i) in order.iter().enumerate() {
        rank[i] = pos + 1;
    }
    let fastest = order[0];
    let slowest = order[order.len() - 1];

    let cols = out_cols();
    // The rank column is as wide as the largest rank number or the "Rank" header.
    let rank_w = relative.len().to_string().chars().count().max("Rank".len());

    // Widths to right-align the percentage and uncertainty so their decimal
    // points line up (every value has two decimals, so equal width aligns them).
    let pct_w = relative
        .iter()
        .filter(|item| !item.is_reference)
        .map(|item| format!("{:+.2}", (item.ratio - 1.0) * 100.0).len())
        .max()
        .unwrap_or(0);
    let unc_w = relative
        .iter()
        .filter_map(|item| item.stddev.map(|s| format!("{:.2}", s * 100.0).len()))
        .max();

    // The duration text and optional tag width that follow each label.
    let tails: Vec<(String, usize)> = relative
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let suffix = if item.is_reference {
                ": reference".to_string()
            } else {
                // Percentage difference from the reference, with the propagated
                // uncertainty (both in percentage points).
                format!(": {}", relative_cell(item.ratio, item.stddev, pct_w, unc_w))
            };
            let tag_w = if i == fastest || i == slowest {
                " (fastest)".chars().count()
            } else {
                0
            };
            (suffix, tag_w)
        })
        .collect();

    // One shared label width, so every ": …" result starts at the same column.
    // Reserve the indent (2), the widest tail, and the rank column ("  " + rank).
    let max_tail = tails
        .iter()
        .map(|(s, t)| s.chars().count() + t)
        .max()
        .unwrap();
    let natural = relative
        .iter()
        .map(|item| item.result.label.chars().count())
        .max()
        .unwrap();
    let label_w = natural.min(cols.saturating_sub(2 + max_tail + 2 + rank_w));

    // Each row keeps its visible width separately, since the (fastest)/(slowest)
    // suffixes embed ANSI codes that must not count toward column alignment.
    let rows: Vec<(String, usize)> = relative
        .iter()
        .zip(&tails)
        .enumerate()
        .map(|(i, (item, (suffix, _)))| {
            let label = truncate(&item.result.label, label_w);
            let mut left = format!("  {label:<label_w$}{suffix}");
            let mut width = left.chars().count();
            if i == fastest {
                left.push_str(&format!(" {}", "(fastest)".green().bold()));
                width += " (fastest)".chars().count();
            }
            if i == slowest {
                left.push_str(&format!(" {}", "(slowest)".red().bold()));
                width += " (slowest)".chars().count();
            }
            (left, width)
        })
        .collect();

    let col = rows.iter().map(|(_, width)| *width).max().unwrap();
    // Pad manually: bolding "Results" embeds ANSI codes that {:<col$} would miscount.
    let pad = " ".repeat(col.saturating_sub("Results".len()));
    println!("{}{pad}  {}", "Results".bold(), "Rank".bold());
    for ((left, width), r) in rows.iter().zip(&rank) {
        let pad = " ".repeat(col - width);
        println!("{left}{pad}  {}", r.to_string().bold());
    }

    // Welch's t-test against the reference: flag the rows whose difference
    // could plausibly be measurement noise (silence = significant at 5%).
    let summary = |item: &NormBenchmark| {
        let times = item.result.times(|e| e.wall_clock);
        let (m, s) = stats::mean_stddev(&times);
        (m, s, times.len())
    };
    let reference = relative.iter().find(|i| i.is_reference).unwrap();
    let (ref_m, ref_s, ref_n) = summary(reference);
    for item in relative.iter().filter(|i| !i.is_reference) {
        let (m, s, n) = summary(item);
        let (Some(s), Some(ref_s)) = (s, ref_s) else {
            continue;
        };
        let (t, df) = stats::welch_t(m, s, n, ref_m, ref_s, ref_n);
        let p = stats::t_test_p(t, df);
        if p >= 0.05 {
            println!(
                "{}: '{}' is not significantly different from the reference (p = {p:.2})",
                "Note".yellow(),
                item.result.label,
            );
        }
    }
}
