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

Without `-j/--jobs`, megafine uses all available cores.

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
- no parameter scan
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
- --cleanup 'CMD' : will run this command after each command execution;

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

### Measurement floor

megafine spawns commands with `posix_spawn` (vfork), so both the wall-clock and
the reported `Peak` carry a fixed overhead as the child briefly shares megafine's
address space before `exec`, so `Peak` includes megafine's own resident set
(~4-5 MiB), and very fast commands are dominated by spawn time.

To keep results honest, at startup megafine runs `/bin/true` twice per worker
(a warmup and a measurements pass), and takes the mean wall-clock and peak RSS
of the measured round as a noise floor. If a benchmarked command is below
those values (mean time or peak RSS), the run is aborted with a message and a
non-zero exit. Calibration and this check are skipped in `--region` mode (it
times a sub-window and forks for its pipe, so the whole-process floor doesn't
apply), or if `--no-calibrate` option is used.

## Roadmap

- [ ] Outlier detection / warnings
- [ ] Measurements offloading via slurm/MPI
- [ ] rdtsc measurement in region mode, for finer measurements
- [ ] multiple slots support in region mode, to allow multiple comparison
  inside same process or subtle timings 
- [ ] pretty progress bars when -r is specified (low priority)
- [ ] add an estimated end time when -r is specified
- [ ] --raw output format for scripting, dumping all measurements or final ratios
- [ ] allow to define a target stddev (ie. run until you've reached ±1%)
- [ ] allow to force a pattern-defined scheduling (strict round robin per worker, …)
- [ ] add tests, in case of many features 

## Changelog

### [Unreleased]

- [x] Pin the main, display and Ctrl-C handler threads to the leftover CPUs.
- [x] Add `--pin-reserved NUM` to book NUM CPUs for megafine's own threads when
  no leftover exists.

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
