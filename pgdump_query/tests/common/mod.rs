//! The fixture vocabulary every integration test speaks.
//!
//! Each `tests/*.rs` file is its own crate, so without a shared module every
//! file that needs a fixture path carries its own copy of one — and copies of
//! a path helper drift silently, since nothing compares them. This module is
//! what a `tests/` directory has for that, and it costs one `mod common;` per
//! file.
//!
//! Everything here is a *path* or an *expectation*. Anything that drives the
//! library — a `collect`, a `drain`, a `census_of` — stays in the file whose
//! subject it is: those differ per test file in ways that matter, and merging
//! them here would be the second mistake this module exists to undo.

// Each test binary compiles the whole module and uses a subset of it, so
// per-binary dead-code warnings are structural rather than informative.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use arrow::array::RecordBatch;
use pgdump_query::{NestedPlan, render_field};

/// The **hand-written** dump at `tests/data/edge_cases.sql` — not a `pg_dump`
/// product and not part of the generated tree. It is the fixture for
/// structural edge cases no real dump reliably contains (a `COPY`-like phrase
/// inside a value, a header-less block, an embedded `\.`), which is why it is
/// written by hand and checked in.
///
/// Its generated namesake is [`edge_cases_fixture`], which is real `pg_dump`
/// output for the `edge_cases` schema. The two are unrelated files.
pub fn edge_cases() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/edge_cases.sql")
}

/// The root of the generated fixture tree, laid out as
/// `fixtures/<version>/<schema>/<flag-set>.sql`.
pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
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
/// blocks at all), and `dumpall` (concatenated, multi-`\connect`).
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
    out
}

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
/// is the only flag combination in the fixture matrix that emits a `\connect`
/// at all (plain `pg_dump` never
/// does), so it is the only one two copies of can be concatenated into this
/// shape.
///
/// The second copy has its database name changed so the two `\connect`
/// targets are distinguishable: `pgdt_fixture` (the fixture generator's fixed
/// `DB_NAME`) appears nowhere in a `--create` dump except in the
/// `CREATE DATABASE`/`ALTER DATABASE`/`\connect` lines naming it, so a
/// literal string replace is safe and needs no real second Postgres instance.
/// The rest of the schema — every table, type, and row — is identical between
/// the two, which is deliberate: it means a table name like `public.widgets`
/// genuinely collides across databases, exercising database-scoped resolution
/// and `table_stream`'s ambiguity reporting against a real dump instead of
/// only hand-written unit input.
pub fn multidb_fixture(version: u32) -> (tempfile::TempDir, PathBuf) {
    let content = std::fs::read_to_string(edge_cases_fixture(version, "create")).unwrap();
    let renamed = content.replace("pgdt_fixture", "pgdt_fixture_2");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("multidb.sql");
    std::fs::write(&path, format!("{content}{renamed}")).unwrap();
    (dir, path)
}

/// Every column of `batch` rendered back to PostgreSQL text via
/// [`render_field`] — which works regardless of `SchemaMode`: a hand-written
/// fixture with no DDL (like [`edge_cases`]) resolves every column
/// `Utf8View` either way, so this reads identically to a hardcoded
/// `StringViewArray` downcast there, and it also handles a real `pg_dump`
/// fixture's typed columns (`docs/design/decisions.md`, "D66":
/// render-back is exactly this build's own decode/render round trip).
///
/// The plan is `Scalar` for every column, which is right for every
/// non-nested type. A test reading a nested column wants the stream's own
/// `ResolvedSchema::plans` instead, and asks for them where it queries.
pub fn rows_of(batch: &RecordBatch) -> Vec<Vec<Option<String>>> {
    (0..batch.num_rows())
        .map(|row| {
            batch
                .columns()
                .iter()
                .map(|c| {
                    render_field(c.as_ref(), row, &NestedPlan::Scalar)
                        .expect("every fixture value renders back")
                })
                .collect()
        })
        .collect()
}

/// `public.widgets` from the hand-written [`edge_cases`] dump, as decoded
/// rows. Every value here is chosen to break a naive scanner or decoder: an
/// embedded newline and tab, a literal backslash, a `COPY`-like phrase inside
/// a field, an empty string next to a NULL, and a bare carriage return.
pub fn widgets_expected() -> Vec<Vec<Option<String>>> {
    vec![
        vec![
            Some("1".into()),
            Some("alpha".into()),
            Some("a simple widget".into()),
            Some("2024-01-01 00:00:00+00".into()),
        ],
        vec![Some("2".into()), Some("beta".into()), None, Some("2024-01-02 00:00:00+00".into())],
        vec![
            Some("3".into()),
            Some("gamma".into()),
            Some("multi\nline\twith a backslash \\ inside".into()),
            None,
        ],
        vec![
            Some("4".into()),
            Some("delta".into()),
            Some(
                "contains a COPY-like phrase: COPY public.widgets (id) FROM stdin; -- not a directive"
                    .into(),
            ),
            Some("2024-01-04 00:00:00+00".into()),
        ],
        vec![
            Some("5".into()),
            Some("".into()),
            Some("empty name to the left".into()),
            Some("2024-01-05 00:00:00+00".into()),
        ],
        vec![
            Some("6".into()),
            Some("epsilon".into()),
            Some("carriage\rreturn, octal A, hex B".into()),
            Some("2024-01-06 00:00:00+00".into()),
        ],
    ]
}
