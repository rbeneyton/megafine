# Megafine

A multithreaded command-line benchmarking tool.

Inspired by [hyperfine](https://crates.io/crates/hyperfine) with concurrent execution support and specialized scheduling of runs.

## Usage

```sh
megafine -j 2 'sleep 0.1' 'sleep 0.15'
```

## Differences with hyperfine

- -j support
- no Windows support
- GPL instead of MIT
- no export in many formats (markdown, json, …)

## History

This project takes roots in [hyperfine issue 58](https://github.com/sharkdp/hyperfine/issues/58).
As hyperfine output is strongly coupled to internal sequential execution, the fix/implementation is not trivial.
After waiting for resolution for many years, a clean proof of concept from scratch have been coded, and can be polished and published here, targeting only my own
needs, and not all hyperfine features.
