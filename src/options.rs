use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tracing::debug;

use crate::cli::Cli;
use crate::format::TimeUnit;

pub struct Options {
    pub jobs: usize,
    pub warmup: u64,
    pub runs: Option<u64>,
    /// `None` runs commands directly; `Some(path)` runs them through that shell.
    pub shell: Option<PathBuf>,
    pub setup: Option<String>,
    pub prepare: Option<String>,
    pub conclude: Option<String>,
    pub cleanup: Option<String>,
    pub time_unit: Option<TimeUnit>,
    /// Time only the command's `megafine_start()`/`megafine_stop()` region.
    pub region: bool,
    /// Calibrate the measurement floor against `/bin/true` before timing.
    pub calibrate: bool,
    /// Pin each concurrent job to its own disjoint subset of the allowed CPUs.
    pub pin: bool,
    /// CPUs booked for megafine's own threads, excluded from the workers' partition.
    pub pin_reserved: usize,
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

impl Options {
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

        if let Some(0) = cli.runs {
            bail!("--runs must be at least 1");
        }

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

        Ok(Options {
            jobs,
            warmup: cli.warmup,
            runs: cli.runs,
            shell,
            setup: cli.setup.clone(),
            prepare: cli.prepare.clone(),
            conclude: cli.conclude.clone(),
            cleanup: cli.cleanup.clone(),
            time_unit,
            region: cli.region,
            calibrate: !cli.region && !cli.no_calibrate,
            pin: !cli.no_pin,
            pin_reserved: cli.pin_reserved,
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
