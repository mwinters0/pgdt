//! `count-instructions <program> [args…]`: run a command and report the
//! user-space instructions it retired — `perf`'s `instructions:u` — for
//! `scripts/measure.py`'s `release-profile` figure, which reads it beside the
//! wall clock to tell lost inlining from code layout (`docs/design/roadmap.md`,
//! "A downstream build is within 3% of ours").
//!
//! It opens one hardware counter on itself before spawning the command, and
//! after waiting writes one `instructions_u=<n>` line to stderr. The command's
//! streams are inherited, and it exits with the command's own code, or
//! `128 + n` for a death by signal `n`, as `peak-rss` does — a death adding a
//! `signal=<n>` line. A count the kernel multiplexed, running for less of the
//! command's life than it was enabled, is not scaled: it is reported as
//! `instructions_multiplexed=<running_ns>/<enabled_ns>` in place of the count,
//! and the harness refuses the rep. A counter that cannot be opened exits 1
//! with neither line, before anything runs, and one that cannot be read, or
//! that the command's `exec` never enabled, exits 1 after it; a command that cannot be started
//! exits 127 where it was not found and 126 otherwise, writing no count.
//!
//! **What is counted is the command and every thread and process it starts,
//! from its `exec` to its exit, and nothing of this process.** The counter is
//! opened disabled, `inherit`ed by every task the command clones and enabled
//! by the kernel at the command's own `exec` (`enable_on_exec`), so the
//! spawning side's instructions — before and after the `exec` — stand in no
//! reading. A child's count is folded into the counter this process reads when
//! that child exits, and the wait returns once the command has: a process it
//! leaves running past its exit is not counted. Kernel and hypervisor instructions are excluded: a codegen
//! change moves user space, and a kernel path is the kernel's.
//!
//! **Why a wrapper rather than `perf stat`.** `perf` is the host's, tied to its
//! kernel and linked against its distribution's libraries, and the figure runs
//! in the pinned image; built static, this needs nothing of either, as
//! `peak-rss` does not. Counting needs no sampling, so the timed run it wraps
//! pays a counter save on each context switch and nothing per instruction.
//!
//! The container must let the call through: the runtime's default seccomp
//! profile refuses `perf_event_open` without `CAP_PERFMON`, which the harness
//! grants the counted legs alone (`measure.COUNT_INSTRUCTIONS_CAPS`).

use std::ffi::OsString;
use std::io::{self, ErrorKind};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitCode, ExitStatus};

/// `PERF_TYPE_HARDWARE`.
const PERF_TYPE_HARDWARE: u32 = 0;
/// `PERF_COUNT_HW_INSTRUCTIONS`.
const PERF_COUNT_HW_INSTRUCTIONS: u64 = 1;
/// `PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING`: the read
/// is the count, then the nanoseconds it was enabled and running.
const READ_FORMAT: u64 = 1 | 2;
/// `PERF_FLAG_FD_CLOEXEC`, so the command does not inherit the descriptor.
const PERF_FLAG_FD_CLOEXEC: libc::c_ulong = 8;

/// `struct perf_event_attr` at `PERF_ATTR_SIZE_VER0`, the 64 bytes every
/// kernel since 2.6.31 reads, which holds every field set here.
#[repr(C)]
#[derive(Default)]
struct PerfEventAttr {
    kind: u32,
    size: u32,
    config: u64,
    sample_period: u64,
    sample_type: u64,
    read_format: u64,
    /// The bitfield after `read_format`, by bit position.
    flags: u64,
    wakeup_events: u32,
    bp_type: u32,
    config1: u64,
}

const DISABLED: u64 = 1 << 0;
const INHERIT: u64 = 1 << 1;
const EXCLUDE_KERNEL: u64 = 1 << 5;
const EXCLUDE_HV: u64 = 1 << 6;
const ENABLE_ON_EXEC: u64 = 1 << 12;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: count-instructions <program> [args...]");
        return ExitCode::from(2);
    };
    let rest: Vec<OsString> = args.collect();
    let counter = match open_counter() {
        Ok(fd) => fd,
        Err(err) => {
            eprintln!("count-instructions: perf_event_open: {err}");
            return ExitCode::from(1);
        }
    };
    let status = match Command::new(&program).args(&rest).status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!("count-instructions: {}: {err}", program.to_string_lossy());
            return ExitCode::from(if err.kind() == ErrorKind::NotFound { 127 } else { 126 });
        }
    };
    match read_counter(counter) {
        // A counter the `exec` never enabled counted nothing, which a zero
        // would report as a reading.
        Ok(Reading { enabled: 0, .. }) => {
            eprintln!("count-instructions: the counter was never enabled");
            return ExitCode::from(1);
        }
        Ok(Reading { count, enabled, running }) if running == enabled => {
            eprintln!("instructions_u={count}");
        }
        Ok(Reading { enabled, running, .. }) => {
            eprintln!("instructions_multiplexed={running}/{enabled}");
        }
        Err(err) => {
            eprintln!("count-instructions: read: {err}");
            return ExitCode::from(1);
        }
    }
    if let Some(signal) = status.signal() {
        eprintln!("signal={signal}");
    }
    ExitCode::from(exit_code(status))
}

/// The counter, on this process and inherited by every task it clones,
/// enabled at an inheriting task's `exec`.
fn open_counter() -> io::Result<libc::c_int> {
    let attr = PerfEventAttr {
        kind: PERF_TYPE_HARDWARE,
        size: std::mem::size_of::<PerfEventAttr>() as u32,
        config: PERF_COUNT_HW_INSTRUCTIONS,
        read_format: READ_FORMAT,
        flags: DISABLED | INHERIT | EXCLUDE_KERNEL | EXCLUDE_HV | ENABLE_ON_EXEC,
        ..PerfEventAttr::default()
    };
    // SAFETY: the kernel reads `attr.size` bytes of a live, initialised
    // `perf_event_attr`; the other arguments are plain integers.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_perf_event_open,
            &attr as *const PerfEventAttr,
            0 as libc::pid_t,
            -1 as libc::c_int,
            -1 as libc::c_int,
            PERF_FLAG_FD_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd as libc::c_int)
}

/// One read of the counter in `READ_FORMAT`.
struct Reading {
    count: u64,
    enabled: u64,
    running: u64,
}

fn read_counter(fd: libc::c_int) -> io::Result<Reading> {
    let mut words = [0u64; 3];
    // SAFETY: `words` is 24 writable bytes, which is what `READ_FORMAT`'s
    // read returns for one counter outside a group.
    let n = unsafe { libc::read(fd, words.as_mut_ptr().cast(), std::mem::size_of_val(&words)) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize != std::mem::size_of_val(&words) {
        return Err(io::Error::new(ErrorKind::UnexpectedEof, format!("{n} bytes of 24")));
    }
    // SAFETY: `fd` is the descriptor `open_counter` returned, read once above.
    unsafe { libc::close(fd) };
    Ok(Reading { count: words[0], enabled: words[1], running: words[2] })
}

/// The command's status as an exit code: its own, or `128 + n` for a death by
/// signal `n`.
fn exit_code(status: ExitStatus) -> u8 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code as u8,
        (None, Some(signal)) => (128 + signal) as u8,
        // Neither is a stopped or continued child, which a blocking wait on a
        // child not being traced does not return.
        (None, None) => 1,
    }
}
