//! Ending as a PID namespace's init on every signal that ends the process
//! anywhere else, but a fault (`docs/design/decisions.md`, "D26").
//!
//! The kernel discards any signal an init has left at its default action,
//! whoever sends it (`docs/design/runtime-invariants.md`, "RT19"), and
//! `docker run image pgdt …` makes the binary its container's init without
//! being asked. So there, and only there, each such signal gets a handler that
//! `_exit`s `128 + n` — what a container runtime reports for the signal either
//! way. Elsewhere nothing is installed and every disposition is the default.
//!
//! `pgdt` and `datafusion-cli-pgdump`, whose `run` is `pgdt sql`, both depend
//! on it; the library an embedder links installs no handler.

use std::io;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use signal_hook::SigId;

/// What a binary passes to [`InitShutdown`], named without its own `libc`.
pub use libc::{SIGINT, SIGTERM, c_int};

#[cfg(feature = "test-support")]
pub mod unshare;

/// Whether this process is its PID namespace's init, which the kernel never
/// lets a default-action signal end (`docs/design/runtime-invariants.md`,
/// "RT19").
pub fn namespace_init() -> bool {
    std::process::id() == 1
}

/// Every signal whose default action ends a process, with or without a core,
/// less those no handler here could change:
///
/// - `SIGKILL` and `SIGSTOP`, which nothing catches;
/// - `SIGPIPE`, which the Rust runtime ignores before `main`, so it ends
///   neither binary anywhere;
/// - the six a fault raises, `SIGILL`, `SIGTRAP`, `SIGBUS`, `SIGFPE`,
///   `SIGSEGV` and `SIGSYS`: the kernel forces a fault's signal under the
///   default action, which ends an init too, with its core (RT19), where a
///   handler here would `_exit` in its place.
///
/// Linux only: a PID namespace is Linux's, so elsewhere the list is empty.
fn ending_signals() -> Vec<libc::c_int> {
    #[cfg(target_os = "linux")]
    {
        use libc::*;
        [
            SIGHUP, SIGINT, SIGQUIT, SIGABRT, SIGUSR1, SIGUSR2, SIGALRM, SIGTERM, SIGSTKFLT,
            SIGXCPU, SIGXFSZ, SIGVTALRM, SIGPROF, SIGIO, SIGPWR,
        ]
        .into_iter()
        .chain(SIGRTMIN()..=SIGRTMAX())
        .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// The handlers [`InitShutdown::install`] registered, by signal — empty
/// unless this process is its namespace's init.
pub struct InitShutdown {
    handlers: Vec<(libc::c_int, SigId)>,
}

impl InitShutdown {
    /// As its namespace's init, register an `_exit(128 + n)` on every signal
    /// that ends the process elsewhere, a fault's aside, but those in
    /// `caught`, which the caller answers itself for the whole run; otherwise
    /// register nothing.
    ///
    /// `signal-hook` runs every action registered on a signal, so one the
    /// caller catches must be left out here rather than registered beside its
    /// own handler, which it would pre-empt.
    pub fn install(caught: &[libc::c_int]) -> io::Result<Self> {
        let mut handlers = Vec::new();
        if namespace_init() {
            let always = Arc::new(AtomicBool::new(true));
            for signal in ending_signals().into_iter().filter(|s| !caught.contains(s)) {
                let id = signal_hook::flag::register_conditional_shutdown(
                    signal,
                    128 + signal,
                    Arc::clone(&always),
                )?;
                handlers.push((signal, id));
            }
        }
        Ok(Self { handlers })
    }

    /// Hand `signals` to handlers the caller has **just** registered, from
    /// here to the end of the process. A signal landing between the two
    /// registrations runs both, this one's first, it being the older, so it
    /// exits as a signal a moment earlier would have.
    pub fn release(&mut self, signals: &[libc::c_int]) {
        self.handlers.retain(|&(signal, id)| {
            let held = !signals.contains(&signal);
            if !held {
                signal_hook::low_level::unregister(id);
            }
            held
        });
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// **A fault's signal, and one nothing can handle, is left to the
    /// kernel**; the rest of the ending set is each handled once.
    #[test]
    fn the_handled_signals_leave_the_faults_to_the_kernel() {
        use libc::*;
        let handled = ending_signals();
        for left in [SIGILL, SIGTRAP, SIGBUS, SIGFPE, SIGSEGV, SIGSYS, SIGKILL, SIGSTOP, SIGPIPE] {
            assert!(!handled.contains(&left), "{left}");
        }
        for ending in [SIGHUP, SIGINT, SIGQUIT, SIGABRT, SIGTERM, SIGRTMIN(), SIGRTMAX()] {
            assert!(handled.contains(&ending), "{ending}");
        }
        let mut unique = handled.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), handled.len());
    }
}
