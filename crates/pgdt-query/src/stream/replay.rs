//! Pass 2, for one sub-stream: replaying its segments in file order, and a
//! block made active — at a `COPY` header the scanner read, inside a block
//! where a partition starts, or where a token resumes — with what the loop
//! records of where it is.

pub(super) mod dynamic;
pub(super) mod partition;
pub(super) mod plan;
pub(super) mod segment;

use std::collections::VecDeque;
use std::sync::Arc;

use arrow::array::RecordBatch;
use async_stream::try_stream;
use futures::Stream;

use pgdt_codec::ResolvedSchema;
use pgdt_core::CopyHeader;
use pgdt_core::Result;
use pgdt_core::internal::{RawRow, RowSplit, validated_prefix};
use pgdt_dump::internal::announce_read_loop;
use pgdt_dump::{ChunkedScanner, CopyScanner, Event, Row};
use pgdt_io::ByteRangeSource;

use self::dynamic::{DynamicRead, Skip};
use self::plan::{ReplayPlan, resolve_for_query};
use self::segment::{Segment, SegmentEntry, first_row_start};
use super::note::EarlyStop;
use super::resume::{InCopyResume, ResumeToken};
use super::table::StreamShared;
use crate::batch::{RetainedChunks, RowBatcher};
use crate::predicate::{ComparisonNote, ResolvedExpr};

/// Pass 2, for one sub-stream: replay `segments` in file order, in whatever
/// batches `plan.query_options` asks for. This is the whole of the row path: a
/// serial [`table_stream`](crate::stream::table_stream) is this function over one segment per kept run of
/// each matching block, a partitioned replay it over each group
/// [`plan_partitions`](self::partition::plan_partitions) handed out.
///
/// **Under a [`DynamicFilter`](crate::stream::replay::dynamic::DynamicFilter)** (`dynamic`), each row group the replay
/// enters is first asked of the filter's state ([`DynamicRead::at_row`]), and
/// the group being read is asked again where the state has moved by the next
/// chunk the replay takes ([`DynamicRead::at_chunk`]): one it rules out ends
/// the segment there, and the replay re-enters the block at the next group it
/// keeps as a piece starting inside the data, the segment's limit unchanged.
/// Under [`RowEvaluation::On`](crate::stream::replay::dynamic::RowEvaluation::On), a row the static filter keeps is evaluated
/// against the state before a column of the row decodes, in every block, and
/// dropped where it rejects it ([`DynamicRead::rejects`]). A block sorted on a
/// bound the state requires stops at its first row past it, as under a static
/// filter's.
pub(super) fn replay<'a>(
    source: &'a dyn ByteRangeSource,
    plan: Arc<ReplayPlan>,
    segments: Vec<Segment>,
    shared: StreamShared,
    resume: Option<ResumeToken>,
    fingerprint: u64,
    mut dynamic: Option<DynamicRead>,
) -> impl Stream<Item = Result<RecordBatch>> + Send + 'a {
    try_stream! {
        let read = plan.read;
        let query_options = &plan.query_options;
        let semantics = query_options.semantics;

        // The chunk length every segment's replay repeats, announced once for
        // the whole sub-stream. The budget comes from `QueryOptions`, the plan
        // holding only a `ReadOptions`: a cold query states the two passes'
        // parallelism separately because they split differently. **The replay loop could not grant a
        // wait** (`docs/design/decisions.md`, "D5"): a batch's
        // `StringViewBuilder` holds every chunk it has taken a `Utf8View` into
        // until it flushes, so this loop can never be the task that frees one
        // it waits on. Stated rather than left to the default, so whatever the
        // mapping pass granted is un-stated on the same source.
        announce_read_loop(source, read, query_options.parallelism);

        let mut rows_emitted = resume.as_ref().map_or(0, |t| t.rows_emitted);
        // Where a token paused inside a block, and which block: its scanner
        // resumes the first segment only where that segment holds the pause.
        let paused_in = resume
            .as_ref()
            .and_then(|t| t.in_copy.as_ref().map(|in_copy| (t.offset, in_copy.header_offset)));

        // Only the first segment can start mid-row (a resumed stream paused
        // between two of one block's rows); its scanner and in-flight batcher
        // are prebuilt here.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) if token.in_copy.is_some() => {
                let (scanner, opened) = resume_state(token, &plan)?;
                let active = opened.map(|(active, resolved, notes)| {
                    *shared.resolved_schema.lock().unwrap() = resolved;
                    *shared.comparison_notes.lock().unwrap() = notes;
                    active
                });
                (active, Some(scanner))
            }
            _ => (None, None),
        };
        // One buffer for the whole replay: every row of a block has the same
        // width, so after the first it never grows again.
        let mut split = RowSplit::default();

        // Every block holding a stop gets its entry up front, so a drained
        // stream tells one whose stop never fired from one with none planned.
        // A block's segments are adjacent, being in file order.
        {
            let mut early_stops = shared.early_stops.lock().unwrap();
            for segment in &segments {
                let header_offset = segment.block.header_offset;
                if plan.stops.contains_key(&header_offset)
                    && early_stops.last().is_none_or(|last| last.header_offset != header_offset)
                {
                    early_stops.push(EarlyStop { header_offset, unread_bytes: None });
                }
            }
        }

        // A segment a dynamic filter cut short is followed by the rest of it
        // the filter keeps, re-entered ahead of the segments after it.
        let mut queue = VecDeque::from(segments);
        while let Some(segment) = queue.pop_front() {
            let block = &segment.block;
            let seg_start = segment.start;
            let seg_limit = segment.limit;
            let seg_end = block.end_offset;
            let block_database = plan.database.clone();
            let stop = plan.stops.get(&block.header_offset);
            if let Some(dynamic) = dynamic.as_mut() {
                dynamic.enter(block, &plan);
            }

            let resumes_here = paused_in.is_some_and(|(offset, header_offset)| {
                header_offset == block.header_offset
                    && segment.start < offset
                    && offset <= seg_limit
            });
            let resumed = first_scanner.take();
            if resumed.is_some() && !resumes_here {
                // The pause lies before this segment, and every row between
                // the two is in a group pruning skipped: the paused block
                // state holds nothing and is dropped.
                active = None;
            }
            let scanner = match resumed.filter(|_| resumes_here) {
                Some(scanner) => scanner,
                None => match segment.entry {
                    SegmentEntry::Header => CopyScanner::resume(segment.start, None),
                    SegmentEntry::Interior => {
                        // Where this piece's first row starts — and whether
                        // it has one at all. A piece whose search lands past
                        // its own limit had two cuts fall inside one row: the
                        // row belongs to the piece before it.
                        let row_start =
                            first_row_start(source, segment.start, seg_end, read)
                                .await?
                                .filter(|&start| start <= seg_limit && start < seg_end);
                        // No header line is in range, so the schema comes off
                        // the map's own copy of it.
                        let (opened, resolved, notes) = activate(
                            block.header.clone(),
                            block.header_offset,
                            block_database.clone(),
                            &plan,
                        )?;
                        // Published even for a piece holding no row, so a
                        // sub-stream handed only such pieces — cuts inside
                        // one row, or the runs pruning left — reports the
                        // block's schema as one entered at its header does.
                        *shared.comparison_notes.lock().unwrap() = notes;
                        *shared.resolved_schema.lock().unwrap() = resolved;
                        let Some(row_start) = row_start else { continue };
                        let skip = dynamic.as_mut().map_or(Skip::Read, |dynamic| {
                            dynamic.at_row(row_start, seg_start, seg_limit, semantics)
                        });
                        match skip {
                            Skip::Read => {}
                            Skip::To(start) => {
                                queue.push_front(segment.rest_from(start));
                                continue;
                            }
                            Skip::Rest => continue,
                        }
                        active = Some(opened);
                        CopyScanner::resume(row_start, Some((block.header_offset, 0)))
                    }
                },
            };
            let mut chunked = ChunkedScanner::new(scanner, seg_end, read);
            let mut chunks = RetainedChunks::new();
            // Set once this segment has read the line that ends at or past
            // its limit — the last it owns, and the one the next segment's
            // search skips, so the two tile — or a row past its block's
            // sorted bound, after which it owns none the filter keeps, or the
            // start of a group a dynamic filter rules out. A later segment of
            // the same block stops at its own first row, where pruning has not
            // already skipped it.
            let mut past_limit = false;
            // Where the row that stopped this segment ends, if one did.
            let mut stopped_at: Option<u64> = None;
            // Where the rest of this segment is searched from, where a
            // dynamic filter ruled out the group its next row starts.
            let mut rest_from: Option<u64> = None;

            loop {
                // The state is read again at each chunk taken, where it has
                // moved, and a group it now rules out ends the segment before
                // the chunk is read (`docs/design/decisions.md`, "D93").
                if let Some(dynamic) = dynamic.as_mut() {
                    match dynamic.at_chunk(
                        chunked.scanner().position(),
                        seg_start,
                        seg_limit,
                        semantics,
                    ) {
                        Skip::Read => {}
                        Skip::To(start) => {
                            rest_from = Some(start);
                            break;
                        }
                        Skip::Rest => break,
                    }
                }
                let mut chunk = chunked.next_chunk(source).await?;
                let (offset, bytes) = chunk.read();
                if !bytes.is_empty() {
                    chunks.retain(offset, bytes);
                }
                // The rows of the span being scanned, validated as UTF-8 in one
                // pass rather than one `from_utf8` per field, and where the span
                // starts, which turns a row's file offset into a position in
                // them. **Taken on the first row that will decode something**,
                // so a query that decodes nothing pays nothing
                // (`docs/design/decisions.md`, "D27").
                let mut validated: Option<(u64, &str)> = None;
                while let Some(event) = chunk.next_event()? {
                    // Whether the scanner now stands at a row's start
                    // inside the block's data.
                    let at_row = matches!(event, Event::CopyStart(_) | Event::Row(_));
                    match event {
                        Event::CopyStart(start) => {
                            let (opened, resolved, notes) = activate(
                                start.header,
                                start.header_offset,
                                block_database.clone(),
                                &plan,
                            )?;
                            *shared.comparison_notes.lock().unwrap() = notes;
                            *shared.resolved_schema.lock().unwrap() = resolved;
                            active = Some(opened);
                        }
                        Event::Row(row) => {
                            if let Some((header_offset, _, batcher, filter, _)) =
                                active.as_mut()
                            {
                                let unchecked = RawRow::unchecked(row.raw);
                                let raw = if batcher.decodes_fields()
                                    || filter.reads_fields()
                                    || dynamic.as_ref().is_some_and(DynamicRead::reads_fields)
                                {
                                    let (span_base, span) = chunk.span();
                                    let prefix = match validated {
                                        Some((base, prefix)) if base == span_base => prefix,
                                        _ => {
                                            let prefix = validated_prefix(span);
                                            validated = Some((span_base, prefix));
                                            prefix
                                        }
                                    };
                                    row_text(prefix, span_base, &row)
                                        .map_or(unchecked, RawRow::validated)
                                } else {
                                    unchecked
                                };
                                // One split per row, shared
                                // (`docs/design/decisions.md`, "D28").
                                split.restart();
                                let keep = filter.matches(
                                    raw,
                                    &mut split,
                                    batcher.table(),
                                    row.offset,
                                )?;
                                // A kept row a dynamic filter's state
                                // rejects is dropped before a column of
                                // it decodes (`DynamicRead::rejects`).
                                let rejected = keep
                                    && dynamic.as_ref().is_some_and(|dynamic| {
                                        dynamic.rejects(
                                            raw,
                                            &mut split,
                                            batcher.table(),
                                            row.offset,
                                        )
                                    });
                                // A row a filter keeps makes every term
                                // of its stop `True`, so each stop is
                                // asked only of a row its filter
                                // rejected — the dynamic one's in a
                                // group it is armed in
                                // (`DynamicPruning::arm`), and of every
                                // row there where its rows are not
                                // evaluated.
                                let dynamic_stop = dynamic.as_ref().and_then(DynamicRead::stop);
                                let unevaluated = dynamic
                                    .as_ref()
                                    .is_some_and(|dynamic| !dynamic.evaluates_rows());
                                let passed = if keep {
                                    (rejected || unevaluated)
                                        && dynamic_stop
                                            .is_some_and(|stop| stop.passed(raw, &mut split))
                                } else {
                                    stop.is_some_and(|stop| stop.passed(raw, &mut split))
                                        || dynamic_stop
                                            .is_some_and(|stop| stop.passed(raw, &mut split))
                                };
                                if passed {
                                    past_limit = true;
                                    stopped_at = Some(chunk.scanner().position());
                                } else if keep && !rejected {
                                    batcher.push_row(
                                        *header_offset,
                                        row.offset,
                                        raw,
                                        &mut split,
                                        &mut chunks,
                                    )?;
                                }
                                if batcher.should_flush() {
                                    let begins_at = batcher
                                        .batch_start()
                                        .unwrap_or_else(|| chunk.scanner().position());
                                    let batch = batcher.flush()?;
                                    chunks.invalidate_block_cache();
                                    rows_emitted += batch.num_rows() as u64;
                                    *shared.batch_offset.lock().unwrap() = begins_at;
                                    *shared.position.lock().unwrap() =
                                        snapshot(chunk.scanner(), &active, rows_emitted, fingerprint);
                                    yield batch;
                                }
                            }
                        }
                        Event::CopyEnd(_) => {
                            if let Some((_, _, mut batcher, _, _)) = active.take()
                                && !batcher.is_empty()
                            {
                                let begins_at = batcher
                                    .batch_start()
                                    .unwrap_or_else(|| chunk.scanner().position());
                                let batch = batcher.flush()?;
                                chunks.invalidate_block_cache();
                                rows_emitted += batch.num_rows() as u64;
                                *shared.batch_offset.lock().unwrap() = begins_at;
                                *shared.position.lock().unwrap() =
                                    snapshot(chunk.scanner(), &active, rows_emitted, fingerprint);
                                yield batch;
                            }
                        }
                        // A replay segment covers exactly one block, so
                        // the only non-row line in range is the `COPY`
                        // header itself, which arrives as `CopyStart`.
                        Event::Line(_) | Event::DollarQuoteEnd(_) => {}
                        Event::LargeObjectStart(_) | Event::LargeObjectEnd(_) => {}
                    }
                    // The line just consumed ended at or past this
                    // segment's limit, so it was its last. Checked after
                    // the event, the straddling row being *this*
                    // segment's.
                    if past_limit || chunk.scanner().position() > seg_limit {
                        past_limit = true;
                        break;
                    }
                    // The next row is this segment's: where it enters a
                    // group a dynamic filter rules out, the segment ends.
                    if at_row && let Some(dynamic) = dynamic.as_mut() {
                        match dynamic.at_row(
                            chunk.scanner().position(),
                            seg_start,
                            seg_limit,
                            semantics,
                        ) {
                            Skip::Read => {}
                            Skip::To(start) => {
                                rest_from = Some(start);
                                past_limit = true;
                                break;
                            }
                            Skip::Rest => {
                                past_limit = true;
                                break;
                            }
                        }
                    }
                }

                // Everything before the scanner's new position has already had
                // its chance to be referenced by a zero-copy view, so it is
                // safe to drop. Which chunks that releases is
                // `RetainedChunks`' rule, not this loop's.
                chunks.release_through(chunked.scanner().position());

                if past_limit || chunked.at_end() {
                    break;
                }
            }

            if let Some(stopped_at) = stopped_at {
                let owned_end = seg_limit.saturating_add(1).min(block.terminator_offset);
                let unread = owned_end.saturating_sub(stopped_at);
                if unread > 0 {
                    let mut early_stops = shared.early_stops.lock().unwrap();
                    record_unread(&mut early_stops, block.header_offset, unread);
                }
            }
            if let Some(start) = rest_from {
                queue.push_front(segment.rest_from(start));
            }

            // A segment that stopped at its limit rather than at the block's
            // `\.` has no `CopyEnd` to flush it, so it flushes here — and
            // clears the block state either way, the next segment possibly
            // being a different block with a different schema.
            if let Some((_, _, mut batcher, _, _)) = active.take()
                && !batcher.is_empty()
            {
                let begins_at =
                    batcher.batch_start().unwrap_or_else(|| chunked.scanner().position());
                let batch = batcher.flush()?;
                rows_emitted += batch.num_rows() as u64;
                *shared.batch_offset.lock().unwrap() = begins_at;
                *shared.position.lock().unwrap() =
                    snapshot(chunked.scanner(), &active, rows_emitted, fingerprint);
                yield batch;
            }
        }
    }
}

/// Add `unread` to the entry of `early_stops` for the block at
/// `header_offset`, adding the entry in file order where there is none: a
/// dynamic filter's stop is found as the replay runs, not planned.
fn record_unread(early_stops: &mut Vec<EarlyStop>, header_offset: u64, unread: u64) {
    match early_stops.iter().position(|e| e.header_offset >= header_offset) {
        Some(at) if early_stops[at].header_offset == header_offset => {
            *early_stops[at].unread_bytes.get_or_insert(0) += unread;
        }
        at => early_stops.insert(
            at.unwrap_or(early_stops.len()),
            EarlyStop { header_offset, unread_bytes: Some(unread) },
        ),
    }
}

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, `QueryOptions::filter` resolved against this block's
/// own schema (a table's blocks can order its columns differently), and the database this block
/// is attributed to. Each
/// [`ResolvedExpr`] leaf carries the field index it reads — into the block's
/// **unprojected** column list — plus the typed comparison it makes. Shared
/// with the block's [`PlannedBlock`](self::plan::PlannedBlock), so every piece of a block evaluates one
/// tree.
type Active = (u64, CopyHeader, RowBatcher, Arc<ResolvedExpr>, Option<String>);

/// What [`activate`] hands back: the block's state, and the schema and notes
/// a stream publishes for it.
type Opened = (Active, ResolvedSchema, Vec<ComparisonNote>);

/// Take one block's resolution from `plan` — resolving it here where the
/// plan holds none (`plan_blocks`) — and build the [`RowBatcher`] that will
/// hold its rows.
///
/// The three places a block becomes active — a `COPY` header the scanner
/// read, a partition that started inside a block, and a resumed token —
/// differ only in where the header comes from.
fn activate(
    header: CopyHeader,
    header_offset: u64,
    database: Option<String>,
    plan: &ReplayPlan,
) -> Result<Opened> {
    let resolved_here;
    let block = match plan
        .blocks
        .get(&header_offset)
        .filter(|planned| planned.database == database && planned.header == header)
    {
        Some(planned) => planned,
        None => {
            resolved_here = resolve_for_query(
                &header,
                header_offset,
                database.as_deref(),
                &plan.query_options,
                plan.metadata.as_deref(),
                &plan.table,
                None,
            )?;
            &resolved_here
        }
    };
    let batcher = RowBatcher::new(
        &block.resolved,
        header.qualified_name(),
        plan.query_options.clone(),
        block.field_targets.clone(),
        block.unrepresentable.clone(),
    );
    let (resolved, notes) = (block.resolved.clone(), block.notes.clone());
    Ok(((header_offset, header, batcher, Arc::clone(&block.filter), database), resolved, notes))
}

/// Reconstruct the in-progress block state a [`ResumeToken`] captured, if any:
/// the scanner's row counter (so a later `CopyEnd` reports the block's true
/// total) and a fresh [`RowBatcher`](crate::batch::RowBatcher) on the original block's schema.
fn resume_state(token: &ResumeToken, plan: &ReplayPlan) -> Result<(CopyScanner, Option<Opened>)> {
    let scanner = CopyScanner::resume(
        token.offset,
        token.in_copy.as_ref().map(|ic| (ic.header_offset, ic.rows_in_block)),
    );
    let active = token
        .in_copy
        .as_ref()
        .map(|ic| activate(ic.header.clone(), ic.header_offset, ic.database.clone(), plan))
        .transpose()?;
    Ok((scanner, active))
}

fn snapshot(
    scanner: &CopyScanner,
    active: &Option<Active>,
    rows_emitted: u64,
    query_fingerprint: u64,
) -> ResumeToken {
    let in_copy = active.as_ref().map(|(header_offset, header, _, _, database)| InCopyResume {
        header: header.clone(),
        header_offset: *header_offset,
        rows_in_block: scanner.in_copy_rows().unwrap_or(0),
        database: database.clone(),
    });
    ResumeToken {
        offset: scanner.position(),
        rows_emitted,
        query_fingerprint,
        generation: 0,
        in_copy,
    }
}

/// Where `row` sits inside `prefix`, the UTF-8-validated leading part of the
/// span it was scanned out of — `None` when the row runs past it, which is
/// every row of a span whose validation failed and none of one whose
/// validation held. `str::get` rather than an index, so a mis-derived offset
/// costs the row its fast path instead of panicking.
fn row_text<'a>(prefix: &'a str, span_base: u64, row: &Row<'_>) -> Option<&'a str> {
    let start = usize::try_from(row.offset.checked_sub(span_base)?).ok()?;
    prefix.get(start..start.checked_add(row.raw.len())?)
}

#[cfg(test)]
mod tests;
