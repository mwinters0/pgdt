//! The cache saved on a throttle (`docs/design/decisions.md`, "D62").

use std::time::{Duration, Instant};

use pgdt_core::Result;
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_io::ByteRangeSource;
use pgdt_model::DumpIndex;

/// How many times the elapsed scan has to cover the last save's own cost
/// before another save is worth taking, which bounds save overhead at roughly
/// `1/K` of scan time.
const SAVE_THROTTLE_K: u32 = 20;

/// Decides whether a mid-scan cache save has earned its cost
/// (`docs/design/decisions.md`, "D62").
///
/// **The rule is self-tuning, not an interval**: skip a block's save unless at
/// least [`SAVE_THROTTLE_K`] times the last save's own duration has elapsed
/// since it. **Exits are exempt** — EOF, a settled target and an interrupt all
/// save unconditionally, an interrupt before the first read included.
///
/// The gate also decides when the map is rebuilt (`close_copy_block`; the
/// cost is [`splice`](super::splice)'s `KD5`).
pub(crate) struct SaveThrottle {
    last_save: Instant,
    last_cost: Duration,
}

impl SaveThrottle {
    /// Starts due: `last_cost` is zero, so the first block of a segment always
    /// banks, rather than leaving a resumable frontier never written down.
    pub(crate) fn new() -> Self {
        Self { last_save: Instant::now(), last_cost: Duration::ZERO }
    }

    /// The rule itself, over measured quantities rather than clocks.
    pub(crate) fn due_after(elapsed: Duration, last_cost: Duration) -> bool {
        elapsed >= last_cost.saturating_mul(SAVE_THROTTLE_K)
    }

    pub(crate) fn due(&self) -> bool {
        Self::due_after(self.last_save.elapsed(), self.last_cost)
    }

    /// Save, and time the save — that duration is the whole input to the next
    /// decision. A disabled cache makes this ~free and so seldom throttles:
    /// only a block ending within [`SAVE_THROTTLE_K`] times that measured
    /// cost of the last is held back.
    pub(crate) async fn save(
        &mut self,
        cache: &CacheMode,
        watch: &SourceWatch,
        source: &dyn ByteRangeSource,
        index: &DumpIndex,
    ) -> Result<()> {
        let started = Instant::now();
        cache.save(watch, source, index).await?;
        self.last_cost = started.elapsed();
        self.last_save = Instant::now();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
