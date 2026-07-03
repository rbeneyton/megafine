mod cli;
mod command;
mod display;
mod executor;
mod format;
mod measurement;
mod options;
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
use crate::format::{auto_unit, format_bytes, format_time, truncate};
use crate::measurement::{BenchmarkResult, compute};
use crate::options::Options;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let mut cli = Cli::parse();
    if cli.commands == ["-"] {
        cli.commands = command::from_stdin()?;
    }
    let cli = cli;

    let options = Options::from_cli(&cli)?;
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
        Some(e) => Err(e),
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
                    format_time(center, unit).green().bold(),
                    format_time(stddev, unit).green(),
                    format_time(user, unit).blue(),
                    format_time(system, unit).blue(),
                    format_bytes(peak).blue(),
                );
                println!(
                    "  Range ({} … {}):  {} … {}    {} runs",
                    "min".cyan(),
                    "max".purple(),
                    format_time(stats::min(&times), unit).cyan(),
                    format_time(stats::max(&times), unit).purple(),
                    times.len(),
                );
            }
            None => {
                println!(
                    "  Time ({}):  {}    [User: {}, System: {}, Peak: {}]   {} run",
                    "abs".green().bold(),
                    format_time(center, unit).green().bold(),
                    format_time(user, unit).blue(),
                    format_time(system, unit).blue(),
                    format_bytes(peak).blue(),
                    times.len(),
                );
            }
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
                let pct = format!("{:+.2}", (item.ratio - 1.0) * 100.0);
                match (item.stddev, unc_w) {
                    (Some(stddev), Some(uw)) => {
                        format!(
                            ": {pct:>pct_w$}% (± {:>uw$})",
                            format!("{:.2}", stddev * 100.0)
                        )
                    }
                    _ => format!(": {pct:>pct_w$}%"),
                }
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
}
