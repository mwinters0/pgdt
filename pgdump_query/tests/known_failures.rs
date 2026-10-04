//! The strict known-failure table: a fixture that exposes a defect lands
//! before its fix, and this asserts that the defect is still there
//! (`docs/design/decisions.md`, "D71"): the test-side mirror of the emitter
//! register's `KD<k>` exemption, keeping the evidence first without a gap
//! reading as coverage.
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

use pgdump_query::map::SpanBody;
use pgdump_query::{DataBlock, LocalFileSource, ScanOptions, build_map};

mod common;
use common::all_fixtures;

/// What a correct reading of the fixture would show.
#[derive(Debug, Clone)]
enum Case {
    /// Every `COPY` block and `INSERT` run carries a TOC entry.
    DataSpansAttributed,
}

struct KnownFailure {
    kd: &'static str,
    fixture: &'static str,
    /// A fixture the case passes on, where the tree holds the table read
    /// correctly; `None` where no fixture does, the case then resting on its
    /// own panics for a fixture holding nothing it reads.
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
    }
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
