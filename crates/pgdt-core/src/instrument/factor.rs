//! The factor a query's plan settled: the most its decode and its ordering
//! keys hold beside a line, per byte of it (`docs/design/decisions.md`,
//! "D154"), recorded where a replay is planned so an instrumented binary
//! reports the one its query planned rather than a figure restating it.
//!
//! **Only with the instrument**: a shipped build neither computes nor keeps
//! it, the planner's call being behind the same feature.

#[cfg(feature = "introspect")]
pub use enabled::{factor_planned, factor_reading};

#[cfg(feature = "introspect")]
mod enabled {
    use std::sync::Mutex;

    /// Every plan's factor so far: `None` until one is planned, then the
    /// largest, `None` inside where no plan held anything that grows.
    static PLANNED: Mutex<Option<Option<(u64, u64)>>> = Mutex::new(None);

    /// A replay was planned holding `factor` beside its line — `held` bytes
    /// for every `per` of it — or nothing that grows with it. The largest a
    /// process plans is kept.
    pub fn factor_planned(factor: Option<(u64, u64)>) {
        let Ok(mut planned) = PLANNED.lock() else { return };
        let largest = match (planned.flatten(), factor) {
            (Some((a, b)), Some((c, d))) => {
                // `a / b` against `c / d`, in integers.
                Some(if u128::from(a) * u128::from(d) >= u128::from(c) * u128::from(b) {
                    (a, b)
                } else {
                    (c, d)
                })
            }
            (kept, new) => kept.or(new),
        };
        *planned = Some(largest);
    }

    /// The largest factor any replay this process planned held, as `held`
    /// bytes for every `per`: `None` where none was planned, `Some(None)`
    /// where every plan held nothing that grows with its line.
    pub fn factor_reading() -> Option<Option<(u64, u64)>> {
        PLANNED.lock().ok().and_then(|planned| *planned)
    }
}

#[cfg(all(test, feature = "introspect"))]
mod tests {
    use super::*;

    #[test]
    fn the_largest_factor_planned_is_kept() {
        // One process-wide reading, so one test walks every transition.
        assert_eq!(factor_reading(), None);
        factor_planned(None);
        assert_eq!(factor_reading(), Some(None));
        factor_planned(Some((65, 16)));
        factor_planned(Some((1, 1)));
        assert_eq!(factor_reading(), Some(Some((65, 16))));
        factor_planned(Some((161, 16)));
        factor_planned(None);
        assert_eq!(factor_reading(), Some(Some((161, 16))));
    }
}
