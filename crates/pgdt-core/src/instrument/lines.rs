//! The moments a held line's draw is first admitted and returned, told to an
//! instrumented binary that asked to hear them: what the heap holds resident
//! as the ledger lends a returned draw's bytes to the next line is read at
//! those moments (`docs/design/decisions.md`, "D165").
//!
//! **Only with the instrument**: a shipped build neither computes an event nor
//! keeps an observer, the ledger's calls being behind the same feature. The
//! observer runs on the reader's thread, outside the ledger's lock, as the
//! draw returns or the line is released.

#[cfg(feature = "introspect")]
pub use enabled::{LineEvent, LineMoment, line_moment, observe_lines};

#[cfg(feature = "introspect")]
mod enabled {
    use std::sync::OnceLock;

    /// Which moment of a held line's draw an event is.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LineMoment {
        /// Its first draw was admitted: the line holds a charge where it held
        /// none, a re-read after giving its draw up included.
        Admitted,
        /// What it held was returned: its row was emitted, its reader dropped
        /// it, or it was refused.
        Returned,
    }

    /// One moment of one line's draw.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct LineEvent {
        pub moment: LineMoment,
        /// The line's start in the file.
        pub start: u64,
        /// The charge admitted, or what was returned.
        pub charge: u64,
        /// What the ledger's lines and statistics hold once it is, or `None`
        /// where the ledger keeps no account, having no limit.
        pub drawn: Option<u64>,
    }

    /// The one observer a process registers, or none.
    static OBSERVER: OnceLock<fn(LineEvent)> = OnceLock::new();

    /// Tell `observer` every [`LineEvent`] from now on. The first observer
    /// registered is kept: one binary, one report.
    pub fn observe_lines(observer: fn(LineEvent)) {
        let _ = OBSERVER.set(observer);
    }

    /// The ledger's hook: `event` happened. Nothing where no observer is
    /// registered.
    #[inline]
    pub fn line_moment(event: LineEvent) {
        if let Some(observer) = OBSERVER.get() {
            observer(event);
        }
    }
}

#[cfg(all(test, feature = "introspect"))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static HEARD: Mutex<Vec<LineEvent>> = Mutex::new(Vec::new());

    fn heard(event: LineEvent) {
        HEARD.lock().unwrap().push(event);
    }

    #[test]
    fn the_observer_hears_each_moment_and_none_before_it_is_registered() {
        // One process-wide observer, so one test walks every transition.
        let event =
            LineEvent { moment: LineMoment::Admitted, start: 7, charge: 64, drawn: Some(64) };
        line_moment(event);
        observe_lines(heard);
        line_moment(event);
        let returned = LineEvent { moment: LineMoment::Returned, drawn: None, ..event };
        line_moment(returned);
        assert_eq!(*HEARD.lock().unwrap(), [event, returned]);
    }
}
