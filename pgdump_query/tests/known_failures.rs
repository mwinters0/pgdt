//! The strict known-failure table: a fixture that exposes a defect lands
//! before its fix, and this asserts that the defect is still there
//! (`docs/design/roadmap-P31-correctness-evidence.md`, "A known failure is
//! asserted to fail").
//!
//! Each row names a register entry, a fixture by `<schema>/<flag-set>`, and
//! the **case**: what reading that fixture correctly would show. The walk
//! finds the fixture at every major the tree holds it and asserts the case
//! fails at each, then asserts that it passes on the row's **control**, a
//! fixture lacking the shape, so a case that fails for a reason other than its
//! defect (a renamed table, a moved column) reads as a broken row rather than
//! as the defect. A fix turns its row red; the fixing slice deletes the row,
//! and the row's fixture is then swept like any other.
//!
//! Every row's `KD<k>` is an open entry of `docs/status/deficiencies.md`,
//! which `scripts/test_deficiencies.py` checks, no Rust test reading `docs/`.
//! A sweep elsewhere that the same defect trips carries a strict exclusion of
//! its own, naming the entry (`tests/value_oracle.rs`'s `EXCLUSIONS`).

use std::path::{Path, PathBuf};

use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::map::SpanBody;
use pgdump_query::resolve::ColumnResolution;
use pgdump_query::{
    DataBlock, LocalFileSource, QueryOptions, ResolvedSchema, ScanOptions, build_map, table_stream,
};

mod common;
use common::all_fixtures;

/// What a correct reading of the fixture would show.
#[derive(Debug, Clone)]
enum Case {
    /// Every `COPY` block and `INSERT` run carries a TOC entry.
    DataSpansAttributed,
    /// The column resolves as stated.
    Resolves { table: &'static str, column: &'static str, to: ColumnResolution },
}

struct KnownFailure {
    kd: &'static str,
    fixture: &'static str,
    /// A fixture the case passes on, where the tree holds the table read
    /// correctly; `None` where no fixture does, the case then resting on its
    /// own panics for a table or column that is not there.
    control: Option<&'static str>,
    case: Case,
}

const KNOWN_FAILURES: &[KnownFailure] = &[
    KnownFailure {
        kd: "KD1",
        fixture: "edge_cases/disable-triggers",
        control: Some("edge_cases/data-only"),
        case: Case::DataSpansAttributed,
    },
    KnownFailure {
        kd: "KD1",
        fixture: "emitters/data-only",
        control: Some("emitters/default"),
        case: Case::DataSpansAttributed,
    },
    KnownFailure {
        kd: "KD1",
        fixture: "emitters/dumpall-data-only",
        control: Some("emitters/dumpall"),
        case: Case::DataSpansAttributed,
    },
    KnownFailure {
        kd: "KD73",
        fixture: "emitters/dumpall-binary-upgrade",
        control: Some("emitters/dumpall"),
        case: Case::Resolves {
            table: "emitters.tuned",
            column: "amount",
            to: ColumnResolution::Mapped,
        },
    },
];

/// Every major's copy of `<schema>/<flag-set>`, off the tree.
fn copies(fixture: &str) -> Vec<PathBuf> {
    let suffix = Path::new(fixture).with_extension("sql");
    let mut found: Vec<PathBuf> =
        all_fixtures().into_iter().filter(|path| path.ends_with(&suffix)).collect();
    found.sort();
    found
}

/// The case's verdict on one fixture: `Ok` where it reads correctly, `Err`
/// saying how it does not.
async fn check(case: &Case, path: &Path) -> Result<(), String> {
    match case {
        Case::DataSpansAttributed => {
            let source = LocalFileSource::open(path).unwrap();
            let spans = build_map(&source, &ScanOptions::default()).await.unwrap();
            let mut data = 0;
            let mut bare = Vec::new();
            for span in &spans {
                let table = match &span.body {
                    SpanBody::Data(DataBlock::Copy(block)) => block.header.table.clone(),
                    SpanBody::Data(DataBlock::InsertRun(run)) => run.table.clone(),
                    _ => continue,
                };
                data += 1;
                if span.toc.is_none() {
                    bare.push(table);
                }
            }
            assert!(data > 0, "{}: no data span to attribute", path.display());
            if bare.is_empty() { Ok(()) } else { Err(format!("unattributed: {bare:?}")) }
        }
        Case::Resolves { table, column, to } => {
            let schema = resolved(path, table).await;
            let index = schema
                .schema
                .index_of(column)
                .unwrap_or_else(|_| panic!("{}: {table} has no column {column}", path.display()));
            let found = &schema.columns[index];
            if found == to { Ok(()) } else { Err(format!("{column} resolves {found:?}")) }
        }
    }
}

/// The schema a typed read of `table` commits to.
async fn resolved(path: &Path, table: &str) -> ResolvedSchema {
    let source = LocalFileSource::open(path).unwrap();
    let mut stream = table_stream(
        &source,
        table,
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        CacheMode::DISABLED,
    );
    while let Some(batch) = stream.next().await {
        batch.unwrap();
    }
    stream.resolved_schema()
}

#[tokio::test]
async fn every_known_failure_still_fails_and_its_control_passes() {
    for row in KNOWN_FAILURES {
        let failing = copies(row.fixture);
        assert!(!failing.is_empty(), "{}: no fixture {} in the tree", row.kd, row.fixture);
        for path in &failing {
            let verdict = check(&row.case, path).await;
            assert!(
                verdict.is_err(),
                "{}: {} now reads correctly ({:?}) — the fix's slice deletes this row",
                row.kd,
                path.display(),
                row.case
            );
        }
        let Some(control) = row.control else { continue };
        let controls = copies(control);
        assert!(!controls.is_empty(), "{}: no control {control} in the tree", row.kd);
        for path in &controls {
            if let Err(why) = check(&row.case, path).await {
                panic!("{}: the control {} fails the case too: {why}", row.kd, path.display());
            }
        }
    }
}
