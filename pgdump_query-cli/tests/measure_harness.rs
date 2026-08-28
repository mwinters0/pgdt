//! `scripts/measure.py`'s own unit tests, run by `cargo test --workspace`.
//!
//! The harness exists because a prose recipe is something nobody executes, and
//! a recipe nobody executes drifts out of runnability without anyone noticing
//! (`docs/design/roadmap.md`, out-of-band ledger, `M19`). A Python test suite
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
//! **Failed, not skipped, when `uv` is absent**, for the reason
//! `perf_generator_fidelity.rs` gives at length: `mise.toml` pins `uv`, a skip
//! is invisible in a green suite, and an invisible skip is how the thing being
//! guarded rots.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scripts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts")
}

#[test]
fn the_measurement_harness_passes_its_own_tests() {
    let uv_runnable =
        Command::new("uv").arg("--version").output().is_ok_and(|o| o.status.success());
    assert!(
        uv_runnable,
        "`uv` is not runnable, so the measurement harness's only guard cannot run. \
         `mise.toml` pins it: run `mise install`."
    );

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
