use super::*;
use pgdt_core::Error;
use pgdt_dump::ScanOptions;
use pgdt_dump::internal::Builder;
use pgdt_dump::{CopyScanner, Event};
use pgdt_fixtures::{edge_cases, fixture};
use pgdt_io::Cancellation;
use pgdt_io::{ByteRangeSource, PartitionRead};
use pgdt_io::{LocalFileSource, Parallelism};
use pgdt_model::internal::FieldCount;
use pgdt_model::{BlockCensus, UnrepresentableTier};
use pgdt_model::{DataBlock, SpanBody};
use std::sync::Arc;

/// A cancellation already asked for, so the scan under it stops at its
/// first poll rather than at a moment a test would have to arrange.
fn cancelled() -> Arc<Cancellation> {
    let cancel = Arc::new(Cancellation::new());
    cancel.cancel();
    cancel
}

/// Counts every field spelled `infinity` past the format spec, as a
/// `date` column's count would: what a split must count exactly as the
/// serial pass does.
struct Infinities;

impl FieldCount for Infinities {
    fn counts_any(&self) -> bool {
        true
    }

    fn tier(&self, _column: usize, field: &[u8]) -> Option<UnrepresentableTier> {
        (field == b"infinity").then_some(UnrepresentableTier::Format)
    }
}

/// How a block `columns` wide is censused and counted here, `None` at
/// the metadata level.
fn plan(columns: Option<usize>) -> Option<CensusPlan> {
    columns.map(|width| CensusPlan { width, counter: Arc::new(Infinities) })
}

/// One `COPY` block as the **serial** pass states it: the reference every
/// split below is checked against.
#[derive(Debug, PartialEq, Eq)]
struct Reference {
    header_offset: u64,
    data_offset: u64,
    end_offset: u64,
    columns: Option<usize>,
    interior: Interior,
}

/// Drive the serial scanner and `map::Builder` over `file`, one buffer, as
/// `crate::forward::map_forward` does over its chunks, and report every `COPY` block it found.
/// Synchronous on purpose: the reference must not depend on a source, a
/// runtime or a chunk size, the very things the split may differ in.
fn reference(file: &[u8]) -> Vec<Reference> {
    let mut scanner = CopyScanner::new();
    let mut builder = Builder::with_database(None);
    let mut columns: Vec<usize> = Vec::new();
    while let Some(event) = scanner.next_event(file, true).expect("a scannable file") {
        match event {
            Event::CopyStart(start) => {
                columns.push(start.header.columns.len());
                builder.on_copy_start(*start, true);
                builder.count_block(Arc::new(Infinities));
            }
            Event::Row(row) => builder.on_row(row.offset, row.raw),
            Event::CopyEnd(end) => builder.on_copy_end(end).expect("nothing gathers here"),
            Event::Line(line) => builder.feed_line(line.offset, line.raw),
            Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
            Event::LargeObjectStart(start) => builder.on_large_object_start(start.start_offset),
            Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
        }
    }
    let spans = builder.finish(file.len() as u64);
    let mut widths = columns.into_iter();
    spans
        .into_iter()
        .filter_map(|span| match span.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some(Reference {
                header_offset: block.header_offset,
                data_offset: block.data_offset,
                end_offset: block.end_offset,
                columns: Some(widths.next().expect("one header per block")),
                interior: Interior {
                    census: block.census.expect("every block is censused and counted here"),
                    end: CopyEnd {
                        terminator_offset: block.terminator_offset,
                        end_offset: block.end_offset,
                        row_count: block.row_count,
                    },
                },
            }),
            _ => None,
        })
        .collect()
}

/// Cut `[data_offset, end_offset)` into `pieces` even ranges, scan each the
/// way a worker would, and merge. The bytes handed to each piece run to
/// end of file — the caller's half of the contract.
fn split(file: &[u8], block: &Reference, pieces: usize) -> Option<Interior> {
    cut_up(file, block, pieces).0
}

/// The same, reporting how many pieces owned a row — what says a passing
/// equality was a real split.
fn cut_up(file: &[u8], block: &Reference, pieces: usize) -> (Option<Interior>, usize) {
    let len = block.end_offset - block.data_offset;
    let mut scans = Vec::with_capacity(pieces);
    for i in 0..pieces as u64 {
        let start = block.data_offset + len * i / pieces as u64;
        let limit = block.data_offset + len * (i + 1) / pieces as u64;
        let entry = if i == 0 { PieceEntry::RowStart } else { PieceEntry::Resync };
        scans.push(
            scan_piece(
                &file[start as usize..],
                start,
                entry,
                limit,
                plan(block.columns).as_ref(),
                None,
            )
            .expect("a piece of a scannable file"),
        );
    }
    let working = scans.iter().filter(|p| p.rows > 0).count();
    (merge(&scans), working)
}

/// **The property the whole mechanism rests on: how a block's interior was
/// split must not be visible in the answer.** A block cut every which way
/// still reports the serial scanner's row count, terminator and end, and
/// the serial pass's census.
#[test]
fn a_split_interior_answers_what_the_serial_scanner_answers() {
    let files = [
        edge_cases(),
        fixture(16, "edge_cases", "default"),
        fixture(16, "types", "default"),
        fixture(16, "partitions", "default"),
        fixture(13, "edge_cases", "default"),
        fixture(18, "objects", "default"),
    ];
    let mut blocks_checked = 0usize;
    let mut really_split = 0usize;
    for path in files {
        let file = std::fs::read(&path).unwrap();
        let blocks = reference(&file);
        for block in &blocks {
            for pieces in [1usize, 2, 3, 4, 5, 7, 8, 16, 33, 64] {
                let (got, working) = cut_up(&file, block, pieces);
                assert_eq!(
                    got.as_ref(),
                    Some(&block.interior),
                    "{} block at {} in {pieces} pieces",
                    path.display(),
                    block.data_offset
                );
                if working > 1 {
                    really_split += 1;
                }
            }
            blocks_checked += 1;
        }
    }
    assert!(blocks_checked > 0, "fixture discovery found no COPY blocks");
    // Without this the equalities above could all be one piece owning
    // every row: the serial path compared to itself.
    assert!(really_split > 0, "no cut ever gave two pieces rows of their own");
}

/// More pieces than rows is the degenerate end of the same property: a
/// piece with no row start owns nothing, and every row is still seen once.
#[test]
fn more_pieces_than_rows_still_counts_every_row_once() {
    let file = b"COPY public.t (a) FROM stdin;\n1\n2\n3\n\\.\n\nSELECT 1;\n".to_vec();
    let block = &reference(&file)[0];
    assert_eq!(block.interior.end.row_count, 3);
    for pieces in 1..=40 {
        assert_eq!(split(&file, block, pieces).as_ref(), Some(&block.interior), "{pieces}");
    }
}

/// A window that does not reach the terminator answers `None` rather than
/// closing the block short, which is what tells the leader to hand out more
/// of the interior. The limit lands one byte into the first row.
#[test]
fn a_window_short_of_the_terminator_closes_nothing() {
    let file = b"COPY public.t (a) FROM stdin;\n1\n2\n3\n\\.\n".to_vec();
    let block = &reference(&file)[0];
    let piece = scan_piece(
        &file[block.data_offset as usize..],
        block.data_offset,
        PieceEntry::RowStart,
        block.data_offset + 1,
        plan(block.columns).as_ref(),
        None,
    )
    .unwrap();
    assert_eq!(piece.terminator, None);
    assert_eq!(piece.rows, 1);
    assert_eq!(piece.through, block.data_offset + 2);
    assert_eq!(merge(std::slice::from_ref(&piece)), None);
}

/// **A `\.` line past the block's end is not a terminator**, an earlier
/// piece having found the real one: `merge` never reaches past it.
#[test]
fn a_later_pieces_false_terminator_is_never_taken() {
    let file = b"COPY public.t (a) FROM stdin;\n1\n\\.\nSELECT 1;\n\\.\nSELECT 2;\n".to_vec();
    let block = &reference(&file)[0];

    let real = scan_piece(
        &file[block.data_offset as usize..],
        block.data_offset,
        PieceEntry::RowStart,
        block.end_offset,
        plan(block.columns).as_ref(),
        None,
    )
    .unwrap();
    // The piece after the block reads the trailing `\.` as a terminator.
    let after = scan_piece(
        &file[block.end_offset as usize..],
        block.end_offset,
        PieceEntry::RowStart,
        file.len() as u64,
        plan(block.columns).as_ref(),
        None,
    )
    .unwrap();
    assert!(after.terminator.is_some(), "the fixture must offer a false terminator");

    assert_eq!(merge(&[real, after]).as_ref(), Some(&block.interior));
}

/// The census is a union over the pieces, which is what makes a mixed
/// column resolvable: `{1,2}` in one piece and `{{1,2}}` in another records
/// `(1, 2)`, where either alone records a wrong single depth.
#[test]
fn the_census_unions_across_pieces() {
    let file = b"COPY public.t (a) FROM stdin;\n{1,2}\n{{1,2},{3,4}}\n\\.\n".to_vec();
    let block = &reference(&file)[0];
    assert_eq!(block.interior.census.shapes[0].dims, Some((1, 2)));

    let split = split(&file, block, 2).unwrap();
    assert_eq!(split.census.shapes[0].dims, Some((1, 2)));
    assert_eq!(&split, &block.interior);
}

/// **The count sums across pieces**, each value counted by the one piece
/// that owns its row, so a split counts what the serial pass counts.
#[test]
fn the_count_sums_across_pieces() {
    let file =
        b"COPY public.t (a, b) FROM stdin;\ninfinity\tx\n1\tinfinity\ninfinity\tinfinity\n2\t3\n\\.\n"
            .to_vec();
    let block = &reference(&file)[0];
    let counts: Vec<u64> =
        block.interior.census.unrepresentable.iter().map(|c| c.format()).collect();
    assert_eq!(counts, [2, 2]);
    for pieces in 2..=4 {
        let (split, working) = cut_up(&file, block, pieces);
        assert!(working > 1, "a real split");
        assert_eq!(split.as_ref(), Some(&block.interior), "{pieces} pieces");
    }
}

/// **A block at the metadata level is counted and never censused**: its
/// pieces, handed no width, count the rows a censused split counts and
/// fold none of them (`docs/design/decisions.md`, "D35").
#[test]
fn a_split_handed_no_width_counts_its_rows_and_censuses_none() {
    let file = b"COPY public.t (a) FROM stdin;\n{1}\n{{2}}\n{3}\n{4}\n\\.\n".to_vec();
    let block = Reference { columns: None, ..reference(&file).remove(0) };
    let (split, working) = cut_up(&file, &block, 2);
    let split = split.unwrap();
    assert_eq!(working, 2, "a real split");
    assert_eq!(split.census, BlockCensus::default());
    assert_eq!(split.end, block.interior.end);
}

/// **A piece never hands the scanner a mid-row byte**, and this is the row
/// that would be misread if it did: `a\.`, which COPY TEXT writes as
/// `a\\.`, so the tail from its second backslash is `\.` and an LF. I7's
/// guarantee is about a *line start*, so a piece entered there would close
/// the block early; the resync is a real forward read to the next LF.
#[test]
fn a_cut_inside_a_row_whose_tail_is_a_backslash_dot_is_resynced_past() {
    let file = b"COPY public.t (a) FROM stdin;\na\\\\.\n\\.\n".to_vec();
    let block = &reference(&file)[0];
    assert_eq!(block.interior.end.row_count, 1);
    // From the row's second backslash the bytes read `\.\n` — a
    // terminator line to anyone entering mid-row.
    let cut = block.data_offset + 2;
    assert_eq!(&file[cut as usize..cut as usize + 3], b"\\.\n");

    let piece = scan_piece(
        &file[cut as usize..],
        cut,
        PieceEntry::Resync,
        block.end_offset,
        plan(Some(1)).as_ref(),
        None,
    )
    .unwrap();
    assert_eq!(
        piece.terminator,
        Some((block.interior.end.terminator_offset, block.end_offset)),
        "the resync must skip the row's tail and find the real terminator"
    );
    assert_eq!(piece.rows, 0);

    for pieces in 1..=12 {
        assert_eq!(split(&file, block, pieces).as_ref(), Some(&block.interior), "{pieces}");
    }
}

/// The byte counts a fixture-sized block has to be scheduled against.
///
/// A dump fixture is kilobytes and the source's own partition is several
/// read chunks (`pgdt_io`'s `PLAIN_PARTITION_CHUNKS`), so at the shipped
/// chunk size every block here sits inside one partition and the scheduler
/// declines it. Announcing 8 bytes as the read size makes the source
/// advise 64-byte partitions, which is what exercises the window loop, the
/// tail read and the growth path at all. **It is bounded below by the
/// smallest region any fixture here ends with**: [`scan_region`]'s floor
/// is one whole partition, so a larger partition would decline a trailing
/// block and the sweeps below, which assert a closed region and a non-empty
/// read list for every block, would fail on it.
fn scheduled(source: &LocalFileSource, jobs: usize) -> ScanOptions {
    source.hint_read_size(8);
    ScanOptions {
        chunk_size_bytes: 8,
        parallelism: Parallelism::workers(jobs, pgdt_io::DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    }
}

/// **The same property [`a_split_interior_answers_what_the_serial_scanner_answers`]
/// asserts about the parse, asserted about the schedule**: the cut the
/// leader actually makes — the source's own boundaries, window size and
/// tail reads — is still invisible in the answer. At eight jobs against
/// the local source's four pool slots this also exercises the wait, the
/// run completing because a worker's read drops before its parse returns
/// (`ByteRangeSource::hint_wait_policy`).
#[tokio::test]
async fn a_scheduled_region_answers_what_the_serial_scanner_answers() {
    let files = [
        edge_cases(),
        fixture(16, "edge_cases", "default"),
        fixture(16, "types", "default"),
        fixture(13, "edge_cases", "default"),
    ];
    let mut blocks_checked = 0usize;
    for path in files {
        let file = std::fs::read(&path).unwrap();
        let size = file.len() as u64;
        let blocks = reference(&file);
        let source = LocalFileSource::open(&path).unwrap();
        for block in &blocks {
            for jobs in [2usize, 3, 8] {
                let options = scheduled(&source, jobs);
                let got = scan_region(
                    &source,
                    &options,
                    "public.t",
                    block.header_offset,
                    block.data_offset,
                    plan(block.columns),
                    size,
                    None,
                )
                .await
                .unwrap();
                assert_eq!(
                    got.scan,
                    RegionScan::Closed(block.interior.clone()),
                    "{} block at {} under {jobs} jobs",
                    path.display(),
                    block.data_offset
                );
            }
            blocks_checked += 1;
        }
    }
    assert!(blocks_checked > 0, "fixture discovery found no COPY blocks");
}

/// Answers exactly what it wraps, recording every read's length.
struct Recording {
    inner: LocalFileSource,
    reads: std::sync::Mutex<Vec<(u64, usize)>>,
    /// When set, the advice is restated as
    /// [`pgdt_io::PartitionRead::Whole`] at this unit — the only way to
    /// put a `Whole` piece *wider* than a unit in front of the leader.
    unit: Option<u64>,
}

impl pgdt_io::ByteRangeSource for Recording {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdt_core::Result<bytes::Bytes>> + Send + '_>,
    > {
        self.reads.lock().unwrap().push((offset, len));
        self.inner.read_range(offset, len)
    }
    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdt_core::Result<u64>> + Send + '_>>
    {
        self.inner.size()
    }
    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdt_core::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        self.inner.modified()
    }
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }
    fn partitions(&self, range: std::ops::Range<u64>) -> pgdt_io::Partitioning {
        let advice = self.inner.partitions(range);
        match self.unit {
            Some(unit) => advice.reading(PartitionRead::Whole { unit }),
            None => advice,
        }
    }
}

/// **No first read a worker makes at an offset on a
/// `PartitionRead::Chunked` source exceeds `ScanOptions::chunk_size_bytes`** — the
/// first read of a piece as much as the tail; a read grown to finish a
/// longer line is held to `max_line_bytes` alone. It is what keeps every
/// buffer such a worker takes but a grown one a *pooled* one
/// (`pgdt_io`'s `BufferPool::keeps` admits the announced length and nothing
/// longer). It is **not** source-agnostic: the block-decoding path reads a
/// piece whole, where the bound that binds is the source's own unit
/// ([`no_read_a_worker_makes_exceeds_the_stated_unit`]) — evidence
/// `parallel-peak-rss`, decision `docs/design/decisions.md`, "D9". The
/// assertion is on the reads the leader issued, not on the advice.
#[tokio::test]
async fn no_read_a_worker_makes_exceeds_the_chunk_size() {
    let path = edge_cases();
    let file = std::fs::read(&path).unwrap();
    let size = file.len() as u64;
    let blocks = reference(&file);
    let inner = LocalFileSource::open(&path).unwrap();
    let source = Recording { inner, reads: std::sync::Mutex::new(Vec::new()), unit: None };
    let mut scheduled_blocks = 0usize;
    for block in &blocks {
        for jobs in [2usize, 3, 8] {
            source.reads.lock().unwrap().clear();
            let options = scheduled(&source.inner, jobs);
            let got = scan_region(
                &source,
                &options,
                "public.t",
                block.header_offset,
                block.data_offset,
                plan(block.columns),
                size,
                None,
            )
            .await
            .unwrap();
            if matches!(got.scan, RegionScan::Closed(_)) {
                scheduled_blocks += 1;
            }
            let reads = source.reads.lock().unwrap();
            assert!(!reads.is_empty(), "a scheduled region reads something");
            // **The first read at any offset is the one this is about.**
            // The growth path is the one legitimate way past the chunk and
            // is never the first read at its offset, while a piece's own
            // opening read is unless the piece before it read from that
            // offset too, which then holds it to the growth ceiling alone
            // — so the invariant needs no arithmetic on the lengths.
            let mut seen: Vec<u64> = Vec::new();
            for &(offset, len) in reads.iter() {
                if seen.contains(&offset) {
                    assert!(
                        len <= options.max_line_bytes,
                        "a {len}-byte read at {offset} is past the growth ceiling"
                    );
                    continue;
                }
                seen.push(offset);
                assert!(
                    len <= options.chunk_size_bytes,
                    "the first read at {offset} under {jobs} jobs is {len} bytes, past the \
                     {}-byte chunk",
                    options.chunk_size_bytes
                );
            }
        }
    }
    assert!(scheduled_blocks > 0, "no region was actually scheduled");
}

/// **No read a worker makes on a `PartitionRead::Whole` source exceeds the
/// unit that source stated**, for a piece one unit covers. A longer piece
/// is read on past its first read a chunk at a time, so what this asserts
/// of the rest is `chunk_size`, not the unit; the fixture here runs a
/// chunk far below the unit, which is the case it covers. It is
/// what bounds the body buffer on the block-decoding path, where a
/// `Whole` read is capped at `min(piece, unit)` and a cut is one unit wide
/// (`pgdt_io`'s `BOUNDARIED_PARTITION_UNITS`, and
/// `docs/design/decisions.md`, "D8"). Asserted over a **plain** file restated as
/// `Whole`, because on the one source that says `Whole` today
/// `min(piece, unit)` is `piece` and the bound is vacuous.
///
/// It does **not** assert that the read is poolable:
/// `pgdt_io`'s `BufferPool::keeps` admits the announced chunk and nothing
/// longer, at either width.
#[tokio::test]
async fn no_read_a_worker_makes_exceeds_the_stated_unit() {
    const UNIT: u64 = 24;
    let path = edge_cases();
    let file = std::fs::read(&path).unwrap();
    let size = file.len() as u64;
    let blocks = reference(&file);
    let inner = LocalFileSource::open(&path).unwrap();
    let source = Recording { inner, reads: std::sync::Mutex::new(Vec::new()), unit: Some(UNIT) };
    let mut scheduled_blocks = 0usize;
    let mut wide_pieces = 0usize;
    for block in &blocks {
        for jobs in [2usize, 3, 8] {
            source.reads.lock().unwrap().clear();
            let options = scheduled(&source.inner, jobs);
            let got = scan_region(
                &source,
                &options,
                "public.t",
                block.header_offset,
                block.data_offset,
                plan(block.columns),
                size,
                None,
            )
            .await
            .unwrap();
            if matches!(got.scan, RegionScan::Closed(_)) {
                scheduled_blocks += 1;
            }
            // The piece is wider than the stated unit, so an unbounded
            // `Whole` would read past `UNIT`: the assertion is not vacuous.
            let advice = source.partitions(block.data_offset..size);
            if advice.partition_bytes() > UNIT {
                wide_pieces += 1;
            }
            let reads = source.reads.lock().unwrap();
            assert!(!reads.is_empty(), "a scheduled region reads something");
            // Same shape as the chunked bound above: a repeat at one
            // offset is the growth retry, ceilinged at `max_line_bytes`.
            let mut seen: Vec<u64> = Vec::new();
            for &(offset, len) in reads.iter() {
                if seen.contains(&offset) {
                    assert!(
                        len <= options.max_line_bytes,
                        "a {len}-byte read at {offset} is past the growth ceiling"
                    );
                    continue;
                }
                seen.push(offset);
                assert!(
                    len as u64 <= UNIT,
                    "the first read at {offset} under {jobs} jobs is {len} bytes, past the \
                     {UNIT}-byte unit"
                );
            }
        }
    }
    assert!(scheduled_blocks > 0, "no region was actually scheduled");
    assert!(wide_pieces > 0, "no piece was wider than the stated unit");
}

/// **Two ways to be declined, and neither is a branch on the source's
/// type**: a caller that states no parallelism keeps the serial path, and a
/// region too small for one of the source's own partitions is left alone
/// however many jobs were stated.
#[tokio::test]
async fn a_region_too_small_to_cut_is_left_to_the_serial_scanner() {
    let path = fixture(16, "types", "default");
    let file = std::fs::read(&path).unwrap();
    let size = file.len() as u64;
    let block = &reference(&file)[0];
    let source = LocalFileSource::open(&path).unwrap();

    let serial = scheduled(&source, 1);
    assert!(serial.parallelism.is_serial(), "one job is the serial value, not a Workers of 1");
    let got = scan_region(
        &source,
        &serial,
        "public.t",
        block.header_offset,
        block.data_offset,
        plan(block.columns),
        size,
        None,
    )
    .await
    .unwrap();
    assert_eq!(got.scan, RegionScan::Declined);

    let shipped = ScanOptions {
        parallelism: Parallelism::workers(8, pgdt_io::DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    };
    source.hint_read_size(shipped.chunk_size_bytes);
    let got = scan_region(
        &source,
        &shipped,
        "public.t",
        block.header_offset,
        block.data_offset,
        plan(block.columns),
        size,
        None,
    )
    .await
    .unwrap();
    assert_eq!(got.scan, RegionScan::Declined, "a kilobyte block against a shipped partition");
}

/// Cancellation is answered between windows, and is neither a closed block
/// nor an error: the caller banks the map it had before the region.
#[tokio::test]
async fn a_cancelled_region_closes_nothing_and_raises_nothing() {
    let path = fixture(16, "types", "default");
    let file = std::fs::read(&path).unwrap();
    let size = file.len() as u64;
    let block = &reference(&file)[0];
    let source = LocalFileSource::open(&path).unwrap();
    let options = ScanOptions { cancel: Some(cancelled()), ..scheduled(&source, 4) };
    let got = scan_region(
        &source,
        &options,
        "public.t",
        block.header_offset,
        block.data_offset,
        plan(block.columns),
        size,
        None,
    )
    .await
    .unwrap();
    assert_eq!(got.scan, RegionScan::Cancelled);
}

/// **A count asked for is not a count delivered, and this is the only
/// party that knows the difference** (`docs/design/decisions.md`, "D64").
/// It is answered whatever became of the region, so a block too small to
/// cut still carries the arrangement-wide fact.
#[tokio::test]
async fn a_budget_affording_fewer_readers_than_asked_for_answers_a_shortfall() {
    let path = fixture(16, "types", "default");
    let file = std::fs::read(&path).unwrap();
    let size = file.len() as u64;
    let block = &reference(&file)[0];
    let source = LocalFileSource::open(&path).unwrap();
    source.hint_read_size(8);
    // `scheduled` announces an 8-byte chunk, so a partition is 64 bytes,
    // and a 128-byte budget buys two readers where four were asked for.
    let advice = source.partitions(block.data_offset..size);
    assert_eq!(advice.partition_bytes(), 64, "the fixture's own partition size moved");

    let options =
        ScanOptions { parallelism: Parallelism::workers(4, 128), ..scheduled(&source, 4) };
    let got = scan_region(
        &source,
        &options,
        "public.t",
        block.header_offset,
        block.data_offset,
        plan(block.columns),
        size,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        got.shortfall,
        Some(Shortfall {
            asked: 4,
            delivered: 2,
            bound_by: BoundBy::Budget,
            // What the four asked for would have held — the source's own
            // per-worker term.
            would_hold_bytes: Some(64 * 4),
        }),
    );
}

/// **Silence is the claim that the announced count was dispatched**, so the
/// two arrangements that deliver what was asked answer no shortfall: one
/// reader asked for, and a budget affording every reader asked for. The
/// region floor is not a shortfall either ([`Shortfall`]), which the
/// second case here pins.
#[tokio::test]
async fn an_arrangement_that_delivers_what_was_asked_answers_no_shortfall() {
    let path = fixture(16, "types", "default");
    let file = std::fs::read(&path).unwrap();
    let size = file.len() as u64;
    let block = &reference(&file)[0];
    let source = LocalFileSource::open(&path).unwrap();

    let serial = scheduled(&source, 1);
    let got = scan_region(
        &source,
        &serial,
        "public.t",
        block.header_offset,
        block.data_offset,
        plan(block.columns),
        size,
        None,
    )
    .await
    .unwrap();
    assert_eq!(got.scan, RegionScan::Declined, "one job is the serial path");
    assert_eq!(got.shortfall, None, "one reader asked for is one reader delivered");

    // A shipped-size partition against a file of kilobytes: declined by
    // the floor, a fact about where the leader stands and not about the
    // arrangement.
    let shipped = ScanOptions {
        parallelism: Parallelism::workers(8, pgdt_io::DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    };
    source.hint_read_size(shipped.chunk_size_bytes);
    let got = scan_region(
        &source,
        &shipped,
        "public.t",
        block.header_offset,
        block.data_offset,
        plan(block.columns),
        size,
        None,
    )
    .await
    .unwrap();
    assert_eq!(got.scan, RegionScan::Declined);
    assert_eq!(got.shortfall, None, "a region too small to cut is not a shortfall");
}

/// **Only the leader may say a block is unterminated**, the other half of
/// `scan_piece` never claiming end of file. The offset named is the
/// header's, as the serial scanner names it too.
#[tokio::test]
async fn a_block_the_file_ends_inside_is_unterminated() {
    let mut file = b"COPY public.t (a) FROM stdin;\n".to_vec();
    let data_offset = file.len() as u64;
    for i in 0..500 {
        file.extend_from_slice(format!("{i}\n").as_bytes());
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("truncated.sql");
    std::fs::write(&path, &file).unwrap();

    let source = LocalFileSource::open(&path).unwrap();
    let options = scheduled(&source, 4);
    let err = scan_region(
        &source,
        &options,
        "public.t",
        0,
        data_offset,
        plan(Some(1)),
        file.len() as u64,
        None,
    )
    .await
    .expect_err("a COPY block with no terminator");
    assert!(
        matches!(err, Error::UnterminatedCopyBlock { header_offset: 0 }),
        "unexpected error: {err}"
    );
}

/// A row longer than the tail read is read by growing the read, bounded by
/// the same ceiling the serial loop enforces.
#[tokio::test]
async fn a_row_longer_than_a_tail_read_is_still_one_row() {
    let wide = "x".repeat(4096);
    let file = format!("COPY public.t (a) FROM stdin;\n{wide}\n{wide}\n{wide}\n\\.\n\nSELECT 1;\n")
        .into_bytes();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.sql");
    std::fs::write(&path, &file).unwrap();
    let block = &reference(&file)[0];
    assert_eq!(block.interior.end.row_count, 3);

    let source = LocalFileSource::open(&path).unwrap();
    for jobs in [2usize, 4, 8] {
        let options = scheduled(&source, jobs);
        let got = scan_region(
            &source,
            &options,
            "public.t",
            block.header_offset,
            block.data_offset,
            plan(block.columns),
            file.len() as u64,
            None,
        )
        .await
        .unwrap();
        assert_eq!(got.scan, RegionScan::Closed(block.interior.clone()), "{jobs} jobs");
    }
}

/// **A split block refuses the row a serial scan refuses** (I91): the
/// first row ending otherwise than the block's first, named by `COPY`'s
/// line and its offset, whichever piece it falls in and whether or not it
/// starts one; and the DDL after the terminator, scanned as rows by a piece
/// past the block, is held to nothing — a literal's raw CR included (I90).
#[tokio::test]
async fn a_split_block_refuses_the_row_a_serial_scan_refuses() {
    const TAIL: &[u8] = b"\\.\n\nCREATE TYPE public.e AS ENUM (\n    'a\r\nb'\n);\n";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("endings.sql");
    let header = b"COPY public.t (a) FROM stdin;\n";
    for (block, odd, refusal) in [
        ("\n", "\r\n", pgdt_core::RowEndingRefusal::LiteralCarriageReturn),
        ("\r\n", "\n", pgdt_core::RowEndingRefusal::LiteralNewline),
    ] {
        for odd_row in [None, Some(1u64), Some(37), Some(150), Some(299)] {
            let mut file = header.to_vec();
            let mut odd_offset = 0u64;
            for i in 0..300u64 {
                if Some(i) == odd_row {
                    odd_offset = file.len() as u64;
                }
                let ending = if Some(i) == odd_row { odd } else { block };
                file.extend_from_slice(format!("{i}{ending}").as_bytes());
            }
            file.extend_from_slice(TAIL);
            std::fs::write(&path, &file).unwrap();
            let source = LocalFileSource::open(&path).unwrap();
            for jobs in [2usize, 3, 8] {
                let options = scheduled(&source, jobs);
                let got = scan_region(
                    &source,
                    &options,
                    "public.t",
                    0,
                    header.len() as u64,
                    plan(Some(1)),
                    file.len() as u64,
                    None,
                )
                .await;
                let context = format!("{block:?} block, odd row {odd_row:?}, {jobs} jobs");
                match (odd_row, got) {
                    (None, Ok(outcome)) => {
                        assert_eq!(
                            outcome.scan,
                            RegionScan::Closed(reference(&file)[0].interior.clone()),
                            "{context}"
                        );
                    }
                    (
                        Some(row),
                        Err(Error::RowEndingRefused { table, line, line_offset, refusal: why }),
                    ) => {
                        assert_eq!(
                            (table.as_str(), line, line_offset, why),
                            ("public.t", row + 1, odd_offset, refusal),
                            "{context}"
                        );
                    }
                    (_, other) => panic!("{context}: {other:?}"),
                }
            }
        }
    }
}
