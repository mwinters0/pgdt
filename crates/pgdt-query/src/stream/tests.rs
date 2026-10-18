use super::mapping::map_for_query;
use super::replay::plan::{PlannedReplay, plan_replay};
use super::replay::segment::{Segment, SegmentEntry};
use super::resume::query_fingerprint;
use super::*;
use crate::options::{QueryOptions, ScanExtent};
use pgdt_dump::cache::StrictIdentity;
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_dump::{ReadOptions, ScanOptions};
use pgdt_io::Parallelism;
use pgdt_model::{CopyBlock, Finding, Severity};
use std::sync::Arc;

/// **The count note names a lever a plain source actually has** (`KD32`):
/// a source recommending no per-reader cost keeps
/// `pgdt_io::DEFAULT_MEMORY_BUDGET` however large an allowance is
/// stated (`docs/design/decisions.md`, "D83"), so a remedy offering the
/// budget alone is inert exactly where this note fires on one. Both arms
/// are checked, the span-carrying one and the bare, and the second half
/// is the premise itself — without it the first half is a string
/// asserting its own wording.
#[test]
fn the_count_note_names_a_lever_a_plain_source_has() {
    for span in [Some(1 << 20), None] {
        let note = PlanNote::parallelism_budget_limited(8, 2, 8 << 20, span, 64 << 20, false, true);
        let message = note.message();
        assert!(message.contains("a smaller read chunk"), "{message}");
    }

    // What makes the budget clause alone inert: twice the allowance is
    // the same budget, both of them `DEFAULT_MEMORY_BUDGET`.
    let budget = |bytes| {
        let request = pgdt_io::ArrangementRequest::recommended(8, None);
        pgdt_io::Arrangement::resolve(&pgdt_io::Allowance::Stated(bytes), request)
            .parallelism()
            .memory_bytes()
    };
    assert_eq!(budget(2 << 30), Some(pgdt_io::DEFAULT_MEMORY_BUDGET));
    assert_eq!(budget(4 << 30), budget(2 << 30));
}

/// **[`PlanNote::budget_bytes`] answers for exactly the notes whose
/// sentence quotes a budget**, which is what a caller appends its
/// provenance clause to (`docs/design/decisions.md`, "D64"). Asserted
/// against the message rather than against a second list of kinds: a note
/// added later that names a budget and forgets the accessor fails here,
/// where a list would agree with itself.
#[test]
fn a_note_quoting_a_budget_is_the_one_that_reports_it() {
    let memory_bytes = 64 << 20;
    let notes = [
        PlanNote::compressed_block_path_declined(9, 1 << 20, 2 << 20, memory_bytes, true),
        PlanNote::allocation_below_floor(8 << 20, memory_bytes, false, true),
        PlanNote::batch_span_narrowed(64 << 20, 1 << 20, 7, memory_bytes, false, true),
        PlanNote::parallelism_budget_limited(
            8,
            7,
            8 << 20,
            Some(1 << 20),
            memory_bytes,
            false,
            true,
        ),
        PlanNote {
            kind: PlanNoteKind::StatisticsPruned {
                skipped_groups: 1,
                groups: 2,
                skipped_bytes: 3,
                bytes: 4,
            },
            levers: Vec::new(),
        },
    ];
    for note in notes {
        let quoted = note.message().contains(&format!("memory budget of {memory_bytes} byte(s)"));
        assert_eq!(note.budget_bytes().is_some(), quoted, "{}", note.message());
        assert!(note.budget_bytes().is_none_or(|bytes| bytes == memory_bytes));
    }
}

/// **A budget's note is a `Warning` exactly where it declined something
/// asked of it**: a skip and a narrowed span are `Info`, as `pgdt` has
/// always printed them `note:`. A sink recovers the note itself.
#[test]
fn a_plan_note_warns_where_the_budget_declined_what_was_asked() {
    let memory_bytes = 64 << 20;
    let cases = [
        (PlanNote::compressed_block_path_declined(9, 1, 2, memory_bytes, true), Severity::Warning),
        (PlanNote::allocation_below_floor(8 << 20, memory_bytes, false, true), Severity::Warning),
        (
            PlanNote::parallelism_budget_limited(8, 7, 1, None, memory_bytes, false, true),
            Severity::Warning,
        ),
        (
            PlanNote::batch_span_narrowed(64 << 20, 1 << 20, 7, memory_bytes, false, true),
            Severity::Info,
        ),
        (
            PlanNote {
                kind: PlanNoteKind::StatisticsPruned {
                    skipped_groups: 1,
                    groups: 2,
                    skipped_bytes: 3,
                    bytes: 4,
                },
                levers: Vec::new(),
            },
            Severity::Info,
        ),
    ];
    for (note, severity) in cases {
        let finding: &dyn Finding = &note;
        assert_eq!(finding.severity(), severity, "{}", finding.message());
        assert_eq!(finding.as_any().downcast_ref::<PlanNote>(), Some(&note));
    }
}

/// A resumed stream keeps a segment while it owns a row at or past the
/// pause — the row starting exactly at its limit included — and enters
/// one begun before the pause on the LF ending the paused row.
#[test]
fn a_segment_resumes_from_the_row_its_pause_left() {
    let block = CopyBlock {
        header: pgdt_core::parse_copy_header(b"COPY public.t (id) FROM stdin;").unwrap(),
        header_offset: 10,
        data_offset: 40,
        terminator_offset: 400,
        end_offset: 403,
        row_count: 0,
        partition_root: None,
        statistics: None,
        statistics_declined: None,
        ignored_refusals: None,
        checked_in_full: false,
        census: Some(pgdt_model::BlockCensus::default()),
    };
    let whole = Segment::over(&block, 39..403);
    assert_eq!((whole.start, whole.entry), (10, SegmentEntry::Header));
    let run = Segment::over(&block, 99..199);
    assert_eq!((run.start, run.entry), (99, SegmentEntry::Interior));
    let at = |segment: &Segment, offset| {
        segment.clone().resumed_at(offset).map(|s| (s.start, s.limit, s.entry))
    };

    assert_eq!(at(&run, 0), Some((99, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 60), Some((99, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 100), Some((99, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 150), Some((149, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 199), Some((198, 199, SegmentEntry::Interior)));
    assert_eq!(at(&run, 200), None);
    assert_eq!(at(&whole, 10), Some((10, 403, SegmentEntry::Header)));
    assert_eq!(at(&whole, 120), Some((119, 403, SegmentEntry::Interior)));
}

/// A sub-stream's resume token is stamped with the partition it came out
/// of, so "resuming a partition is unsupported" is an error rather than a
/// silent superset of the rows that partition had left.
#[test]
fn a_partitions_fingerprint_differs_from_the_whole_streams() {
    let options = QueryOptions::default();
    let whole = query_fingerprint("public.t", &options, None);
    let first = query_fingerprint("public.t", &options, Some((0, 4)));
    let second = query_fingerprint("public.t", &options, Some((1, 4)));
    let of_two = query_fingerprint("public.t", &options, Some((0, 2)));
    assert_ne!(whole, first);
    assert_ne!(first, second);
    assert_ne!(first, of_two);
}

/// One table in two blocks naming its columns in different orders.
pub(super) fn two_blocks(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("two_blocks.sql");
    std::fs::write(
        &path,
        "COPY public.t (a, b) FROM stdin;\n1\tx\n2\t\\N\n\\.\n\n\
             COPY public.t (b, a) FROM stdin;\ny\t3\n\\.\n",
    )
    .unwrap();
    path
}

/// **A replay planned for `table_stream` or `table_stream_partitions`
/// keeps no block's statistics**: once its plan and segments
/// exist, the matched blocks the map handed over are the only holders of
/// each block's `Arc`, so dropping them frees the statistics before a row
/// is read, on the partitioned path and the serial one alike.
#[tokio::test]
async fn a_replay_holds_no_block_s_statistics() {
    use pgdt_io::LocalFileSource;
    use pgdt_model::{BlockStatistics, GroupSizing};

    let dir = tempfile::tempdir().unwrap();
    let source = LocalFileSource::open(two_blocks(dir.path())).unwrap();
    let query_options = QueryOptions {
        scan_extent: ScanExtent::Full,
        parallelism: Parallelism::workers(4, pgdt_io::DEFAULT_MEMORY_BUDGET),
        ..QueryOptions::default()
    };
    let mapped = map_for_query(
        &source,
        "public.t",
        &ScanOptions::default(),
        &query_options,
        &CacheMode::DISABLED,
        &SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap(),
    )
    .await
    .unwrap();
    let mut matches = mapped.matches;
    assert_eq!(matches.len(), 2);
    for block in &mut matches {
        block.statistics = Some(Arc::new(BlockStatistics {
            group_size: 1 << 20,
            sizing: GroupSizing::Stated,
            groups: Vec::new(),
            columns: vec![None; block.header.columns.len()],
        }));
    }
    let held: Vec<_> =
        matches.iter().map(|block| Arc::clone(block.statistics.as_ref().unwrap())).collect();

    let PlannedReplay { plan, groups, .. } = plan_replay(
        &source,
        mapped.database,
        &matches,
        mapped.metadata,
        ReadOptions::default(),
        query_options,
    )
    .unwrap();
    let serial: Vec<Segment> = matches.iter().flat_map(|block| plan.segments(block)).collect();
    assert!(groups.iter().flatten().chain(&serial).all(|s| s.block.statistics.is_none()));
    drop(matches);
    for statistics in &held {
        assert_eq!(Arc::strong_count(statistics), 1, "only the test's own copy remains");
    }
}
