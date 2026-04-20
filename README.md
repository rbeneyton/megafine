# megafine

A multithreaded command-line benchmarking tool.

Inspired by [hyperfine](https://crates.io/crates/hyperfine) with concurrent execution support and specialized scheduling of runs.

## Usage

```sh
megafine -j 2 'sleep 0.1' 'sleep 0.15'
```
