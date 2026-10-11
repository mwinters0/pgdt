use std::time::Duration;

use super::*;

/// The throttle's whole rule, over measured quantities rather than a
/// clock: a save that cost nothing never blocks another, and a save that
/// cost something blocks the next one until the scan has done `K` times
/// that much work.
#[test]
fn a_save_is_due_once_the_scan_has_outrun_the_last_ones_cost() {
    let ms = Duration::from_millis;

    assert!(SaveThrottle::due_after(Duration::ZERO, Duration::ZERO), "a free save never waits");
    assert!(SaveThrottle::due_after(ms(1), Duration::ZERO));

    // K = 20: 10ms of saving buys 200ms of silence.
    assert!(!SaveThrottle::due_after(ms(199), ms(10)));
    assert!(SaveThrottle::due_after(ms(200), ms(10)));
    assert!(SaveThrottle::due_after(ms(1000), ms(10)));

    // Blocks far apart and saves far cheaper than they are: untouched.
    assert!(SaveThrottle::due_after(Duration::from_secs(45), ms(300)));

    // And the pathological one: a cache expensive enough that saving it
    // every block is the scan.
    assert!(!SaveThrottle::due_after(ms(11), ms(10)));
}

/// A `last_cost` big enough to overflow `Duration * u32` saturates instead
/// of panicking. Unreachable in practice — it takes a save costing a
/// twentieth of `Duration::MAX` — but the multiplication is on the hot
/// path of every block.
#[test]
fn an_absurd_save_cost_saturates_rather_than_panicking() {
    assert!(!SaveThrottle::due_after(Duration::MAX / 2, Duration::MAX));
}
