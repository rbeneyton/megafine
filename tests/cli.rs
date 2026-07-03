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
        "-r",
        "2",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
        "sleep 0.05",
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
        "--raw",
        "-r",
        "2",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
        "sleep 0.05",
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
        "--raw",
        "--reference",
        "2",
        "-r",
        "2",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
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
        "--reference",
        "2",
        "-r",
        "2",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
        "sleep 0.05",
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
        "-r",
        "2",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
        "-n",
        "MYNAME",
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
    assert!(
        s.contains("Benchmark 1") && s.contains("Benchmark 2"),
        "stdout: {s}"
    );
}

#[test]
fn jobs_and_warmup_smoke() {
    let out = run(&[
        "-j",
        "2",
        "-w",
        "1",
        "-r",
        "2",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
        "sleep 0.02",
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
    assert!(
        stdout(&out).contains("Benchmark 1"),
        "stdout: {}",
        stdout(&out)
    );
}

/// Temp file collecting one line per run, removed on drop.
struct RunLog(std::path::PathBuf);

impl RunLog {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("megafine-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        RunLog(path)
    }

    fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.0)
            .expect("run log should exist")
            .lines()
            .map(String::from)
            .collect()
    }
}

impl Drop for RunLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[test]
fn run_id_is_unique_and_incrementing() {
    let log = RunLog::new("run-id");
    let cmd = format!("sh -c 'echo $MEGAFINE_RUN_ID >> {}'", log.0.display());
    let out = run(&[
        "-j",
        "2",
        "-w",
        "2",
        "-r",
        "4",
        "--no-calibrate",
        "--no-pin",
        &cmd,
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let mut ids: Vec<u64> = log.lines().iter().map(|l| l.parse().unwrap()).collect();
    ids.sort_unstable();
    assert_eq!(ids, (0..6).collect::<Vec<u64>>()); // 2 warmups + 4 runs
}

#[test]
fn prepare_shares_run_id_with_its_run() {
    let log = RunLog::new("prepare-id");
    let prepare = format!("sh -c 'echo p$MEGAFINE_RUN_ID >> {}'", log.0.display());
    let cmd = format!("sh -c 'echo c$MEGAFINE_RUN_ID >> {}'", log.0.display());
    let out = run(&[
        "-p",
        &prepare,
        "-r",
        "3",
        "--no-calibrate",
        "--no-pin",
        &cmd,
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let mut lines = log.lines();
    lines.sort_unstable();
    assert_eq!(lines, vec!["c0", "c1", "c2", "p0", "p1", "p2"]);
}

#[test]
fn conclude_runs_after_each_run_with_its_run_id() {
    let log = RunLog::new("conclude-id");
    let cmd = format!("sh -c 'echo c$MEGAFINE_RUN_ID >> {}'", log.0.display());
    let conclude = format!("sh -c 'echo z$MEGAFINE_RUN_ID >> {}'", log.0.display());
    let out = run(&[
        "--conclude",
        &conclude,
        "-r",
        "3",
        "--no-calibrate",
        "--no-pin",
        &cmd,
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let mut lines = log.lines();
    lines.sort_unstable();
    assert_eq!(lines, vec!["c0", "c1", "c2", "z0", "z1", "z2"]);
}

#[test]
fn estimator_labels_the_time() {
    let out = run(&[
        "--estimator",
        "p90",
        "-r",
        "3",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("p90"), "stdout: {s}");
}

#[test]
fn precision_controls_decimals() {
    let out = run(&[
        "--precision",
        "1",
        "-u",
        "s",
        "-r",
        "2",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.02",
    ]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("0.0 s"), "stdout: {s}");
    assert!(!s.contains("0.020 s"), "stdout: {s}");
}

#[test]
fn invalid_estimator_errors() {
    let out = run(&["--estimator", "avg", "-r", "1", "--no-calibrate", "a"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("invalid estimator"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn raw_with_one_command_errors() {
    let out = run(&["--raw", "-r", "2", "--no-calibrate", "sleep 0.02"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("2 commands"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn reference_out_of_range_errors() {
    let out = run(&["--reference", "5", "-r", "2", "--no-calibrate", "a", "b"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("out of range"),
        "stderr: {}",
        stderr(&out)
    );
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
    assert!(
        stderr(&out).contains("non-zero"),
        "stderr: {}",
        stderr(&out)
    );
}

#[test]
fn failing_run_dumps_partial_results() {
    // Succeeds for run ids 0 and 1, fails on 2; -j 1 makes the abort point
    // deterministic, so exactly 2 measurements are collected before the error.
    let out = run(&[
        "-j",
        "1",
        "-r",
        "5",
        "--no-calibrate",
        "--no-pin",
        "sh -c 'test $MEGAFINE_RUN_ID -lt 2'",
    ]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("non-zero"),
        "stderr: {}",
        stderr(&out)
    );
    let s = stdout(&out);
    assert!(s.contains("Benchmark 1"), "stdout: {s}");
    assert!(s.contains("2 runs"), "stdout: {s}");
}

#[test]
fn failing_run_keeps_raw_stdout_empty() {
    let out = run(&[
        "--raw",
        "-j",
        "1",
        "-r",
        "3",
        "--no-calibrate",
        "--no-pin",
        "sleep 0.01",
        "false",
    ]);
    assert!(!out.status.success());
    assert!(stdout(&out).is_empty(), "stdout: {}", stdout(&out));
}
