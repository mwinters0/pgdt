//! **A key factor**: what an ordering key over a value the server
//! normalizes holds per byte of the value's text, beside the text — the
//! type a family states its own in, which `crate::key`'s `factor` reads a
//! column's comparison by (`docs/design/decisions.md`, "D152").

/// At most `held` bytes for every `per` bytes of a value's text, beside the
/// text itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyFactor {
    pub held: u64,
    pub per: u64,
}

impl KeyFactor {
    /// The most a key over a text of `len` bytes holds.
    pub fn of(self, len: u64) -> u64 {
        let held = (u128::from(len) * u128::from(self.held)).div_ceil(u128::from(self.per));
        u64::try_from(held).unwrap_or(u64::MAX)
    }

    /// The larger of two factors, `None` being none at all.
    pub(crate) fn max(a: Option<Self>, b: Option<Self>) -> Option<Self> {
        match (a, b) {
            (Some(a), Some(b)) => {
                // `a.held / a.per` against `b.held / b.per`, in integers.
                Some(if a.held * b.per >= b.held * a.per { a } else { b })
            }
            (a, b) => a.or(b),
        }
    }
}
