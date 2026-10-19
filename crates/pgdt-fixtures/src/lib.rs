//! The fixture vocabulary every crate's tests speak: where the fixtures are,
//! the sweep's enumerator, the sandboxes, the multi-database concatenation,
//! the fields PostgreSQL refuses, a fixture mapped beside its cache, and the
//! hand-written `edge_cases.sql`.
//!
//! Each `tests/*.rs` file is its own crate, so without one shared home every
//! file that needs a fixture path carries its own copy of one — and copies of
//! a path helper drift silently, since nothing compares them. This is that
//! home: a member only tests depend on, each as a dev-dependency, depending on
//! no member but behind `mapping`, so every crate's tests reach it with the
//! graph one-way (`docs/design/decisions.md`, "D68").
//!
//! Everything here is a *path* or an *expectation*, but for three calls into
//! the library, behind `mapping`: a fixture copied out of the tree and mapped
//! beside its cache, which every test reading a map starts from and which
//! differs per test only in the request it states, that cache with its
//! statistics stripped, and the arrangement a mapping pass stating a worker
//! count and no allowance reads under. Anything else that drives
//! the library — a `collect`, a `drain`, a `census_of` — stays in the file
//! whose subject it is: those differ per test file in ways that matter, and
//! merging them here would be the second mistake this crate exists to undo.

use std::path::{Path, PathBuf};

/// The **hand-written** dump beside this crate's manifest — not a `pg_dump`
/// product and not part of the generated tree. It is the fixture for
/// structural edge cases no real dump reliably contains (a `COPY`-like phrase
/// inside a value, a header-less block, an embedded `\.`), which is why it is
/// written by hand and checked in.
///
/// Its generated namesake is [`edge_cases_fixture`], which is real `pg_dump`
/// output for the `edge_cases` schema. The two are unrelated files.
pub fn edge_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("edge_cases.sql")
}

/// The root of the generated fixture tree, laid out as
/// `fixtures/<version>/<schema>/<flag-set>.sql`.
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// One generated fixture, addressed the way the tree is laid out. Each schema
/// gets a shorthand below; reach for this one only when the schema is itself a
/// variable.
pub fn fixture(version: u32, schema: &str, flag_set: &str) -> PathBuf {
    fixtures_root().join(version.to_string()).join(schema).join(format!("{flag_set}.sql"))
}

/// Real `pg_dump` output for the `edge_cases` schema — as distinct from the
/// hand-written [`edge_cases`].
pub fn edge_cases_fixture(version: u32, flag_set: &str) -> PathBuf {
    fixture(version, "edge_cases", flag_set)
}

/// Real `pg_dump` output for the `types` schema: the column-per-type fixture
/// the resolution and decode tests read.
pub fn types_fixture(version: u32, flag_set: &str) -> PathBuf {
    fixture(version, "types", flag_set)
}

/// Real `pg_dump` output for the `objects` schema: the DDL-object inventory
/// fixture, including large objects and dollar-quoted function bodies.
pub fn objects_fixture(version: u32, flag_set: &str) -> PathBuf {
    fixture(version, "objects", flag_set)
}

/// Real `pg_dump` output for the `partitions` schema: a partitioned table,
/// whose data arrives as one `COPY` block per partition (I2).
pub fn partitions_fixture(version: u32, flag_set: &str) -> PathBuf {
    fixture(version, "partitions", flag_set)
}

/// Real `pg_dump` output for the `statistics` schema: the column shapes
/// per-row-group statistics are gathered from, and a value longer than a read
/// chunk.
pub fn statistics_fixture(version: u32, flag_set: &str) -> PathBuf {
    fixture(version, "statistics", flag_set)
}

/// Every major we generate fixtures for. The properties these versions are
/// swept for are ones the dump format holds identically across all of them
/// (I25 for the array literal shape, say) — running the same assertions on
/// each is what says so.
pub const VERSIONS: [u32; 6] = [13, 14, 15, 16, 17, 18];

/// Every real `pg_dump` output file the fixture generator produced, across
/// all six routine versions and every schema — read off the tree rather
/// than listed, so a new schema or flag set is swept the moment the generator
/// writes it. Includes the degenerate shapes the design doc calls out by
/// name: `data-only`, `schema-only`, `inserts`/`column-inserts` (no `COPY`
/// blocks at all), and `dumpall` (concatenated, multi-`\connect`). In path
/// order, so a sweep's failures read the same on every filesystem.
pub fn all_fixtures() -> Vec<PathBuf> {
    let root = fixtures_root();
    let mut out = Vec::new();
    for version in std::fs::read_dir(&root).unwrap() {
        let version = version.unwrap().path();
        if !version.is_dir() {
            continue;
        }
        for schema in std::fs::read_dir(&version).unwrap() {
            let schema = schema.unwrap().path();
            if !schema.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&schema).unwrap() {
                let path = entry.unwrap().path();
                if path.extension().is_some_and(|e| e == "sql") {
                    out.push(path);
                }
            }
        }
    }
    assert!(!out.is_empty(), "fixture discovery found nothing — did the tree move?");
    out.sort();
    out
}

#[cfg(feature = "mapping")]
mod parsed;
mod refused;
#[cfg(feature = "mapping")]
pub use parsed::{arranged, parsed_copy, ungathered};
pub use refused::{REFUSED_FIELDS, RefusedField, refused_field};
#[cfg(feature = "mapping")]
pub use refused::{past_refused, sweep_request};

/// A private copy of `source` in a fresh tempdir, named `name`, so a test can
/// freely read and write a colocated `.dtcache` beside it without touching
/// the checked-in fixture. The `TempDir` is returned because dropping it
/// deletes the copy.
pub fn sandboxed(source: &Path, name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join(name);
    std::fs::copy(source, &dump).unwrap();
    (dir, dump)
}

/// [`sandboxed`] applied to the hand-written [`edge_cases`] dump — the
/// cache tests' default subject.
pub fn sandboxed_edge_cases() -> (tempfile::TempDir, PathBuf) {
    sandboxed(&edge_cases(), "edge_cases.sql")
}

/// Two copies of `edge_cases/create.sql`, concatenated: a real
/// `\connect`-delimited multi-database dump, the shape `pg_dumpall` and
/// hand-concatenated dump files produce. `--create`
/// is the only `pg_dump` flag combination in the fixture matrix that emits a
/// `\connect` at all (plain `pg_dump` never does; `dumpall.sql` is
/// `pg_dumpall`'s own), so it is the only one two copies of can be
/// concatenated into this shape.
///
/// The second copy has its database name changed so the two `\connect`
/// targets are distinguishable: `pgdt_fixture` (the fixture generator's fixed
/// `DB_NAME`) appears nowhere in a `--create` dump except in the
/// `CREATE DATABASE`/`ALTER DATABASE`/`\connect` lines and the TOC comments
/// naming it, so a
/// literal string replace is safe and needs no real second Postgres instance.
/// The rest of the schema — every table, type, and row — is identical between
/// the two, which is deliberate: it means a table name like `public.widgets`
/// genuinely collides across databases, exercising database-scoped resolution
/// and a cold query's ambiguity reporting against a real dump instead of
/// only hand-written unit input.
pub fn multidb_fixture(version: u32) -> (tempfile::TempDir, PathBuf) {
    let content = std::fs::read_to_string(edge_cases_fixture(version, "create")).unwrap();
    let renamed = content.replace("pgdt_fixture", "pgdt_fixture_2");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multidb.sql");
    std::fs::write(&path, format!("{content}{renamed}")).unwrap();
    (dir, path)
}
