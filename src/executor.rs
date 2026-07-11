use std::fs::File;
use std::io::{PipeReader, Read};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::measurement::Execution;
use crate::options::{InputMode, Invocation, Options, OutputMode};
use crate::perf;

/// Largest amount of a failing command's stderr to keep for the error message.
const STDERR_CAP: usize = 8 * 1024;

/// A benchmarked or hook command exited non-zero. Carries the child's exit
/// code so megafine can terminate with the same one.
#[derive(Debug)]
pub struct CommandFailed {
    pub code: i32,
    message: String,
}

impl std::fmt::Display for CommandFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CommandFailed {}

impl Options {
    /// Execute the pre-parsed command and measure its resource usage. stdout is
    /// discarded; stderr is drained as it is produced (so it can never fill the
    /// pipe and block the child) and reported if the command exits non-zero.
    ///
    /// `measured` marks a timing run of a benchmarked command (as opposed to a
    /// hook or calibration run): only those honor `--region` (pipe via
    /// `MEGAFINE_FD`, wall-clock summed across `megafine_start()`/
    /// `megafine_stop()` calls) and `--ignore-failure` (a non-zero exit is
    /// kept as a failed measurement instead of surfacing as an error).
    ///
    /// `run_id`, when set, is exposed to the child as `MEGAFINE_RUN_ID`.
    pub fn execute(
        &self,
        inv: &Invocation,
        measured: bool,
        run_id: Option<u64>,
    ) -> Result<Execution> {
        let region = measured && self.region;
        let command_line = &inv.line;
        let mut command = Command::new(&inv.argv[0]);
        command.args(&inv.argv[1..]).stderr(Stdio::piped());
        // --output/--input only shape the timing runs; hooks keep null stdio.
        match (measured, &self.input) {
            (true, InputMode::File(path)) => {
                let file = File::open(path)
                    .with_context(|| format!("--input: cannot open '{}'", path.display()))?;
                command.stdin(Stdio::from(file));
            }
            _ => {
                command.stdin(Stdio::null());
            }
        }
        match (measured, &self.output) {
            (true, OutputMode::Inherit) => {
                command.stdout(Stdio::inherit());
            }
            (true, OutputMode::File(path)) => {
                let file = File::create(path)
                    .with_context(|| format!("--output: cannot create '{}'", path.display()))?;
                command.stdout(Stdio::from(file));
            }
            _ => {
                command.stdout(Stdio::null());
            }
        }
        if let Some(id) = run_id {
            command.env("MEGAFINE_RUN_ID", id.to_string());
        }

        // Both ends close-on-exec; we re-open the write end in the child below.
        let region_pipe = region
            .then(|| std::io::pipe().context("cannot create a pipe for region mode data exchange"))
            .transpose()?;
        if let Some((reader, writer)) = &region_pipe {
            // Best-effort: events buffer until the run ends, so give the pipe room.
            unsafe { libc::fcntl(reader.as_raw_fd(), libc::F_SETPIPE_SZ, 1 << 20) };
            let write_fd = writer.as_raw_fd();
            command.env("MEGAFINE_FD", write_fd.to_string());
            // Clear CLOEXEC on the write end *only* in this child, so it alone
            // keeps the write side past exec (children forked concurrently by
            // other workers inherit it CLOEXEC and drop it at their own exec).
            unsafe {
                command.pre_exec(move || match libc::fcntl(write_fd, libc::F_SETFD, 0) {
                    -1 => Err(std::io::Error::last_os_error()),
                    _ => Ok(()),
                });
            }
        }

        // A timed-out run is killed as a whole process group (a shell's
        // grandchildren would otherwise survive and keep stderr open), so the
        // child becomes its own group leader.
        let timeout = if measured { self.timeout } else { None };
        if timeout.is_some() {
            unsafe {
                command.pre_exec(|| match libc::setpgid(0, 0) {
                    -1 => Err(std::io::Error::last_os_error()),
                    _ => Ok(()),
                });
            }
        }

        // With --counters the child requests tracing, so the kernel freezes it
        // at the entry of its exec: the parent attaches the counters while
        // zero child instructions have run, then detaches (see perf.rs).
        if self.counters {
            unsafe {
                command.pre_exec(|| match libc::ptrace(libc::PTRACE_TRACEME, 0, 0, 0) {
                    -1 => Err(std::io::Error::last_os_error()),
                    _ => Ok(()),
                });
            }
        }

        let start = Instant::now();
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn command '{command_line}'"))?;
        // Drop the parent's write end so the read side EOFs at child exit.
        let region_reader = region_pipe.map(|(reader, _writer)| reader);
        // Attach to the exec-stopped child, then release it. The freeze is
        // constant overhead on the wall clock, absorbed by calibration. On
        // failure the child is killed and reaped rather than run unmeasured.
        let counters = if self.counters {
            let attach = |pid: i32| -> std::io::Result<perf::Counters> {
                let mut status = 0;
                if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if !libc::WIFSTOPPED(status) {
                    return Err(std::io::Error::other("child not stopped at exec"));
                }
                let counters = perf::attach(pid)?;
                // Detach with no signal: the exec SIGTRAP is suppressed.
                if unsafe { libc::ptrace(libc::PTRACE_DETACH, pid, 0, 0) } == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(counters)
            };
            match attach(child.id() as i32) {
                Ok(counters) => Some(counters),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(e).with_context(|| {
                        format!("failed to attach perf counters to '{command_line}'")
                    });
                }
            }
        } else {
            None
        };
        // The stderr drain below blocks until the child tree exits, so the
        // timeout is enforced from a thread polling a pidfd: on expiry it
        // SIGKILLs the process group. The group leader cannot be reaped
        // before our wait4 (the drain runs first), so the pgid cannot be
        // recycled — no pid-reuse race.
        let timed_out = Arc::new(AtomicBool::new(false));
        let killer = timeout
            .map(|limit| {
                let pid = child.id() as i32;
                let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as libc::c_int;
                if pidfd < 0 {
                    let e = std::io::Error::last_os_error();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(anyhow::Error::new(e)
                        .context(format!("pidfd_open failed for command '{command_line}'")));
                }
                let deadline = start + limit;
                let flag = Arc::clone(&timed_out);
                Ok(std::thread::spawn(move || {
                    let mut fds = libc::pollfd {
                        fd: pidfd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    loop {
                        let remaining = deadline.saturating_duration_since(Instant::now());
                        let ms = remaining.as_millis().min(i32::MAX as u128) as i32;
                        match unsafe { libc::poll(&mut fds, 1, ms) } {
                            0 => {
                                flag.store(true, Ordering::Relaxed);
                                unsafe { libc::kill(-pid, libc::SIGKILL) };
                            }
                            -1 if std::io::Error::last_os_error().kind()
                                == std::io::ErrorKind::Interrupted =>
                            {
                                continue;
                            }
                            _ => {} // the child exited, nothing to kill
                        }
                        break;
                    }
                    unsafe { libc::close(pidfd) };
                }))
            })
            .transpose()?;

        // Reading to EOF drains the pipe and blocks until the child closes
        // stderr (≈ its exit), so wait4 then reaps immediately.
        let captured = child.stderr.take().map(drain_capped).unwrap_or_default();
        // Region events buffered while stderr drained; collect them now.
        let region_window = region_reader.map(read_region_window);
        let (status, mut exec) = wait4(&child)
            .with_context(|| format!("failed to wait for command '{command_line}'"))?;
        exec.wall_clock = start.elapsed().as_secs_f64();
        // Returns immediately: the pidfd is readable once the child terminated.
        if let Some(handle) = killer {
            let _ = handle.join();
        }

        if !status.success() {
            if measured && self.ignore_failure {
                exec.failed = true;
            } else {
                let stderr = String::from_utf8_lossy(&captured);
                let stderr = stderr.trim_end();
                let mut message = if timed_out.load(Ordering::Relaxed) {
                    format!(
                        "command '{command_line}' timed out after {}s (killed)",
                        timeout
                            .expect("timed_out is only set by the killer thread")
                            .as_secs_f64()
                    )
                } else {
                    format!(
                        "command '{command_line}' terminated with a non-zero exit code ({status})"
                    )
                };
                if !stderr.is_empty() {
                    message.push_str(&format!("\n--- stderr ---\n{stderr}"));
                }
                // Shell convention for signal deaths: 128 + signal number.
                let code = status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
                return Err(CommandFailed { code, message }.into());
            }
        }

        if let Some(counters) = counters {
            exec.counters = Some(counters.read().with_context(|| {
                format!("failed to read the perf counters of '{command_line}'")
            })?);
        }

        if let Some(window) = region_window {
            exec.wall_clock = window.with_context(|| {
                format!(
                    "command '{command_line}' produced no region events : is-it instrumented \
                     (megafine_start/stop) and built with instrument/megafine.[h|rs]?"
                )
            })?;
        }

        Ok(exec)
    }
}

/// The CPUs this process is currently allowed to run on, as sorted core ids.
/// Partitioning *this* set (rather than every online CPU) makes pinning compose
/// with an outer `taskset`/cpuset and skips offline cores.
pub fn allowed_cpus() -> Result<Vec<usize>> {
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::cpu_set_t>();
    if unsafe { libc::sched_getaffinity(0, size, &mut set) } < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to read CPU affinity");
    }
    Ok((0..libc::CPU_SETSIZE as usize)
        .filter(|&c| unsafe { libc::CPU_ISSET(c, &set) })
        .collect())
}

/// Split `cpus` into `jobs` contiguous groups of equal size, so every worker has
/// the same CPU budget and timings stay comparable. `reserve` CPUs are booked
/// upfront, plus any leftover (when the rest isn't a multiple of `jobs`); both
/// are set aside from the low end, since CPU 0 tends to carry OS/IRQ work, and
/// returned first so orchestration threads can be parked there. Callers
/// guarantee `jobs <= cpus.len() - reserve`, so every group is non-empty.
pub fn partition(cpus: &[usize], jobs: usize, reserve: usize) -> (Vec<usize>, Vec<Vec<usize>>) {
    let per_worker = (cpus.len() - reserve) / jobs;
    let leftover = reserve + (cpus.len() - reserve) % jobs;
    let groups = cpus[leftover..]
        .chunks_exact(per_worker)
        .map(<[usize]>::to_vec)
        .collect();
    (cpus[..leftover].to_vec(), groups)
}

pub fn pin_thread(cpus: &[usize]) -> Result<()> {
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for &c in cpus {
        unsafe { libc::CPU_SET(c, &mut set) };
    }
    let size = std::mem::size_of::<libc::cpu_set_t>();
    if unsafe { libc::sched_setaffinity(0, size, &set) } < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to set thread CPU affinity");
    }
    Ok(())
}

/// Read all 9-byte `[tag:u8][ns:u64]` events from `reader` and return
/// the summed `stop - start` window in seconds, or `None` if no complete pair.
/// TODO: add sub identifier to allow multiple slots
fn read_region_window(mut reader: PipeReader) -> Option<f64> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).ok()?;

    let mut pending: Option<u64> = None;
    let mut total_ns: u64 = 0;
    let mut matched = false;
    for rec in bytes.chunks_exact(9) {
        let ns = u64::from_ne_bytes(rec[1..9].try_into().unwrap());
        match rec[0] {
            0 => pending = Some(ns),
            1 => {
                if let Some(start) = pending.take() {
                    total_ns += ns.saturating_sub(start);
                    matched = true;
                }
            }
            _ => {}
        }
    }
    matched.then(|| total_ns as f64 / 1e9)
}

/// Read `pipe` to EOF, keeping at most `STDERR_CAP` bytes but draining the rest.
fn drain_capped(mut pipe: ChildStderr) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 8192];
    while let Ok(n) = pipe.read(&mut chunk) {
        if n == 0 {
            break;
        }
        if kept.len() < STDERR_CAP {
            let take = n.min(STDERR_CAP - kept.len());
            kept.extend_from_slice(&chunk[..take]);
        }
    }
    kept
}

fn timeval_to_second(tv: libc::timeval) -> f64 {
    tv.tv_sec as f64 + tv.tv_usec as f64 / 1_000_000.0
}

/// Reap `child` via `wait4(2)` to obtain its exit status and rusage. The
/// `time_wall_clock` is left at 0.0 for the caller to fill, since the timing
/// window belongs to `execute`, not the reap.
fn wait4(child: &Child) -> Result<(ExitStatus, Execution)> {
    let pid = child.id() as i32;
    let mut status = 0;
    let mut rusage = MaybeUninit::zeroed();

    let result = unsafe { libc::wait4(pid, &mut status, 0, rusage.as_mut_ptr()) };
    let err = std::io::Error::last_os_error();
    let errno = err.raw_os_error().unwrap_or(-1);
    (result >= 0).then_some(()).ok_or(err).with_context(|| {
        format!("wait4(2) on pid {pid} failed: returned {result}, errno {errno}")
    })?;

    let rusage = unsafe { rusage.assume_init() };
    Ok((
        ExitStatus::from_raw(status),
        Execution {
            wall_clock: 0.0,
            time_user: timeval_to_second(rusage.ru_utime),
            time_system: timeval_to_second(rusage.ru_stime),
            // ru_maxrss is reported in KiB on Linux.
            max_rss: (rusage.ru_maxrss.max(0) as u64) * 1024,
            major_faults: rusage.ru_majflt.max(0) as u64,
            minor_faults: rusage.ru_minflt.max(0) as u64,
            vol_ctx_switches: rusage.ru_nvcsw.max(0) as u64,
            invol_ctx_switches: rusage.ru_nivcsw.max(0) as u64,
            counters: None,
            failed: false,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_even_split() {
        let (reserved, groups) = partition(&[0, 1, 2, 3], 2, 0);
        assert_eq!(reserved, vec![]);
        assert_eq!(groups, vec![vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn partition_natural_leftover() {
        // 5 CPUs over 2 jobs: the low CPU is set aside.
        let (reserved, groups) = partition(&[0, 1, 2, 3, 4], 2, 0);
        assert_eq!(reserved, vec![0]);
        assert_eq!(groups, vec![vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn partition_with_reserve() {
        let (reserved, groups) = partition(&[0, 1, 2, 3], 2, 2);
        assert_eq!(reserved, vec![0, 1]);
        assert_eq!(groups, vec![vec![2], vec![3]]);
    }

    #[test]
    fn partition_reserve_plus_leftover() {
        // 5 CPUs, reserve 1: 4 remain → 2 per worker, no extra leftover.
        let (reserved, groups) = partition(&[0, 1, 2, 3, 4], 2, 1);
        assert_eq!(reserved, vec![0]);
        assert_eq!(groups, vec![vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn partition_groups_are_equal_sized() {
        let (_, groups) = partition(&[0, 1, 2, 3, 4, 5, 6], 3, 0);
        assert!(groups.iter().all(|g| g.len() == groups[0].len()));
    }
}
