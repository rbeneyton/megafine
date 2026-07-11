use anyhow::{Context, Result, anyhow, bail};
use tracing::debug;

use crate::cli::Cli;
use crate::format::TimeUnit;
use crate::measurement::Metric;
use crate::stats::Estimator;

/// Destination of a measured command's stdout (--output). A file is truncated
/// on every run, so it holds the last run's output.
pub enum OutputMode {
    Null,
    Inherit,
    File(std::path::PathBuf),
}

/// Source of a measured command's stdin (--input). A file is reopened on
/// every run, so each run reads it from the start.
pub enum InputMode {
    Null,
    File(std::path::PathBuf),
}

/// Row order of the ranking table (--sort).
#[derive(PartialEq)]
pub enum Sort {
    /// The commands' input order (the `Benchmark N` numbering).
    Command,
    /// Best central value first.
    Metric,
}

/// A command line pre-parsed into the argv it will be spawned with, so the
/// parse happens once at startup (fail-fast) instead of on every run.
pub struct Invocation {
    /// The original command line, for labels and error messages.
    pub line: String,
    pub argv: Vec<String>,
}

pub struct Options {
    pub jobs: usize,
    pub warmup: u64,
    pub runs: Option<u64>,
    /// Stop a command once the 95% CI half-width of its mean is below this
    /// percentage of the mean.
    pub target: Option<f64>,
    /// `None` runs commands directly; `Some(path)` runs them through that shell.
    pub shell: Option<String>,
    /// Keep timing runs that exit non-zero as failed measurements.
    pub ignore_failure: bool,
    /// Kill a timing run's process group once it exceeds this duration.
    pub timeout: Option<std::time::Duration>,
    /// stdout destination of the timing runs.
    pub output: OutputMode,
    /// stdin source of the timing runs.
    pub input: InputMode,
    pub setup: Option<Invocation>,
    pub prepare: Option<Invocation>,
    pub conclude: Option<Invocation>,
    pub cleanup: Option<Invocation>,
    pub time_unit: Option<TimeUnit>,
    /// Digits after the decimal point for displayed times.
    pub precision: usize,
    /// Central-value statistic reported for the metric and relative ratios.
    pub estimator: Estimator,
    /// The per-run quantity all statistics are computed on.
    pub metric: Metric,
    /// Time only the command's `megafine_start()`/`megafine_stop()` region.
    pub region: bool,
    /// Calibrate the measurement floor against `/bin/true` before timing.
    pub calibrate: bool,
    /// Collect hardware perf counters for every run.
    pub counters: bool,
    /// Pin each concurrent job to its own disjoint subset of the allowed CPUs.
    pub pin: bool,
    /// CPUs booked for megafine's own threads, excluded from the workers' partition.
    pub pin_reserved: usize,
    /// Row order of the ranking table.
    pub sort: Sort,
    /// Print only the relative-speed ratios on stdout.
    pub raw: bool,
    /// 0-based index of the command used as the relative-speed baseline.
    pub reference: usize,
}

pub fn all_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.into())
        .unwrap_or(1)
}

/// Parse `line` into the argv `execute` will spawn: `[shell, -c, line]`
/// when a shell is configured, otherwise split into words here, once.
fn invocation(shell: Option<&str>, line: &str) -> Result<Invocation> {
    let argv = match shell {
        None => {
            let parts = shell_words::split(line)
                .with_context(|| format!("could not parse command '{line}'"))?;
            if parts.is_empty() {
                bail!("empty command '{line}'");
            }
            parts
        }
        Some(shell) => vec![shell.to_string(), "-c".into(), line.into()],
    };
    Ok(Invocation {
        line: line.to_string(),
        argv,
    })
}

impl Options {
    /// Parse `line` into the argv `execute` will spawn (see `invocation`).
    pub fn invocation(&self, line: &str) -> Result<Invocation> {
        invocation(self.shell.as_deref(), line)
    }

    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let jobs = match cli.jobs {
            None | Some(0) => {
                let cores = all_cores();
                if cores <= cli.pin_reserved {
                    bail!(
                        "--pin-reserved {} leaves no CPU for jobs ({cores} available); \
                         reduce it or set --jobs explicitly",
                        cli.pin_reserved
                    );
                }
                cores - cli.pin_reserved
            }
            Some(n) => n,
        };

        let shell = if cli.shell {
            let ppid = unsafe { libc::getppid() };
            let exe = format!("/proc/{ppid}/exe");
            let exe = std::fs::read_link(&exe)
                .with_context(|| format!("could not resolve the current shell via {exe}"))?;
            debug!("Current shell is {}", exe.display());
            let exe = exe
                .into_os_string()
                .into_string()
                .map_err(|p| anyhow!("the shell path {p:?} is not valid UTF-8"))?;
            Some(exe)
        } else {
            None
        };

        let time_unit = cli
            .time_unit
            .as_deref()
            .map(|s| {
                TimeUnit::parse(s)
                    .with_context(|| format!("invalid time unit '{s}' (use us, ms or s)"))
            })
            .transpose()?;

        let estimator = cli
            .estimator
            .as_deref()
            .map(|s| {
                Estimator::parse(s).with_context(|| {
                    format!(
                        "invalid estimator '{s}' (use mean, median, or a percentile like p90 or p999)"
                    )
                })
            })
            .transpose()?
            .unwrap_or(Estimator::Mean);

        let metric = cli
            .metric
            .as_deref()
            .map(|s| {
                Metric::parse(s).ok_or_else(|| {
                    anyhow!(
                        "invalid metric '{s}' (use time, user, sys, rss, major-faults, \
                         minor-faults, vol-ctx, invol-ctx, instructions, cycles, \
                         cache-misses or branch-misses)"
                    )
                })
            })
            .transpose()?
            .unwrap_or(Metric::Time);

        if let Some(0) = cli.runs {
            bail!("--runs must be at least 1");
        }

        if let Some(t) = cli.target {
            if t <= 0.0 {
                bail!("--target must be positive");
            }
            if estimator != Estimator::Mean {
                bail!("--target needs the mean estimator (percentile CIs are not supported)");
            }
        }

        let timeout = cli
            .timeout
            .map(|s| {
                if s <= 0.0 || !s.is_finite() {
                    bail!("--timeout must be positive");
                }
                Ok(std::time::Duration::from_secs_f64(s))
            })
            .transpose()?;

        // Keywords win over paths; a file literally named `null` is `./null`.
        let output = match cli.output.as_deref() {
            None | Some("null") => OutputMode::Null,
            Some("inherit") => OutputMode::Inherit,
            Some(path) => OutputMode::File(path.into()),
        };
        let input = match cli.input.as_deref() {
            None | Some("null") => InputMode::Null,
            Some(path) => {
                // Fail fast: probe once here, reopened for every run.
                std::fs::File::open(path)
                    .with_context(|| format!("--input: cannot open '{path}'"))?;
                InputMode::File(path.into())
            }
        };

        let sort = match cli.sort.as_deref() {
            None | Some("command") => Sort::Command,
            Some("metric") => Sort::Metric,
            Some(s) => bail!("invalid sort order '{s}' (use command or metric)"),
        };

        if cli.raw && cli.commands.len() < 2 {
            bail!("--raw needs at least 2 commands (it prints relative-speed ratios)");
        }

        if cli.command_name.len() > cli.commands.len() {
            bail!(
                "got {} command names but only {} command(s)",
                cli.command_name.len(),
                cli.commands.len()
            );
        }

        let reference = match cli.reference {
            None => 0,
            Some(0) => bail!("--reference must be at least 1 (1-based command index)"),
            Some(n) if n > cli.commands.len() => {
                bail!("--reference {n} out of range (1..={})", cli.commands.len())
            }
            Some(n) => n - 1,
        };

        let hook = |cmd: &Option<String>| {
            cmd.as_deref()
                .map(|s| invocation(shell.as_deref(), s))
                .transpose()
        };
        let setup = hook(&cli.setup)?;
        let prepare = hook(&cli.prepare)?;
        let conclude = hook(&cli.conclude)?;
        let cleanup = hook(&cli.cleanup)?;

        Ok(Options {
            jobs,
            warmup: cli.warmup,
            runs: cli.runs,
            target: cli.target,
            shell,
            ignore_failure: cli.ignore_failure,
            timeout,
            output,
            input,
            setup,
            prepare,
            conclude,
            cleanup,
            time_unit,
            precision: cli.precision,
            estimator,
            metric,
            region: cli.region,
            calibrate: !cli.region && !cli.no_calibrate,
            // Counter metrics need the counters; perf::probe() at startup
            // still reports a clear error when perf events are unavailable.
            counters: cli.counters || metric.needs_counters(),
            pin: !cli.no_pin,
            pin_reserved: cli.pin_reserved,
            sort,
            raw: cli.raw,
            reference,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// Parse args (clap) then build `Options`; binary name is implicit.
    fn opts(args: &[&str]) -> Result<Options> {
        let argv = std::iter::once("megafine").chain(args.iter().copied());
        let cli = Cli::try_parse_from(argv).expect("args should parse at the clap level");
        Options::from_cli(&cli)
    }

    #[test]
    fn defaults() {
        let o = opts(&["echo hi"]).unwrap();
        assert_eq!(o.jobs, all_cores());
        assert_eq!(o.reference, 0);
        assert!(o.pin);
        assert!(!o.raw);
    }

    #[test]
    fn ignore_failure_plumbs() {
        assert!(!opts(&["a"]).unwrap().ignore_failure);
        assert!(opts(&["-i", "a"]).unwrap().ignore_failure);
    }

    #[test]
    fn timeout_validation() {
        assert!(opts(&["--timeout", "0", "a"]).is_err());
        assert!(opts(&["--timeout=-1", "a"]).is_err());
        assert_eq!(
            opts(&["--timeout", "1.5", "a"]).unwrap().timeout,
            Some(std::time::Duration::from_millis(1500))
        );
    }

    #[test]
    fn output_input_parsing() {
        assert!(matches!(opts(&["a"]).unwrap().output, OutputMode::Null));
        assert!(matches!(
            opts(&["--output", "null", "a"]).unwrap().output,
            OutputMode::Null
        ));
        assert!(matches!(
            opts(&["--output", "inherit", "a"]).unwrap().output,
            OutputMode::Inherit
        ));
        assert!(matches!(
            opts(&["--output", "out.log", "a"]).unwrap().output,
            OutputMode::File(_)
        ));
        assert!(matches!(opts(&["a"]).unwrap().input, InputMode::Null));
        assert!(opts(&["--input", "/nonexistent/megafine-in", "a"]).is_err());
    }

    #[test]
    fn metric_defaults_and_parsing() {
        assert!(opts(&["a"]).unwrap().metric == Metric::Time);
        assert!(opts(&["--metric", "rss", "a"]).unwrap().metric == Metric::Rss);
        assert!(opts(&["--metric", "ipc", "a"]).is_err());
        assert!(opts(&["--metric", "bogus", "a"]).is_err());
    }

    #[test]
    fn counter_metric_auto_enables_counters() {
        assert!(!opts(&["a"]).unwrap().counters);
        assert!(opts(&["--metric", "instructions", "a"]).unwrap().counters);
        // A non-counter metric leaves counters off.
        assert!(!opts(&["--metric", "rss", "a"]).unwrap().counters);
    }

    #[test]
    fn sort_parsing() {
        assert!(opts(&["a"]).unwrap().sort == Sort::Command);
        assert!(opts(&["--sort", "metric", "a"]).unwrap().sort == Sort::Metric);
        assert!(opts(&["--sort", "time", "a"]).is_err());
    }

    #[test]
    fn explicit_jobs() {
        assert_eq!(opts(&["-j", "3", "a"]).unwrap().jobs, 3);
    }

    #[test]
    fn default_jobs_minus_reserved() {
        // Only meaningful when more than one CPU is available.
        if all_cores() > 1 {
            let o = opts(&["--pin-reserved", "1", "a"]).unwrap();
            assert_eq!(o.jobs, all_cores() - 1);
        }
    }

    #[test]
    fn runs_zero_rejected() {
        assert!(opts(&["-r", "0", "a"]).is_err());
    }

    #[test]
    fn estimator_defaults_to_mean() {
        assert!(opts(&["a"]).unwrap().estimator == Estimator::Mean);
    }

    #[test]
    fn estimator_parsing() {
        let e = opts(&["--estimator", "p999", "a"]).unwrap().estimator;
        assert!(e == Estimator::Percentile(99.9));
        assert!(opts(&["--estimator", "avg", "a"]).is_err());
    }

    #[test]
    fn target_validation() {
        assert!(opts(&["--target", "0", "a"]).is_err());
        assert!(opts(&["--target", "1", "--estimator", "p90", "a"]).is_err());
        assert_eq!(opts(&["--target", "1", "a"]).unwrap().target, Some(1.0));
    }

    #[test]
    fn raw_needs_two_commands() {
        assert!(opts(&["--raw", "a"]).is_err());
        assert!(opts(&["--raw", "a", "b"]).is_ok());
    }

    #[test]
    fn reference_validation() {
        assert!(opts(&["--reference", "0", "a", "b"]).is_err());
        assert!(opts(&["--reference", "99", "a", "b"]).is_err());
        assert_eq!(opts(&["--reference", "2", "a", "b"]).unwrap().reference, 1);
    }

    #[test]
    fn too_many_command_names() {
        assert!(opts(&["a", "-n", "x", "y"]).is_err());
    }

    #[test]
    fn no_pin_conflicts_with_pin_reserved() {
        // Enforced by clap, so parsing itself fails.
        let argv = ["megafine", "--no-pin", "--pin-reserved", "1", "a"];
        assert!(Cli::try_parse_from(argv).is_err());
    }
}
