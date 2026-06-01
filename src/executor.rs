use std::fs::File;
use std::io::Read;
use std::mem::MaybeUninit;
use std::os::fd::FromRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, ChildStderr, Command, ExitStatus, Stdio};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use crate::measurement::Execution;
use crate::options::Options;

/// Largest amount of a failing command's stderr to keep for the error message.
const STDERR_CAP: usize = 8 * 1024;

impl Options {
    fn build(&self, command_line: &str) -> Result<Command> {
        match &self.shell {
            None => {
                let parts = shell_words::split(command_line)
                    .with_context(|| format!("could not parse command '{command_line}'"))?;
                let (program, args) = parts
                    .split_first()
                    .with_context(|| format!("empty command '{command_line}'"))?;
                let mut command = Command::new(program);
                command.args(args);
                Ok(command)
            }
            Some(path) => {
                let mut command = Command::new(path);
                command.arg("-c").arg(command_line);
                Ok(command)
            }
        }
    }

    /// Execute the command line and measure its resource usage. stdout is
    /// discarded; stderr is drained as it is produced (so it can never fill the
    /// pipe and block the child) and reported if the command exits non-zero.
    ///
    /// With `region`, hand the child a pipe via `MEGAFINE_FD` and report the
    /// wall-clock summed across its `megafine_start()`/`megafine_stop()` calls
    /// instead of the whole-process elapsed time.
    pub fn execute(&self, command_line: &str, region: bool) -> Result<Execution> {
        let mut command = self.build(command_line)?;
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        // Both ends close-on-exec; we re-open the write end in the child below.
        let region_pipe = region
            .then(|| pipe_cloexec().context("cannot create a pipe for region mode data exchange"))
            .transpose()?;
        if let Some((_, write_fd)) = region_pipe {
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

        let start = Instant::now();
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to spawn command '{command_line}'"))?;
        // Drop the parent's write end so the read side EOFs at child exit.
        if let Some((_, write_fd)) = region_pipe {
            unsafe { libc::close(write_fd) };
        }
        // Reading to EOF drains the pipe and blocks until the child closes
        // stderr (≈ its exit), so wait4 then reaps immediately.
        let captured = child.stderr.take().map(drain_capped).unwrap_or_default();
        // Region events buffered while stderr drained; collect them now.
        let region_window = region_pipe.map(|(read_fd, _)| read_region_window(read_fd));
        let mut exec = wait4(&child)
            .with_context(|| format!("failed to wait for command '{command_line}'"))?;
        exec.wall_clock = start.elapsed().as_secs_f64();

        if !exec.status.success() {
            let stderr = String::from_utf8_lossy(&captured);
            let stderr = stderr.trim_end();
            if stderr.is_empty() {
                bail!(
                    "command '{command_line}' terminated with a non-zero exit code ({})",
                    exec.status
                );
            }
            bail!(
                "command '{command_line}' terminated with a non-zero exit code ({})\n--- stderr ---\n{stderr}",
                exec.status
            );
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
/// the same CPU budget and timings stay comparable. Any leftover CPUs (when
/// `cpus.len()` isn't a multiple of `jobs`) are set aside from the low end, since
/// CPU 0 tends to carry OS/IRQ work. Callers guarantee `jobs <= cpus.len()`, so
/// every group is non-empty.
pub fn partition(cpus: &[usize], jobs: usize) -> Vec<Vec<usize>> {
    let per_worker = cpus.len() / jobs;
    let leftover = cpus.len() % jobs;
    cpus[leftover..]
        .chunks_exact(per_worker)
        .map(<[usize]>::to_vec)
        .collect()
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

/// Create a pipe with both ends close-on-exec, enlarged for region-event headroom.
/// Returns `(read_fd, write_fd)`.
fn pipe_cloexec() -> Result<(i32, i32)> {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to create region pipe");
    }
    // Best-effort: events buffer until the run ends, so give the pipe room.
    unsafe { libc::fcntl(fds[0], libc::F_SETPIPE_SZ, 1 << 20) };
    Ok((fds[0], fds[1]))
}

/// Read all 9-byte `[tag:u8][ns:u64]` events from `read_fd` (closing it) and return
/// the summed `stop - start` window in seconds, or `None` if no complete pair.
/// TODO: add sub identifier to allow multiple slots
fn read_region_window(read_fd: i32) -> Option<f64> {
    let mut bytes = Vec::new();
    // SAFETY: we own `read_fd`; File closes it on drop.
    unsafe { File::from_raw_fd(read_fd) }
        .read_to_end(&mut bytes)
        .ok()?;

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
fn wait4(child: &Child) -> Result<Execution> {
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
    Ok(Execution {
        status: ExitStatus::from_raw(status),
        wall_clock: 0.0,
        time_user: timeval_to_second(rusage.ru_utime),
        time_system: timeval_to_second(rusage.ru_stime),
        // ru_maxrss is reported in KiB on Linux.
        max_rss: (rusage.ru_maxrss.max(0) as u64) * 1024,
    })
}
