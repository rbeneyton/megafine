# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/)

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

- Live display drawn on the alternate screen (like `top`), so it never reaches
  the terminal scrollback and survives resizes and font-size changes. It used
  to be redrawn in place, which sprayed a copy of every frame into the history
  whenever the display was taller than the terminal or the terminal was
  resized mid-run
- SIGTERM/SIGHUP stop the run like Ctrl-C (results so far are printed) instead
  of killing megafine on the alternate screen

### Security

## [0.2.0] - 2026-07-12

Full features release.

### Added

- Pin the main, display and Ctrl-C handler threads to the leftover CPUs.
- Add `--pin-reserved NUM` to book NUM CPUs for megafine's own threads when no
  leftover exists.
- `--raw` mode printing only the relative-speed ratios on stdout, for scripts
  or AI skill usage
- `--reference IDX` to pick the relative-speed baseline (1-based command
  index; default: the first command)
- Test suite: in-module unit tests (stats, formatting, ratio/reference
  computation, CPU partitioning, CLI validation) and end-to-end `tests/cli.rs`
  driving the binary (runs, `-j`, `--raw`, `--reference`, region, stdin,
  errors)
- `MEGAFINE_RUN_ID` env var: unique incrementing run id (shared with the run's
  `--prepare`), to keep concurrently generated files apart
- `--conclude CMD` hook (after each timing run, symmetric to `--prepare`,
  sharing its run id)
- Add live ratio measurement
- Dump the statistics collected so far when a run fails (non-zero return
  code), before reporting the error (in `--raw` mode stdout stays empty, so
  scripts cannot mistake partial ratios for complete results)
- `--estimator` to pick the reported central statistic: mean (default),
  median, or a percentile (`p90`, `p999` = 99.9th), applied to the time
  report, the relative speeds and the live display
- Live `ETA` line when `-r/--runs` is given (rate-based: elapsed wall time
  scaled by the warmup+timed runs still to go), refreshed with the counters
- Decode commands once at startup
- Return task stderr on error
- support parameters
- allow to define a target stddev (ie. run until you've reached ±1%)
- Add --counters option with perf data, and full rusage fields
- `-L NAME @FILE` to read parameter values from a file, one per line
- `--gen-completions SHELL` to print a shell completion script
- --ignore-failure to keep timing commands that exit non-zero
- `--timeout SECONDS` to kill a task (whole process group) and treat it as a
  failed run
- stdout/stdin control for benchmarked commands (--output null|inherit|FILE,
  --input FILE; `--output inherit` supersedes a dedicated --show-output)
- `--sort command|metric` to order the ranking table
- `--metric
  time|user|sys|rss|…|instructions|cycles|cache-misses|branch-misses`
- `--parameter-step-n NUM` to split a `--parameter-scan` range into NUM equal
  steps (NUM+1 values) instead of a fixed step size; incompatible with
  `--parameter-step-size`
- `--parameter-step-log` to space the `--parameter-step-n` steps geometrically
  (log scale) between MIN and MAX; MIN must be positive
- Bare `-n` (no NAME) derives the display names automatically by removing the
  common prefix and suffix from the commands
- Add `--schedule saturate|sequential|exclusive|fair` to allow to control
  scheduling

### Fixed

- Default `-j/--jobs` is the available CPUs minus `--pin-reserved`, and
  `--help` shows the current CPU count; `-j` now requires a value (`0` = auto,
  the bare `-j` form is gone)
- Fix resize issue
- Avoid flooding refresh operations

## [0.1.0] - 2026-06-05

Initial basic working version.

### Added

- Concurrent multithreaded execution with a worker pool and run-level
  scheduling (`-j/--jobs`)
- Fixed run count (`-r/--runs`) or run-until-Ctrl-C, with graceful partial
  results on interrupt
- Warmup runs (`-w/--warmup`)
- `--setup` / `--prepare` / `--cleanup` hooks
- Direct execution or via the current shell (`-S/--shell`)
- Per-command display names (`-n/--command-name`)
- Statistics: mean ± σ, min…max range, and a relative-speed summary with
  propagated uncertainty
- CPU user/system time and peak RSS per command
- Time-unit selection (`-u/--time-unit`) with automatic unit picking
- Live, column-aligned progress display sharing one unit per column
- Region timing (`-R/--region`) via `megafine_start()`/`megafine_stop()`, with
  C/C++, Rust & python glue templates and example binaries/script
- Calibrated measurement floor (`/bin/true`) that aborts sub-overhead runs
- Read commands from stdin (`-`)
- Worker sandboxing via cpu affinity (cpuset?)
- Proper outputs truncate & alignment
- Time decimal-digit selection (`--precision`)
