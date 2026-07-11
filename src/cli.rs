use std::num::NonZeroU64;

use clap::Parser;

#[derive(Parser)]
#[command(
    name = "megafine",
    version,
    about = "A multithreaded command-line benchmarking tool."
)]
pub struct Cli {
    /// The command(s) to benchmark, or `-` to read them from stdin (one per line).
    #[arg(required_unless_present = "completions", num_args = 1.., value_name = "command")]
    pub commands: Vec<String>,

    /// Print the completion script for SHELL (bash, zsh, fish, …) on stdout and exit.
    #[arg(
        long = "gen-completions",
        value_name = "SHELL",
        hide = true,
        help_heading = "Output"
    )]
    pub completions: Option<clap_complete::Shell>,

    #[arg(
        short = 'j',
        long,
        value_name = "JOBS",
        help_heading = "Execution",
        help = format!(
            "Run JOBS command invocations simultaneously (0: auto) \
             [default: available CPUs ({}) minus --pin-reserved]",
            crate::options::all_cores()
        )
    )]
    pub jobs: Option<usize>,

    /// How runs are dispatched to the workers: saturate (keep every worker
    /// busy), sequential (one run at a time, round-robin across commands;
    /// implies --jobs 1), exclusive (a command never runs concurrently with
    /// itself), or fair (lockstep rounds; every command keeps the same
    /// number of runs) [default: saturate].
    #[arg(long, value_name = "POLICY", help_heading = "Execution")]
    pub schedule: Option<String>,

    /// Perform NUM warmup runs before the actual benchmark.
    #[arg(
        short,
        long,
        value_name = "NUM",
        default_value_t = 0,
        help_heading = "Execution"
    )]
    pub warmup: u64,

    /// Perform exactly NUM runs. If omitted, run until interrupted with Ctrl-C.
    #[arg(short, long, value_name = "NUM", help_heading = "Execution")]
    pub runs: Option<NonZeroU64>,

    /// Run each command until the 95% confidence interval of its mean is
    /// within ±PCT percent (10 runs minimum; --runs then acts as a cap).
    /// Needs the mean estimator.
    #[arg(long, value_name = "PCT", help_heading = "Execution")]
    pub target: Option<f64>,

    /// Run commands through the current shell instead of direct execution.
    #[arg(short = 'S', long, help_heading = "Execution")]
    pub shell: bool,

    /// Keep timing runs that exit non-zero instead of aborting; their
    /// measurements still count and megafine exits 0.
    #[arg(short = 'i', long, help_heading = "Execution")]
    pub ignore_failure: bool,

    /// Kill a timing run (whole process group) after SECONDS and treat it as
    /// a failed run (aborts the benchmark unless --ignore-failure).
    #[arg(long, value_name = "SECONDS", help_heading = "Execution")]
    pub timeout: Option<f64>,

    /// Where the benchmarked command's stdout goes: null, inherit (disables
    /// the live display), or a FILE truncated on every run [default: null].
    #[arg(long, value_name = "WHERE", help_heading = "Execution")]
    pub output: Option<String>,

    /// What the benchmarked command reads on stdin: null or a FILE [default: null].
    #[arg(long, value_name = "WHERE", help_heading = "Execution")]
    pub input: Option<String>,

    /// Execute CMD once before the timing runs of each command.
    #[arg(short = 's', long, value_name = "CMD", help_heading = "Hooks")]
    pub setup: Option<String>,

    /// Execute CMD before each timing run.
    #[arg(short = 'p', long, value_name = "CMD", help_heading = "Hooks")]
    pub prepare: Option<String>,

    /// Execute CMD after each timing run.
    #[arg(long, value_name = "CMD", help_heading = "Hooks")]
    pub conclude: Option<String>,

    /// Execute CMD after all timing runs of each command.
    #[arg(short = 'c', long, value_name = "CMD", help_heading = "Hooks")]
    pub cleanup: Option<String>,

    /// Measure only the region the command brackets with megafine_start() and
    /// megafine_stop() (see instrument/megafine.[h|rs|py]).
    #[arg(short = 'R', long, help_heading = "Measurement")]
    pub region: bool,

    /// The measured quantity all statistics (estimator, ranking, --target,
    /// significance verdict) are computed on: time (wall clock, or the region
    /// window with --region), user, sys, rss, major-faults, minor-faults,
    /// vol-ctx, invol-ctx, instructions, cycles, cache-misses or
    /// branch-misses (counter metrics imply --counters; all but time stay
    /// whole-process under --region) [default: time].
    #[arg(short = 'm', long, value_name = "METRIC", help_heading = "Measurement")]
    pub metric: Option<String>,

    /// Also collect hardware counters per run via perf events (user space, whole child tree):
    /// instructions retired, CPU cycles, IPC, cache misses and branch misses.
    #[arg(long, help_heading = "Measurement")]
    pub counters: bool,

    /// Skip the /bin/true calibration that establishes the measurement floor.
    #[arg(long, help_heading = "Measurement")]
    pub no_calibrate: bool,

    /// Do not pin each concurrent job to its own CPU core(s). By default the
    /// allowed CPUs are partitioned across the jobs so they cannot contend.
    #[arg(long, help_heading = "Measurement")]
    pub no_pin: bool,

    /// Book NUM CPUs (lowest ids) for megafine's own threads.
    #[arg(
        long,
        value_name = "NUM",
        default_value_t = 0,
        conflicts_with = "no_pin",
        help_heading = "Measurement"
    )]
    pub pin_reserved: usize,

    /// Benchmark each command once per VALUE, substituting it for `{NAME}` in
    /// the command (and -n names). VALUES is a comma-separated list (use a
    /// trailing comma for a single value: `1,`), or @FILE to read one value
    /// per line. Repeatable; several lists multiply.
    #[arg(short = 'L', long = "parameter-list", num_args = 2, value_names = ["NAME", "VALUES"], help_heading = "Parameters")]
    pub parameter_list: Vec<String>,

    /// Benchmark each command once per value of NAME = MIN..=MAX (stepped by
    /// --parameter-step-size), substituting it for `{NAME}`.
    #[arg(short = 'P', long = "parameter-scan", num_args = 3, value_names = ["NAME", "MIN", "MAX"], help_heading = "Parameters")]
    pub parameter_scan: Vec<String>,

    /// Step between two consecutive --parameter-scan values.
    #[arg(
        long,
        value_name = "STEP",
        default_value_t = 1.0,
        help_heading = "Parameters"
    )]
    pub parameter_step_size: f64,

    /// Split each --parameter-scan MIN..=MAX into NUM equal steps (NUM+1
    /// evenly spaced values, both endpoints included) instead of a fixed
    /// step size. Incompatible with --parameter-step-size.
    #[arg(
        long,
        value_name = "NUM",
        conflicts_with = "parameter_step_size",
        help_heading = "Parameters"
    )]
    pub parameter_step_n: Option<usize>,

    /// Space the --parameter-step-n steps geometrically (logarithmically)
    /// between MIN and MAX instead of linearly; MIN must be positive.
    #[arg(long, requires = "parameter_step_n", help_heading = "Parameters")]
    pub parameter_step_log: bool,

    /// Display names for the commands, in order (one per command), should be
    /// last argument. Without any NAME, names are derived automatically by
    /// removing the common prefix and suffix from the commands.
    #[arg(short = 'n', long = "command-name", value_name = "NAME", num_args = 0.., help_heading = "Output")]
    pub command_name: Option<Vec<String>>,

    /// Display time unit: us, ms or s.
    #[arg(short = 'u', long, value_name = "UNIT", help_heading = "Output")]
    pub time_unit: Option<String>,

    /// Digits after the decimal point for displayed times.
    #[arg(
        long,
        value_name = "DIGITS",
        default_value_t = 3,
        help_heading = "Output"
    )]
    pub precision: usize,

    /// Central-value statistic for the metric and relative ratios: mean,
    /// median, or a percentile (p90, p999 = 99.9th) [default: mean].
    #[arg(long, value_name = "ESTIMATOR", help_heading = "Output")]
    pub estimator: Option<String>,

    /// Row order of the ranking table: command (input order) or metric (best
    /// first) [default: command].
    #[arg(long, value_name = "ORDER", help_heading = "Output")]
    pub sort: Option<String>,

    /// Print only the relative-speed ratios (one per line, command order,
    /// reference = first command or --reference) on stdout, for scripts.
    #[arg(long, help_heading = "Output")]
    pub raw: bool,

    /// Use the IDX-th command (1-based) as the relative-speed baseline
    /// [default: the first command].
    #[arg(long, value_name = "IDX", help_heading = "Output")]
    pub reference: Option<usize>,
}
