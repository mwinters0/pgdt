//! What `SIGINT` and `SIGTERM` do to a `parse`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use anyhow::{Context, Result};
use namespace_init::{InitShutdown, namespace_init};
use pgdt_io::Cancellation;

use crate::introspect;

/// Catch `SIGINT` and `SIGTERM` for the duration of a scan, so an interrupted
/// `pgdt parse` saves what it has instead of throwing it away
/// (`docs/design/decisions.md`, "D63"). The guard is **cooperative**: the
/// signal cancels the scan through a `tokio` task, the mapping loop reads
/// that cancellation, and the loop persists the index it owns before
/// returning.
///
/// Three actions run **in the handler itself**, in this order, for each
/// signal: its number is stored in the returned [`InterruptGuard::signalled`];
/// if [`InterruptGuard::armed`] is set, the signal's default action ends the
/// process; then `armed` is set. So a **second** signal, of either kind, ends
/// the process at once — the cache's save writes synchronously on this thread, so
/// nothing a task does could cut a wedged save short — and so does any signal
/// once the caller arms the guard as the scan returns: the cache is saved by
/// then, and a listing blocked on a pipe nobody drains cannot hold the
/// process. Only the cancellation is left to a `tokio` task, the one action
/// that is not async-signal-safe (`docs/design/decisions.md`, "D26").
///
/// The number is stored **before** `armed` is read, and the caller sets
/// `armed` before reading the number, so a signal racing the arming is either
/// ended by its handler or seen by the caller's check.
///
/// As a PID namespace's init — `pgdt` `exec`'d in a container — the kernel
/// discards any signal left at its default action, so there the handler
/// `_exit`s with `128 + n` instead, the status [`die_by`] exits with there,
/// and `init`'s own handlers on both signals are released to these.
pub(crate) fn install_interrupt_guard(
    cancel: Arc<Cancellation>,
    init: &mut InitShutdown,
) -> Result<InterruptGuard> {
    use tokio::signal::unix::{SignalKind, signal};

    let guard = InterruptGuard {
        signalled: Arc::new(AtomicUsize::new(0)),
        armed: Arc::new(AtomicBool::new(false)),
    };
    let kinds = [SignalKind::interrupt(), SignalKind::terminate()];
    for kind in kinds {
        let number = kind.as_raw_value();
        let installing = || format!("installing handler for {number}");
        signal_hook::flag::register_usize(number, Arc::clone(&guard.signalled), number as usize)
            .with_context(installing)?;
        let armed = Arc::clone(&guard.armed);
        if namespace_init() {
            signal_hook::flag::register_conditional_shutdown(number, 128 + number, armed)
        } else {
            signal_hook::flag::register_conditional_default(number, armed)
        }
        .with_context(installing)?;
        signal_hook::flag::register(number, Arc::clone(&guard.armed)).with_context(installing)?;
        let mut stream = signal(kind).with_context(installing)?;
        let cancel = Arc::clone(&cancel);
        tokio::spawn(async move {
            while stream.recv().await.is_some() {
                cancel.cancel();
            }
        });
    }
    init.release(&kinds.map(|kind| kind.as_raw_value()));
    Ok(guard)
}

/// What [`install_interrupt_guard`]'s handlers write: the last signal's
/// number, `0` until one lands, and whether the next one ends the process.
pub(crate) struct InterruptGuard {
    signalled: Arc<AtomicUsize>,
    armed: Arc<AtomicBool>,
}

impl InterruptGuard {
    /// Arm the guard, so any later signal ends the process in its handler,
    /// and end it now by a signal that landed before.
    pub(crate) fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
        match self.signalled.load(Ordering::SeqCst) {
            0 => {}
            n => die_by(n as i32),
        }
    }

    /// End the process by the signal that interrupted the run — `SIGINT`
    /// standing in for a number nothing recorded, which a run cancelled only
    /// by a handler cannot be.
    pub(crate) fn die_interrupted(&self) -> ! {
        match self.signalled.load(Ordering::SeqCst) {
            0 => die_by(tokio::signal::unix::SignalKind::interrupt().as_raw_value()),
            n => die_by(n as i32),
        }
    }
}

/// End the process by `signal`'s default action rather than by `exit(128 +
/// n)`: a shell reports the two alike, but a calling script stops only past a
/// child a signal killed (`docs/design/runtime-invariants.md`, "RT20").
/// Nothing runs destructors past this, so the instrument is asked here rather
/// than left to `commands`' guard, and stdout is flushed.
///
/// **Not as a PID namespace's init**, which the kernel shields from a signal
/// it raises at itself under the default action: the raise would be dropped
/// and `emulate_default_handler` fall back to `abort`. There it exits `128 +
/// n`, which is what a container runtime reports for the signal either way.
fn die_by(signal: i32) -> ! {
    use std::io::Write as _;

    introspect::report(introspect::Sections::Allocator);
    let _ = std::io::stdout().flush();
    if !namespace_init() {
        let _ = signal_hook::low_level::emulate_default_handler(signal);
    }
    std::process::exit(128 + signal);
}
