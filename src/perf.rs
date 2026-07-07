//! Hardware performance counters for spawned commands, via perf_event_open(2).
//!
//! The child calls `PTRACE_TRACEME` in `pre_exec`, so the kernel stops it at
//! the very entry of its exec — after `spawn()` has returned (the exec closes
//! std's CLOEXEC status pipe) but before the first instruction. The parent
//! attaches the counters to the frozen child, detaches to let it run, and
//! reads them after `wait4` (perf fds outlive the child). Exec-exact and
//! race-free even for sub-ms commands. (Parking the child in `pre_exec`
//! instead would deadlock: `spawn()` itself blocks until the exec; and
//! attaching without freezing races the exit of fast commands.)

use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use anyhow::{Context, Result};

/// Hardware counters of one command execution: user space only, whole child
/// tree, corrected for kernel multiplexing.
#[derive(Clone, Copy)]
pub struct PerfCounts {
    pub instructions: u64,
    pub cycles: u64,
    pub cache_misses: u64,
    pub branch_misses: u64,
}

/// `perf_event_attr`, trimmed to PERF_ATTR_SIZE_VER1 (72 bytes): the kernel
/// only reads `size` bytes, and everything megafine needs is in this prefix.
#[repr(C)]
#[derive(Default)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup_events: u32,
    bp_type: u32,
    bp_addr: u64,
    bp_len: u64,
}

// attr.flags bits, in linux/perf_event.h bitfield order.
const INHERIT: u64 = 1 << 1;
const EXCLUDE_KERNEL: u64 = 1 << 5;
const EXCLUDE_HV: u64 = 1 << 6;

// PERF_TYPE_HARDWARE event ids (linux/perf_event.h).
const HW_CPU_CYCLES: u64 = 0;
const HW_INSTRUCTIONS: u64 = 1;
const HW_CACHE_MISSES: u64 = 3;
const HW_BRANCH_MISSES: u64 = 5;
const EVENTS: [u64; 4] = [
    HW_INSTRUCTIONS,
    HW_CPU_CYCLES,
    HW_CACHE_MISSES,
    HW_BRANCH_MISSES,
];

const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 1 << 3;
// read() returns [value, time_enabled, time_running]: the times undo the
// scaling the kernel applies when it multiplexes more counters than the PMU
// has slots for.
const FORMAT_TOTAL_TIME_ENABLED: u64 = 1 << 0;
const FORMAT_TOTAL_TIME_RUNNING: u64 = 1 << 1;

fn open(config: u64, pid: i32) -> io::Result<OwnedFd> {
    let attr = PerfEventAttr {
        type_: 0, // PERF_TYPE_HARDWARE
        size: mem::size_of::<PerfEventAttr>() as u32,
        config,
        read_format: FORMAT_TOTAL_TIME_ENABLED | FORMAT_TOTAL_TIME_RUNNING,
        // User space only, so the default perf_event_paranoid = 2 suffices.
        // Counting starts at open (no `disabled` bit).
        flags: INHERIT | EXCLUDE_KERNEL | EXCLUDE_HV,
        ..Default::default()
    };
    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr,
            pid,
            -1 as libc::c_int, // any CPU
            -1 as libc::c_int, // no group
            PERF_FLAG_FD_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

/// Fail-fast permission/PMU check on the current process, run at startup so a
/// forbidden configuration errors before any benchmark starts, diagnosing the
/// usual culprit (kernel.perf_event_paranoid).
pub fn probe() -> Result<()> {
    let Err(e) = open(HW_INSTRUCTIONS, 0) else {
        return Ok(());
    };
    let paranoid = std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok());
    let hint = match (e.kind(), paranoid) {
        (io::ErrorKind::PermissionDenied, Some(p)) if p > 2 => format!(
            "kernel.perf_event_paranoid is {p}, which blocks unprivileged perf events; \
             run `sudo sysctl kernel.perf_event_paranoid=2`"
        ),
        (io::ErrorKind::PermissionDenied, _) => {
            "perf events are denied even though kernel.perf_event_paranoid allows them \
             (container seccomp profile or LSM policy?)"
                .to_string()
        }
        (io::ErrorKind::NotFound, _) => {
            "the hardware 'instructions' event is unsupported: no PMU is available \
             (VM without PMU passthrough?)"
                .to_string()
        }
        _ => "perf_event_open(2) failed".to_string(),
    };
    Err(e).context(format!("--counters is unavailable: {hint}"))
}

/// The counters attached to one child, counting since `attach`.
pub struct Counters([OwnedFd; 4]);

/// Open every counter on the freshly spawned `pid`.
pub fn attach(pid: i32) -> io::Result<Counters> {
    let mut fds = Vec::with_capacity(EVENTS.len());
    for event in EVENTS {
        fds.push(open(event, pid)?);
    }
    Ok(Counters(fds.try_into().expect("one fd per event")))
}

impl Counters {
    /// Read the counters, after the child exited.
    pub fn read(&self) -> io::Result<PerfCounts> {
        let mut values = [0u64; 4];
        for (fd, out) in self.0.iter().zip(&mut values) {
            let mut buf = [0u64; 3]; // value, time_enabled, time_running
            let n = unsafe {
                libc::read(
                    fd.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    mem::size_of_val(&buf),
                )
            };
            if n != mem::size_of_val(&buf) as isize {
                return Err(io::Error::last_os_error());
            }
            *out = if buf[2] > 0 {
                (buf[0] as f64 * buf[1] as f64 / buf[2] as f64) as u64
            } else {
                buf[0]
            };
        }
        Ok(PerfCounts {
            instructions: values[0],
            cycles: values[1],
            cache_misses: values[2],
            branch_misses: values[3],
        })
    }
}
