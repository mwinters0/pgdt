//! `pgdt` against gzip input, end to end: what the binary does with the file
//! `pg_dump -Fp --compress` writes.

mod common;
use common::{VERSIONS, gzip_fixture, run, stderr_of};

/// **A gzip dump is recognised and refused by name**, never read as plain
/// text: no command builds a gzip index yet, so every one that reads the dump
/// says what it is and writes no cache
/// (`docs/manual/dump-inspection.md`, "`.xz` files are read directly").
#[test]
fn a_gzip_dump_is_refused_as_gzip_and_writes_no_cache() {
    let version = VERSIONS[VERSIONS.len() - 1];
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql.gz");
    std::fs::copy(gzip_fixture(version, "edge_cases", "compress"), &dump).unwrap();
    let source = dump.to_str().unwrap();
    for args in [
        vec!["parse", "--source", source],
        vec!["query", "--source", source, "--table", "public.t"],
    ] {
        let out = run(&args);
        assert!(!out.status.success(), "{args:?} succeeded");
        let stderr = stderr_of(&out);
        assert!(stderr.contains("cannot be read: it is gzip-compressed"), "{args:?}: {stderr}");
    }
    let left: Vec<_> =
        std::fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, ["dump.sql.gz"], "nothing beside the dump");
}
