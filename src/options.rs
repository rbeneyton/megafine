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
    pub cleanup: Option<String>,
    pub time_unit: Option<TimeUnit>,
    /// Time only the command's `megafine_start()`/`megafine_stop()` region.
    pub region: bool,
    /// Calibrate the measurement floor against `/bin/true` before timing.
    pub calibrate: bool,
    /// Pin each concurrent job to its own disjoint subset of the allowed CPUs.
    pub pin: bool,
}

fn all_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.into())
        .unwrap_or(1)
}

impl Options {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let jobs = match cli.jobs {
            None | Some(0) => all_cores(),
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

        if cli.command_name.len() > cli.commands.len() {
            bail!(
                "got {} command names but only {} command(s)",
                cli.command_name.len(),
                cli.commands.len()
            );
        }

        Ok(Options {
            jobs,
            warmup: cli.warmup,
            runs: cli.runs,
            shell,
            setup: cli.setup.clone(),
            prepare: cli.prepare.clone(),
            cleanup: cli.cleanup.clone(),
            time_unit,
            region: cli.region,
            calibrate: !cli.region && !cli.no_calibrate,
            pin: !cli.no_pin,
        })
    }
}
