//! `pgdq parse --chunk-size` / `pgdq query --chunk-size` — the read chunk's
//! CLI surface (`docs/design/measurements.md`, "What the read chunk size is
//! worth").
//!
//! The library already holds what a chunk size *means*: `scan.rs`'s
//! `event_stream_is_independent_of_chunk_size` and `batch.rs`'s
//! `batch_contents_are_independent_of_chunk_size` drive the scanner and the
//! batch stream at chunk sizes down to one byte. What only the binary can say
//! is that the flag reaches those options at all — a flag parsed and dropped
//! on the floor would leave every one of those tests passing and every
//! chunk-size reading a measurement of the default — and that the one value
//! which would hang the read loop is refused before the file is opened.

use std::process::Output;

mod common;
use common::{fixture, run, sandboxed, stderr_of, stdout_of};

fn widgets(extra: &[&str]) -> Output {
    let dump = fixture("16/edge_cases/default.sql");
    let mut args = vec![
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dqcache",
        "none",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

/// A tiny chunk forces nearly every row to straddle a chunk boundary, so this
/// is the flag reaching `ScanOptions` *and* the read carry answering the same
/// rows it does when a chunk holds the whole file.
#[test]
fn a_query_reads_the_same_rows_at_any_chunk_size() {
    let reference = widgets(&[]);
    assert!(reference.status.success(), "{}", stderr_of(&reference));
    for size in ["1", "3", "64", "1048576", "16777216"] {
        let out = widgets(&["--chunk-size", size]);
        assert!(out.status.success(), "chunk {size}: {}", stderr_of(&out));
        assert_eq!(stdout_of(&out), stdout_of(&reference), "chunk {size} changed the rows");
    }
}

/// The same for the scanner behind `parse`: the listing a one-byte chunk
/// leaves is the listing a default chunk leaves. Each run gets its own
/// sandboxed copy, because `parse` writes a cache beside the dump and a
/// second run would resume from the first rather than scan.
#[test]
fn a_parse_reports_the_same_listing_at_any_chunk_size() {
    let mut reference: Option<String> = None;
    for size in [None, Some("1"), Some("4096")] {
        let (_dir, dump) = sandboxed("16/edge_cases/default.sql", "chunked.sql");
        let mut args = vec!["parse", "--source", dump.to_str().unwrap()];
        if let Some(size) = size {
            args.extend_from_slice(&["--chunk-size", size]);
        }
        let out = run(&args);
        assert!(out.status.success(), "chunk {size:?}: {}", stderr_of(&out));
        // Everything but the trailing `wrote cache to <path>`, which names
        // this run's own tempdir.
        let out = stdout_of(&out);
        let got = out
            .lines()
            .filter(|line| !line.starts_with("wrote cache to "))
            .collect::<Vec<_>>()
            .join("\n");
        match &reference {
            None => reference = Some(got),
            Some(want) => assert_eq!(&got, want, "chunk {size:?} changed the listing"),
        }
    }
}

/// Zero is the one value that would turn the read loop into a scan that never
/// advances and never errors, so it is refused by the parser rather than
/// reaching the loop.
#[test]
fn a_zero_chunk_size_is_refused() {
    let out = widgets(&["--chunk-size", "0"]);
    assert!(!out.status.success());
    assert!(
        stderr_of(&out).contains("read nothing"),
        "the refusal should say why: {}",
        stderr_of(&out)
    );
}
