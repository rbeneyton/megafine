//! megafine region-timing instrumentation (Rust).
//!
//! Bracket the region you want timed with `megafine_start()` / `megafine_stop()`.
//! The calls are no-ops unless the program is run under `megafine --region`
//! (which sets `MEGAFINE_FD` in the environment), so the instrumented binary
//! still builds and runs normally on its own.
//!
//! Requires the `libc` crate. These are the target's own functions (nothing
//! interposes them), so no special linkage is needed.
//!
//! ```ignore
//! megafine_start();
//! // ... code to measure ...
//! megafine_stop();
//! ```

use std::sync::OnceLock;

fn region_fd() -> Option<i32> {
    static FD: OnceLock<Option<i32>> = OnceLock::new();
    *FD.get_or_init(|| std::env::var("MEGAFINE_FD").ok()?.parse().ok())
}

/// Emit one 9-byte event `[tag:u8][ns:u64 native-endian]` to `MEGAFINE_FD`.
/// 9 < `PIPE_BUF`, so the write is atomic and safe from multiple threads.
fn emit(tag: u8) {
    let Some(fd) = region_fd() else { return };
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    let ns = ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64;
    let mut buf = [0u8; 9];
    buf[0] = tag;
    buf[1..].copy_from_slice(&ns.to_ne_bytes());
    // Non-owning write; never closes the inherited fd.
    unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
}

pub fn megafine_start() {
    emit(0);
}

pub fn megafine_stop() {
    emit(1);
}
