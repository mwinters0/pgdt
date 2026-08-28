//! `scripts/measure.py`'s own unit tests, run by `cargo test --workspace`.
//!
//! The harness exists because a prose recipe is something nobody executes, and
//! a recipe nobody executes drifts out of runnability without anyone noticing —
//! this repo's own scan-throughput recipe passed a flag its generators do not
//! have and omitted a required path. A Python test suite
//! that only runs when someone remembers to type `uv run python -m unittest`
//! is that same failure with a different extension — and what it guards is the
//! median, the spread and the table formatting, whose silent failure produces
//! a *confidently wrong* table rather than an error.
//!
//! So the one automatic gate this repo has runs it. The suite is ~95 cases in
//! milliseconds: it touches no container, no `sudo` and no multi-gigabyte
//! input, because everything that needs those fails loudly on the first real
//! run anyway.
//!
//! **Failed, not skipped, when `uv` is absent** — see `common::require_uv`
//! for why: `mise.toml` pins `uv`, a skip is invisible in a green suite, and
//! an invisible skip is how the thing being guarded rots.

use std::process::Command;

mod common;
use common::{require_uv, scripts_dir};

#[test]
fn the_measurement_harness_passes_its_own_tests() {
    require_uv("the measurement harness's only guard");

    let out = Command::new("uv")
        .args(["run", "python", "-m", "unittest", "test_measure"])
        .current_dir(scripts_dir())
        .output()
        .expect("uv runs");

    assert!(
        out.status.success(),
        "scripts/test_measure.py failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}
