//! End-to-end tests driving the real `megafine` binary (no crate internals).
//! Runs are kept tiny and pass `--no-calibrate --no-pin` for speed and
//! determinism (pinning needs a permissive cpuset, calibration adds runs).

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn mf() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_megafine"));
    c.env("NO_COLOR", "1"); // stable, un-colored output for substring asserts
    c
}

fn run(args: &[&str]) -> Output {
    mf().args(args).output().expect("failed to run megafine")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn basic_run() {
    let out = run(&["-r", "2", "--no-calibrate", "--no-pin", "sleep 0.02"]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("Benchmark 1"), "stdout: {s}");
    assert!(s.contains("Time"), "stdout: {s}");
}

#[test]
fn two_commands_show_ranking() {
    let out = run(&[
        "-r", "2", "--no-calibrate", "--no-pin", "sleep 0.02", "sleep 0.05",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("reference"), "stdout: {s}");
    assert!(s.contains("Rank"), "stdout: {s}");
    assert!(s.contains("fastest"), "stdout: {s}");
}

#[test]
fn raw_outputs_only_ratios() {
    let out = run(&[
        "--raw", "-r", "2", "--no-calibrate", "--no-pin", "sleep 0.02", "sleep 0.05",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    let lines: Vec<&str> = s.lines().collect();
    assert_eq!(lines.len(), 2, "stdout: {lines:?}");
    assert_eq!(lines[0], "1.000000");
}

#[test]
fn raw_reference_picks_baseline() {
    let out = run(&[
        "--raw", "--reference", "2", "-r", "2", "--no-calibrate", "--no-pin", "sleep 0.02",
        "sleep 0.05",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let lines: Vec<String> = stdout(&out).lines().map(String::from).collect();
    assert_eq!(lines.len(), 2, "stdout: {lines:?}");
    assert_eq!(lines[1], "1.000000"); // the 2nd command is now the baseline
}

#[test]
fn reference_marks_chosen_command() {
    let out = run(&[
        "--reference", "2", "-r", "2", "--no-calibrate", "--no-pin", "sleep 0.02", "sleep 0.05",
        "sleep 0.08",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    let ref_line = s
        .lines()
        .find(|l| l.contains("reference"))
        .expect("a row should be the reference");
    assert!(ref_line.contains("sleep 0.05"), "ref line: {ref_line}");
}

#[test]
fn command_name_is_shown() {
    // Commands before -n, since -n greedily consumes the rest.
    let out = run(&[
        "-r", "2", "--no-calibrate", "--no-pin", "sleep 0.02", "-n", "MYNAME",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("MYNAME"), "stdout: {}", stdout(&out));
}

#[test]
fn reads_commands_from_stdin() {
    let mut child = mf()
        .args(["--no-calibrate", "--no-pin", "-r", "2", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"sleep 0.02\nsleep 0.03\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("Benchmark 1") && s.contains("Benchmark 2"), "stdout: {s}");
}

#[test]
fn jobs_and_warmup_smoke() {
    let out = run(&[
        "-j", "2", "-w", "1", "-r", "2", "--no-calibrate", "--no-pin", "sleep 0.02", "sleep 0.02",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
}

#[test]
fn region_mode_reports_a_benchmark() {
    // The region example brackets the middle sleep; region mode skips calibration.
    let region_bin = env!("CARGO_BIN_EXE_megafine-region-rs");
    let cmd = format!("{region_bin} 0.02 0.05 0.02");
    let out = run(&["--region", "-r", "2", "--no-pin", &cmd]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("Benchmark 1"), "stdout: {}", stdout(&out));
}

#[test]
fn raw_with_one_command_errors() {
    let out = run(&["--raw", "-r", "2", "--no-calibrate", "sleep 0.02"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("2 commands"), "stderr: {}", stderr(&out));
}

#[test]
fn reference_out_of_range_errors() {
    let out = run(&["--reference", "5", "-r", "2", "--no-calibrate", "a", "b"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("out of range"), "stderr: {}", stderr(&out));
}

#[test]
fn runs_zero_errors() {
    let out = run(&["-r", "0", "sleep 0.02"]);
    assert!(!out.status.success());
}

#[test]
fn failing_command_errors() {
    let out = run(&["-r", "1", "--no-calibrate", "--no-pin", "false"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("non-zero"), "stderr: {}", stderr(&out));
}
