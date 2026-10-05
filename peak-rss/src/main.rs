//! `peak-rss <program> [args…]`: run a command and report its peak resident
//! set, for `scripts/measure.py`'s figures that read one.
//!
//! It spawns the command with every stream inherited, waits for it, and writes
//! one `maxrss_kib=<n>` line to stderr: `ru_maxrss` out of
//! `getrusage(RUSAGE_CHILDREN)`, which on Linux is kibibytes. A command a
//! signal ended adds a `signal=<n>` line beside it. It exits with the
//! command's own code, or `128 + n` for a death by signal `n`, the shell's
//! convention — so a kill stays a kill to whatever reads the exit status. The
//! cgroup's `memory.events` still says whether the kill was the OOM reaper's,
//! which no exit status can (`measure.OOM_ORACLE`). A command that cannot be
//! started exits 127 where it was not found and 126 otherwise, writing no
//! `maxrss_kib` line, since nothing ran.
//!
//! **Why a wrapper at all.** A timer around the container runtime reports the
//! client's peak rather than the command's, the timer has to sit inside the
//! container anyway, and bash's `time` reports no memory. Built static, it
//! runs in any image with nothing of the image's — no interpreter, no
//! `/usr/bin/time` — so moving the harness's image cannot break it.
//!
//! **Why `getrusage` rather than polling `/proc`.** `VmHWM` is the same
//! kernel-maintained high-water mark, but it is gone the instant the process
//! becomes a zombie, so a poller's last read is whatever it managed before the
//! end of the run — and a `parse` writes its cache last, which is exactly
//! where a late peak would sit. `RUSAGE_CHILDREN` is read after the wait and
//! cannot miss it.
//!
//! **What the reading carries of this process.** At `exec` the kernel folds
//! the outgoing address space's high-water mark into the child's `maxrss`, so
//! the spawning side's resident set can stand in a reading — that is read off
//! `exec_mmap`, not checked against a probe. A static binary keeps that share
//! small, and the harness's preflight runs this around `/bin/true` in the
//! pinned image, which bounds it: the floor any reading stands on.

use std::ffi::OsString;
use std::io::{self, ErrorKind};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitCode, ExitStatus};

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(program) = args.next() else {
        eprintln!("usage: peak-rss <program> [args...]");
        return ExitCode::from(2);
    };
    let rest: Vec<OsString> = args.collect();
    let status = match Command::new(&program).args(&rest).status() {
        Ok(status) => status,
        Err(err) => {
            eprintln!("peak-rss: {}: {err}", program.to_string_lossy());
            return ExitCode::from(if err.kind() == ErrorKind::NotFound { 127 } else { 126 });
        }
    };
    match children_maxrss_kib() {
        Ok(kib) => eprintln!("maxrss_kib={kib}"),
        Err(err) => {
            eprintln!("peak-rss: getrusage: {err}");
            return ExitCode::from(1);
        }
    }
    if let Some(signal) = status.signal() {
        eprintln!("signal={signal}");
    }
    ExitCode::from(exit_code(status))
}

/// The largest resident set any waited-for child reached, in kibibytes.
fn children_maxrss_kib() -> io::Result<libc::c_long> {
    // SAFETY: `getrusage` writes one `struct rusage` through the pointer and
    // reads nothing from it; a zeroed `rusage` is a valid value of it.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, &mut usage) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(usage.ru_maxrss)
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
