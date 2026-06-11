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

    /// Run JOBS command invocations simultaneously (no value: use all cores).
    #[arg(short = 'j', long, value_name = "JOBS", num_args = 0..=1, default_missing_value = "0")]
    pub jobs: Option<usize>,

    /// Perform NUM warmup runs before the actual benchmark.
    #[arg(short, long, value_name = "NUM", default_value_t = 0)]
    pub warmup: u64,

    /// Perform exactly NUM runs. If omitted, run until interrupted with Ctrl-C.
    #[arg(short, long, value_name = "NUM")]
    pub runs: Option<u64>,

    /// Run commands through the current shell instead of direct execution.
    #[arg(short = 'S', long)]
    pub shell: bool,

    /// Execute CMD once before the timing runs of each command.
    #[arg(short = 's', long, value_name = "CMD")]
    pub setup: Option<String>,

    /// Execute CMD before each timing run.
    #[arg(short = 'p', long, value_name = "CMD")]
    pub prepare: Option<String>,

    /// Execute CMD after all timing runs of each command.
    #[arg(short = 'c', long, value_name = "CMD")]
    pub cleanup: Option<String>,

    /// Display names for the commands, in order (one per command), should be last argument.
    #[arg(short = 'n', long = "command-name", value_name = "NAME", num_args = 1..)]
    pub command_name: Vec<String>,

    /// Display time unit: us, ms or s.
    #[arg(short = 'u', long, value_name = "UNIT")]
    pub time_unit: Option<String>,

    /// Measure only the region the command brackets with megafine_start() and
    /// megafine_stop() (see instrument/megafine.[h|rs]).
    #[arg(short = 'R', long)]
    pub region: bool,

    /// Skip the /bin/true calibration that establishes the measurement floor.
    #[arg(long)]
    pub no_calibrate: bool,

    /// Do not pin each concurrent job to its own CPU core(s). By default the
    /// allowed CPUs are partitioned across the jobs so they cannot contend.
    #[arg(long)]
    pub no_pin: bool,

    /// Book NUM CPUs (lowest ids) for megafine's own threads.
    #[arg(
        long,
        value_name = "NUM",
        default_value_t = 0,
        conflicts_with = "no_pin"
    )]
    pub pin_reserved: usize,
}
