# Megafine

A multithreaded command-line benchmarking tool.

Inspired by [hyperfine](https://crates.io/crates/hyperfine) with concurrent
execution support, sub-timing support and specialized scheduling.

![megafine demo](docs/demo.svg)

## Usage

Compare 10 times 2 commands, using 2 workers:

```sh
megafine -j 2 -r 10 'sleep 0.1' 'sleep 0.15'
```

Without `-r/--runs`, megafine runs each command indefinitely until you press
Ctrl-C, at which point it stops and prints the statistics collected so far:

```sh
megafine -j 2 'sleep 0.1' 'sleep 0.15'   # runs until Ctrl-C
```

Without `-j/--jobs`, megafine uses all available cores (minus the
`--pin-reserved` ones, if any).

You can pass commands via your shell as expected, here 8 sleep commands via
fish:

```sh
megafine (printf 'sleep %s\n' (seq 1 8))
```

Or pass `-` to read the commands from stdin, one per line (options still apply;
put `-n NAME...` after the `-` if you need):

```sh
printf 'sleep %s\n' (seq 1 8) | megafine -j 2 -
```

### Full usage

```sh
A multithreaded command-line benchmarking tool.

Usage: megafine [OPTIONS] [command]...

Arguments:
  [command]...  The command(s) to benchmark, or `-` to read them from stdin (one per line)

Options:
  -h, --help     Print help
  -V, --version  Print version

Output:
  -n, --command-name [<NAME>...]  Display names for the commands, in order (one per command), should be last argument. Without any NAME, names are derived automatically by removing the common prefix and suffix from the commands
  -u, --time-unit <UNIT>          Display time unit: us, ms or s
      --precision <DIGITS>        Digits after the decimal point for displayed times [default: 3]
      --estimator <ESTIMATOR>     Central-value statistic for the metric and relative ratios: mean, median, or a percentile (p90, p999 = 99.9th) [default: mean]
      --sort <ORDER>              Row order of the ranking table: command (input order) or metric (best first) [default: command]
      --raw                       Print only the relative-speed ratios (one per line, command order, reference = first command or --reference) on stdout, for scripts
      --reference <IDX>           Use the IDX-th command (1-based) as the relative-speed baseline [default: the first command]

Execution:
  -j, --jobs <JOBS>        Run JOBS command invocations simultaneously (0: auto) [default: available CPUs (16) minus --pin-reserved]
      --schedule <POLICY>  How runs are dispatched to the workers: saturate (keep every worker busy), sequential (one run at a time, round-robin across commands; implies --jobs 1), exclusive (a command never runs concurrently with
                           itself), or fair (lockstep rounds; every command keeps the same number of runs) [default: saturate]
  -w, --warmup <NUM>       Perform NUM warmup runs before the actual benchmark [default: 0]
  -r, --runs <NUM>         Perform exactly NUM runs. If omitted, run until interrupted with Ctrl-C
      --target <PCT>       Run each command until the 95% confidence interval of its mean is within ±PCT percent (10 runs minimum; --runs then acts as a cap). Needs the mean estimator
  -S, --shell              Run commands through the current shell instead of direct execution
  -i, --ignore-failure     Keep timing runs that exit non-zero instead of aborting; their measurements still count and megafine exits 0
      --timeout <SECONDS>  Kill a timing run (whole process group) after SECONDS and treat it as a failed run (aborts the benchmark unless --ignore-failure)
      --output <WHERE>     Where the benchmarked command's stdout goes: null, inherit (disables the live display), or a FILE truncated on every run [default: null]
      --input <WHERE>      What the benchmarked command reads on stdin: null or a FILE [default: null]

Hooks:
  -s, --setup <CMD>     Execute CMD once before the timing runs of each command
  -p, --prepare <CMD>   Execute CMD before each timing run
      --conclude <CMD>  Execute CMD after each timing run
  -c, --cleanup <CMD>   Execute CMD after all timing runs of each command

Measurement:
  -R, --region              Measure only the region the command brackets with megafine_start() and megafine_stop() (see instrument/megafine.[h|rs|py])
  -m, --metric <METRIC>     The measured quantity all statistics (estimator, ranking, --target, significance verdict) are computed on: time (wall clock, or the region window with --region), user, sys, rss, major-faults, minor-faults,
                            vol-ctx, invol-ctx, instructions, cycles, cache-misses or branch-misses (counter metrics imply --counters; all but time stay whole-process under --region) [default: time]
      --counters            Also collect hardware counters per run via perf events (user space, whole child tree): instructions retired, CPU cycles, IPC, cache misses and branch misses
      --no-calibrate        Skip the /bin/true calibration that establishes the measurement floor
      --no-pin              Do not pin each concurrent job to its own CPU core(s). By default the allowed CPUs are partitioned across the jobs so they cannot contend
      --pin-reserved <NUM>  Book NUM CPUs (lowest ids) for megafine's own threads [default: 0]

Parameters:
  -L, --parameter-list <NAME> <VALUES>     Benchmark each command once per VALUE, substituting it for `{NAME}` in the command (and -n names). VALUES is a comma-separated list (use a trailing comma for a single value: `1,`), or @FILE to
                                           read one value per line. Repeatable; several lists multiply
  -P, --parameter-scan <NAME> <MIN> <MAX>  Benchmark each command once per value of NAME = MIN..=MAX (stepped by --parameter-step-size), substituting it for `{NAME}`
      --parameter-step-size <STEP>         Step between two consecutive --parameter-scan values [default: 1]
      --parameter-step-n <NUM>             Split each --parameter-scan MIN..=MAX into NUM equal steps (NUM+1 evenly spaced values, both endpoints included) instead of a fixed step size. Incompatible with --parameter-step-size
      --parameter-step-log                 Space the --parameter-step-n steps geometrically (logarithmically) between MIN and MAX instead of linearly; MIN must be positive
```

### Runtime schema

```plain
start ─┬─▶ /bin/true  ─┬─▶ setup ─┬─▶ prepare ──▶ command ──▶ conclude ─┬─▶ cleanup ──▶ report
       │               │          │                                     │
       ├─▶ ·········· ─┤          ├─▶ ································ ─┤
       │ (calibration) │          │    (1..jobs concurrent workers)     │
       └─▶ ·········  ─┘          └─▶ ································ ─┘
```

## Differences with hyperfine

- -j support, with dedicated cpu affinity for workers
- region timing via `megafine_start()`/`megafine_stop()` instrumentation
- `--metric` to rank on any collected quantity (time, rss, perf counters, …),
  not just wall clock
- no Windows/MacOS support
- no export in many formats (markdown, json, …)
- no shell specification, either current one or direct execution
- GPL instead of MIT

## History

This project takes roots in concurrent measurements feature request [hyperfine
issue 58](https://github.com/sharkdp/hyperfine/issues/58).

As hyperfine output is strongly coupled to internal sequential execution, the
fix/implementation is not trivial. After waiting for resolution for many
years, a clean proof of concept from scratch have been coded, and is polished
and published here, targeting my own needs, and not all hyperfine features.

IA have been used to manage output formatting (truncate, alignment, …),
documentation and some plumbery code.

## Features

### Warmup/Setup/Prepare/Cleanup

Exactly like hyperfine:
- --warmup <NUM> : will run NUM instances of each commands (on any worker,
  potentially simultaneously) without measuring timings;
- --setup 'CMD' : will run this command before, once per commands;
- --prepare 'CMD' : will run this command before each command execution;
- --conclude 'CMD' : will run this command after each timing run (symmetric
  to --prepare);
- --cleanup 'CMD' : will run this command after each command execution;

### Run id

Every run is given a unique id, incrementing from 0 (warmup runs included),
exposed to the benchmarked command as the `MEGAFINE_RUN_ID` environment
variable. The `--prepare` and `--conclude` commands of a run see the same
value as the run itself. Since megafine executes runs concurrently, use it to keep generated
files apart and avoid any contention:

```sh
megafine -S -j 4 -r 10 -p 'generate > in.$MEGAFINE_RUN_ID' 'process in.$MEGAFINE_RUN_ID'
```

Note that `$MEGAFINE_RUN_ID` expansion needs a shell (`-S`, or wrap the
command in `sh -c '…'`); direct execution does not expand variables.

### Region timing

The goal here is to avoid measurements to be polluted by initial data loading,
asynchronous calls, IO operations, … and only focus on the particular code
that user want to optimize/focus on.

With `-R/--region`, megafine times only the section the program brackets with
`megafine_start()` / `megafine_stop()` calls inside the user's process,
instead of the overall duration.

Include the instrumentation template and bracket the region of interest:

```c
#include "megafine.h"          /* from instrument/ */

megafine_start();
/* ... code to measure ... */
megafine_stop();
```

Then

```sh
g++ -O2 -Iinstrument ... -o ./my_program
megafine --region -r 10 ./my_program
```

The calls are no-ops unless run under `--region` (which passes `MEGAFINE_FD`
in the environment), so the instrumented binary still builds and runs normally
on its own.

Rust, C/C++ and python glue-code templates are provided in `instrument/`:
`megafine.rs`,  `megafine.h` and `megafine.py`. Note that the protocol might
evolve, so use same megafine version than the glue-code.

The `megafine-region-rs`, `megafine-region-cpp` and `megafine-region-py` examples in
`instrument/` sleep for three given durations, bracketing the middle one, so you
can see the effect. Build them with:

```sh
cargo build --bin megafine-region-rs
g++ -O2 -Iinstrument instrument/megafine-region-cpp.cpp -o megafine-region-cpp
```

Then

```sh
# sleeps 0.1s, then 0.2s inside the region, then 0.1s
megafine --region -r 10 './megafine-region-cpp 0.1 0.2 0.1'   # Time ≈ 200 ms
megafine          -r 10 './megafine-region-cpp 0.1 0.2 0.1'   # Time ≈ 400 ms
```

Notes:
- The reported **Time** is the in-process wall-clock summed over all
  start/stop pairs. **User/System/Peak** stay whole-process (`rusage` can't be
  scoped to a sub-region).
- Keep instrumentation coarse (events buffer until the run ends).
- The region is assumed single-threaded / non-overlapping.

### Raw output

`--raw` prints only the relative-speed ratios on stdout, one per line in
command order (the reference command = `1.000000`), for scripts or AI skill
usage. The live display goes to stderr, so stdout can be piped directly:

```sh
megafine --raw -r 10 'sleep 0.1' 'sleep 0.15'
1.000000
1.500372
```

The baseline is the first command by default; `--reference IDX` picks the
IDX-th command (1-based, matching the `Benchmark N:` numbering) instead, both
here and in the normal ranking output.

### Shell completions

`--gen-completions SHELL` prints a completion script (bash, zsh, fish, …) on
stdout. Redirect it wherever your shell looks for completions, e.g. for fish:

```sh
megafine --gen-completions fish > ~/.config/fish/completions/megafine.fish
```

### Measurement floor

megafine spawns commands with `posix_spawn` (vfork), so both the wall-clock and
the reported `Peak` carry a fixed overhead as the child briefly shares megafine's
address space before `exec`, so `Peak` includes megafine's own resident set
(~4-5 MiB), and very fast commands are dominated by spawn time.

To keep results honest, at startup megafine runs `/bin/true` twice per worker
(a warmup and a measurements pass), and takes the mean wall-clock and peak RSS
of the measured round as a noise floor. If a benchmarked command is below
those values (mean time or peak RSS), the run is aborted with a message and a
non-zero exit, after printing the statistics collected so far (as for any
failed run). Calibration and this check are skipped in `--region` mode (it
times a sub-window and forks for its pipe, so the whole-process floor doesn't
apply), or if `--no-calibrate` option is used.

## Roadmap

- [ ] Outlier detection / warnings
- [ ] Measurements offloading via slurm/MPI
- [ ] rdtsc measurement in region mode, for finer measurements
- [ ] multiple slots support in region mode, to allow multiple comparison
  inside same process or subtle timings 
- [ ] pretty progress bars when -r is specified (low priority)
- [ ] ebpf metrics
