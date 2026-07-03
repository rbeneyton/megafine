use clap::Parser;

#[derive(Parser)]
#[command(
    name = "megafine",
    version,
    about = "A multithreaded command-line benchmarking tool."
)]
pub struct Cli {
    /// The command(s) to benchmark, or `-` to read them from stdin (one per line).
    #[arg(required = true, num_args = 1.., value_name = "command")]
    pub commands: Vec<String>,

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
    pub runs: Option<u64>,

    /// Run commands through the current shell instead of direct execution.
    #[arg(short = 'S', long, help_heading = "Execution")]
    pub shell: bool,

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

    /// Display names for the commands, in order (one per command), should be last argument.
    #[arg(short = 'n', long = "command-name", value_name = "NAME", num_args = 1.., help_heading = "Output")]
    pub command_name: Vec<String>,

    /// Display time unit: us, ms or s.
    #[arg(short = 'u', long, value_name = "UNIT", help_heading = "Output")]
    pub time_unit: Option<String>,

    /// Central-value statistic for times and relative speeds: mean, median,
    /// or a percentile (p90, p999 = 99.9th) [default: mean].
    #[arg(long, value_name = "ESTIMATOR", help_heading = "Output")]
    pub estimator: Option<String>,

    /// Print only the relative-speed ratios (one per line, command order,
    /// reference = first command or --reference) on stdout, for scripts.
    #[arg(long, help_heading = "Output")]
    pub raw: bool,

    /// Use the IDX-th command (1-based) as the relative-speed baseline
    /// [default: the first command].
    #[arg(long, value_name = "IDX", help_heading = "Output")]
    pub reference: Option<usize>,
}
