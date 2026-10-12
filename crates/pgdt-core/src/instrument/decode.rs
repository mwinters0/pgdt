//! How often a piecewise `.xz` read began decoding a block rather than
//! resuming one.

/// How many block decodes a piecewise `.xz` read path has begun rather than
/// resumed.
///
/// **Nothing without the instrument**: the field is absent, `begun` compiles
/// away and no build that ships carries the atomic. What it exists for is a
/// property a shipped build cannot state — that a block a forward scan sits
/// inside is decoded once rather than once per read — which a test asserts
/// rather than argues (`docs/design/decisions.md`, "D15").
#[derive(Debug, Default)]
pub struct DecodeCounter {
    #[cfg(feature = "introspect")]
    begun: std::sync::atomic::AtomicU64,
}

impl DecodeCounter {
    /// A block is about to be begun rather than resumed. Counted where that
    /// is decided, so a `begin` that then errors is counted.
    #[inline]
    pub fn begun(&self) {
        #[cfg(feature = "introspect")]
        self.begun.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many have. Racy against a read still running, which the caller
    /// avoids by reading once the read has returned.
    #[cfg(feature = "introspect")]
    pub fn count(&self) -> u64 {
        self.begun.load(std::sync::atomic::Ordering::Relaxed)
    }
}
