//! The purge delay `pgdt` sets as mimalloc's default, read back from the
//! binary as the harness reads it (`--version`), and the environment's
//! overriding it (`src/alloc.rs`, `configure`).

mod common;

use common::pgdt;

/// `pgdt --version`'s first line, under `delay` as `MIMALLOC_PURGE_DELAY`
/// or with the variable removed.
fn version_under(delay: Option<&str>) -> String {
    let mut command = pgdt();
    command.arg("--version").env_remove("MIMALLOC_PURGE_DELAY");
    if let Some(delay) = delay {
        command.env("MIMALLOC_PURGE_DELAY", delay);
    }
    let out = command.output().expect("the pgdt binary runs");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("--version is UTF-8")
}

/// With nothing stated the binary runs under its default, and a stated delay
/// replaces it. **The stated case is what holds `MI_OPTION_PURGE_DELAY`, an
/// index counted by hand, to mimalloc's purge delay**: mimalloc reads the
/// variable into its own option, so an index naming another one would read
/// back the default here, not the value stated. Two stated values, so a
/// default happening to equal one cannot pass it.
#[test]
fn the_purge_delay_is_pgdt_s_default_unless_the_environment_states_one() {
    let default = version_under(None);
    assert!(default.contains(" (purge delay: 100 ms) "), "{default}");
    for stated in ["0", "250"] {
        let version = version_under(Some(stated));
        assert!(version.contains(&format!(" (purge delay: {stated} ms) ")), "{version}");
    }
}
