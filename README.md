# Megafine

A multithreaded command-line benchmarking tool.

Inspired by [hyperfine](https://crates.io/crates/hyperfine) with concurrent
execution support, sub-timing support and specialized scheduling.

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

## Differences with hyperfine

- -j support, with dedicated cpu affinity for workers
- region timing via `megafine_start()`/`megafine_stop()` instrumentation
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
- [ ] allow to force a pattern-defined scheduling (strict round robin per worker, …)

## Changelog

### [Unreleased]

- [x] Pin the main, display and Ctrl-C handler threads to the leftover CPUs.
- [x] Add `--pin-reserved NUM` to book NUM CPUs for megafine's own threads when
  no leftover exists.
- [x] Default `-j/--jobs` is the available CPUs minus `--pin-reserved`, and
  `--help` shows the current CPU count; `-j` now requires a value (`0` = auto,
  the bare `-j` form is gone)
- [x] `--raw` mode printing only the relative-speed ratios on stdout, for
  scripts or AI skill usage
- [x] `--reference IDX` to pick the relative-speed baseline (1-based command
  index; default: the first command)
- [x] Test suite: in-module unit tests (stats, formatting, ratio/reference
  computation, CPU partitioning, CLI validation) and end-to-end `tests/cli.rs`
  driving the binary (runs, `-j`, `--raw`, `--reference`, region, stdin, errors)
- [x] `MEGAFINE_RUN_ID` env var: unique incrementing run id (shared with the
  run's `--prepare`), to keep concurrently generated files apart
- [x] `--conclude CMD` hook (after each timing run, symmetric to `--prepare`,
  sharing its run id)
- [x] Add live ratio measurement
- [x] Fix resize issue
- [x] Dump the statistics collected so far when a run fails (non-zero return
  code), before reporting the error (in `--raw` mode stdout stays empty, so
  scripts cannot mistake partial ratios for complete results)
- [x] `--estimator` to pick the reported central statistic: mean (default),
  median, or a percentile (`p90`, `p999` = 99.9th), applied to the time
  report, the relative speeds and the live display
- [x] Live `ETA` line when `-r/--runs` is given (rate-based: elapsed wall time
  scaled by the warmup+timed runs still to go), refreshed with the counters
- [x] Avoid flooding refresh operations
- [x] Decode commands once at startup
- [x] Return task stderr on error
- [x] support parameters
- [x] allow to define a target stddev (ie. run until you've reached ±1%)
- [x] Add --counters option with perf data, and full rusage fields
- [x] `-L NAME @FILE` to read parameter values from a file, one per line
- [x] `--gen-completions SHELL` to print a shell completion script
- [x] --ignore-failure to keep timing commands that exit non-zero
- [x] `--timeout SECONDS` to kill a task (whole process group) and treat it as
  a failed run
- [x] stdout/stdin control for benchmarked commands (--output
  null|inherit|FILE, --input FILE; `--output inherit` supersedes a dedicated
  --show-output)

### [0.1.0] 2026-06-05

- [x] Concurrent multithreaded execution with a worker pool and run-level
  scheduling (`-j/--jobs`)
- [x] Fixed run count (`-r/--runs`) or run-until-Ctrl-C, with graceful partial
  results on interrupt
- [x] Warmup runs (`-w/--warmup`)
- [x] `--setup` / `--prepare` / `--cleanup` hooks
- [x] Direct execution or via the current shell (`-S/--shell`)
- [x] Per-command display names (`-n/--command-name`)
- [x] Statistics: mean ± σ, min…max range, and a relative-speed summary with
  propagated uncertainty
- [x] CPU user/system time and peak RSS per command
- [x] Time-unit selection (`-u/--time-unit`) with automatic unit picking
- [x] Live, column-aligned progress display sharing one unit per column
- [x] Region timing (`-R/--region`) via `megafine_start()`/`megafine_stop()`,
  with C/C++, Rust & python glue templates and example binaries/script
- [x] Calibrated measurement floor (`/bin/true`) that aborts sub-overhead runs
- [x] Read commands from stdin (`-`)
- [x] Worker sandboxing via cpu affinity (cpuset?)
- [x] Proper outputs truncate & alignment
- [x] Time decimal-digit selection (`--precision`)
