//! Ancillary binary to exercise `megafine --region`.
//!
//! Takes three sleep durations (seconds) and brackets the middle one with the
//! region markers, so `megafine --region` should report ≈ the 2nd value:
//!
//!     sleep(before); megafine_start(); sleep(region); megafine_stop(); sleep(after);
//!
//! Usage: megafine-region-rs <before> <region> <after>

#[path = "megafine.rs"]
mod megafine;

use std::time::Duration;

fn main() {
    let args: Vec<f64> = std::env::args()
        .skip(1)
        .map(|x| x.parse().expect("durations must be floats (seconds)"))
        .collect();
    let &[before, region, after] = args.as_slice() else {
        eprintln!("usage: megafine-region-rs <before> <region> <after>  (seconds)");
        std::process::exit(2);
    };

    std::thread::sleep(Duration::from_secs_f64(before));
    megafine::megafine_start();
    std::thread::sleep(Duration::from_secs_f64(region));
    megafine::megafine_stop();
    std::thread::sleep(Duration::from_secs_f64(after));
}
