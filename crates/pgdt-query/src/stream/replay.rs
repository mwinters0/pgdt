//! Pass 2, for one sub-stream: replaying its segments in file order, and a
//! block made active — at a `COPY` header the scanner read, inside a block
//! where a partition starts, or where a token resumes — with what the loop
//! records of where it is.

pub(super) mod dynamic;
pub(super) mod partition;
pub(super) mod plan;
pub(super) mod segment;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use async_stream::try_stream;
use futures::Stream;

use pgdt_core::CopyHeader;
use pgdt_core::Result;
use pgdt_core::internal::{RawRow, RowSplit, validated_prefix};
use pgdt_dump::internal::{Drawing, announce_read_loop};
use pgdt_dump::{ChunkedScanner, CopyScanner, Event, Row};
use pgdt_io::{Admission, ByteRangeSource, Holder, Ledger};
use pgdt_model::BlockId;

use self::dynamic::{DynamicRead, Skip};
use self::plan::{ReplayPlan, charged, resolve_for_query};
use self::segment::{Segment, SegmentEntry, first_row_start};
use super::note::EarlyStop;
use super::resume::{InCopyResume, ResumeToken};
use super::table::StreamShared;
use crate::batch::{RetainedChunks, RowBatcher};
use crate::predicate::ResolvedExpr;
use crate::prune::SortedStop;

/// Pass 2, for one sub-stream: replay `segments` in file order, in batches of
/// at most `max_rows` rows and whatever else `plan.query_options` bounds them
/// by. This is the whole of the row path: a serial resume is this function
/// over one segment per kept run of each matching block from the token on, a
/// partitioned replay it over each group
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
///
/// **A row nothing planned here reads a field of is streamed** once it
/// outgrows its chunk ([`reads_no_field`]): counted and held to its block's
/// width, its bytes never held and kept whatever `dynamic` reads, so a
/// replay whose plan reads no field holds no line at any length, as its
/// line charge assumes (`docs/design/decisions.md`, "D138", "D160").
///
/// **A line held past its chunk term draws on `ledger`** before its chunk is
/// absorbed (`docs/design/decisions.md`, "D158"), charged its bytes and what
/// the query's one factor says its batch decodes beside them
/// ([`ReplayPlan::factor`]). Its row ends its batch, and the draw is
/// returned before any batch is yielded: a stream suspended at a yield holds
/// nothing, so a caller holding it back stalls no other, and a cut before
/// the row draws again once the earlier rows are out
/// (`docs/design/decisions.md`, "D159").
///
/// **Its reader parks where the ledger asks it to**, as a parse's does: it
/// stands where it reads before absorbing each read, and one asked parks
/// there, dropping that read (its last one it still holds, `KD172`), and
/// emits nothing for it: the batch it holds rows of is kept, its text
/// columns' views into read chunks re-owned first
/// ([`RowBatcher::own_views`]), so its batches are cut where an unlimited
/// ledger's are. It resumes as what passes the room falls, a draw returned or
/// statistics freed; it stands aside before every yield, so a caller holding
/// it back at a batch stalls no line (`docs/design/decisions.md`, "D163").
// upstream: UF5 -- a plan may hold a yielded batch, and its line's copy, unreserved.
// deficiency: KD174 — the draw returned at the yield, a round-robin
// repartition above the scan keeps the batch's decoded copy of the line in
// its coalescer, one per output partition, until its output batch fills; at
// fewer scan partitions than `target_partitions` that is several long lines
// held at once outside every reservation and every carve, past what the
// margin absorbs on some cuts, so the process passes the allowance the
// arrangement was carved from. Billing every plan for `target_partitions`
// copies charges one holding none (UF5's rejections). **(c) unowned**;
// promoted by UF5's fix landing or a phase taking up `pgdt sql`'s resident.
#[allow(clippy::too_many_arguments)]
pub(super) fn replay<'a>(
    source: &'a dyn ByteRangeSource,
    plan: Arc<ReplayPlan>,
    ledger: Ledger,
    max_rows: usize,
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
        // the whole sub-stream. The budget is the replay's own arrangement's:
        // a cold query carves the two passes apart because they split
        // differently. **The replay loop could not grant a
        // wait** (`docs/design/decisions.md`, "D5"): a batch's
        // `StringViewBuilder` holds every chunk it has taken a `Utf8View` into
        // until it flushes or its reader parks, so this loop can never be the
        // task that frees one it waits on. Stated rather than left to the
        // default, so whatever the mapping pass granted is un-stated on the
        // same source.
        announce_read_loop(source, read, plan.parallelism);

        let mut emits =
            Emits::new(&shared, resume.as_ref().map_or(0, |t| t.rows_emitted), fingerprint);
        // Where a token paused inside a block, and which block: its scanner
        // resumes the first segment only where that segment holds the pause.
        let paused_in = resume
            .as_ref()
            .and_then(|t| t.in_copy.as_ref().map(|in_copy| (t.offset, in_copy.block)));

        // Only the first segment can start mid-row (a resumed stream paused
        // between two of one block's rows); its scanner and in-flight batcher
        // are prebuilt here.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) if token.in_copy.is_some() => {
                let (scanner, active) = resume_state(token, &plan, max_rows)?;
                (active, Some(scanner))
            }
            _ => (None, None),
        };
        // One buffer for the whole replay: every row of a block has the same
        // width, so it grows only at a block wider than any before.
        let mut split = RowSplit::default();
        // This sub-stream's draws, for one line at a time, each charged its
        // bytes and the query's decode beside them.
        let mut holder = ledger.holder();
        let factor = plan.factor();
        let charge = move |held: u64| charged(factor, held);

        // Every block holding a stop gets its entry up front, so a drained
        // stream tells one whose stop never fired from one with none planned.
        // A block's segments are adjacent, being in file order.
        {
            let mut early_stops = shared.early_stops.lock().unwrap();
            for segment in &segments {
                let block = segment.block.id();
                if plan.stops.contains_key(&block)
                    && early_stops.last().is_none_or(|last| last.block != block)
                {
                    early_stops.push(EarlyStop { block, unread_bytes: None });
                }
            }
        }

        // A segment a dynamic filter cut short is followed by the rest of it
        // the filter keeps, re-entered ahead of the segments after it.
        let mut queue = VecDeque::from(segments);
        while let Some(segment) = queue.pop_front() {
            let block = &segment.block;
            let id = block.id();
            let seg_start = segment.start;
            let seg_limit = segment.limit;
            let seg_end = block.end_offset;
            let block_database = plan.database.clone();
            let stop = plan.stops.get(&id);
            if let Some(dynamic) = dynamic.as_mut() {
                dynamic.enter(block, &plan);
            }

            let resumes_here = paused_in.is_some_and(|(offset, paused)| {
                paused == id
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
                        // it has one at all. A row starting past its limit is
                        // the next piece's, so the search ends there: a piece
                        // finding none had two cuts fall inside one row,
                        // which belongs to the piece before it.
                        let row_start = first_row_start(
                            source,
                            segment.start,
                            seg_limit.min(seg_end),
                            read.chunk(),
                        )
                        .await?
                        .filter(|&start| start <= seg_limit && start < seg_end);
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
                        // No header line is in range, so the block's state
                        // comes off the map's own copy of it.
                        active = Some(activate(
                            id,
                            block.header.clone(),
                            block.header_offset,
                            block_database.clone(),
                            &plan,
                            max_rows,
                        )?);
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
                // A reader asked to park parks holding no read but the one
                // before it (`KD172`; `docs/design/decisions.md`, "D162"), and
                // with its batch:
                // the read chunks its views point into are given up first.
                let parks = match active.as_mut() {
                    Some(open) if !open.batcher.is_empty() => {
                        let asked = holder.stand(chunked.read_pos());
                        if asked {
                            open.batcher.own_views(&mut chunks);
                        }
                        asked
                    }
                    _ => true,
                };
                let drawing = Drawing::charged(&mut holder, &charge);
                let drawing = if parks { drawing.parking() } else { drawing };
                let mut chunk = chunked
                    .next_chunk_folding(source, None, Some(drawing))
                    .await
                    .map_err(|e| e.in_copy_of(|| block.header.qualified_name()))?;
                // The line this chunk's carry pass completes, where it was
                // drawn for past its chunk term, and its charge: its row, the
                // chunk's first, ends its batch.
                let drawn = chunk
                    .held()
                    .map(|(start, _)| (start, holder.held()))
                    .filter(|&(_, charge)| charge > 0);
                if drawn.is_some()
                    && let Some(open) = active.as_mut()
                {
                    open.batcher.holds_drawn_line();
                }
                // Whether the line this chunk leaves for the next may be
                // streamed: said again each chunk, a block entered inside
                // its data having no header to say it at.
                if let Some(open) = active.as_ref()
                    && chunk.scanner().in_copy_block()
                {
                    chunk.stream_rows(reads_no_field(open, stop));
                }
                // The carried line first, being earlier in the file: a row of
                // the carry pass lies whole in it and in no read chunk.
                if let Some((start, line)) = chunk.held() {
                    chunks.retain(start, line);
                }
                let (offset, bytes) = chunk.read();
                if !bytes.is_empty() {
                    chunks.retain(offset, bytes);
                }
                let chunk_end = offset + bytes.len() as u64;
                // The rows of the span being scanned, validated as UTF-8 in one
                // pass rather than one `from_utf8` per field, and where the span
                // starts, which turns a row's file offset into a position in
                // them. **Taken on the first row that will decode something**,
                // so a query that decodes nothing pays nothing
                // (`docs/design/decisions.md`, "D27").
                let mut validated: Option<(u64, &str)> = None;
                while let Some(event) = chunk.next_event()? {
                    // Whether the scanner now stands at a row's start
                    // inside the block's data, and whether a row was taken.
                    let counted = matches!(event, Event::Row(_) | Event::StreamedRow(_));
                    let at_row = counted || matches!(event, Event::CopyStart(_));
                    match event {
                        Event::CopyStart(start) => {
                            let opened = activate(
                                start.id(),
                                start.header,
                                start.header_offset,
                                block_database.clone(),
                                &plan,
                                max_rows,
                            )?;
                            chunk.stream_rows(reads_no_field(&opened, stop));
                            active = Some(opened);
                        }
                        Event::Row(row) => {
                            if let Some(open) = active.as_mut() {
                                let unchecked = RawRow::unchecked(row.raw);
                                let raw = if open.batcher.decodes_fields()
                                    || open.filter.reads_fields()
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
                                let keep = open.filter.matches(
                                    raw,
                                    &mut split,
                                    open.batcher.table(),
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
                                            open.batcher.table(),
                                            row.offset,
                                        )
                                    });
                                // A row a filter keeps makes every term
                                // of its stop `True`, so the static stop
                                // is asked only of a row its filter
                                // rejected, and the dynamic one's, in a
                                // group it is armed in
                                // (`DynamicPruning::arm`), of a row
                                // either filter rejected and of every
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
                                    // A long nested value starts a batch of
                                    // its own (`RowBatcher::cuts_before`):
                                    // the rows before it are emitted, and a
                                    // pause there resumes at its row, which
                                    // is counted as read and is the next.
                                    if open.batcher.cuts_before(raw, &mut split) {
                                        let read = chunk.scanner().in_copy_rows().unwrap_or(1);
                                        let in_copy = open.in_copy(read - 1);
                                        yield emits.emit(
                                            &mut open.batcher,
                                            &mut chunks,
                                            &mut holder,
                                            row.offset,
                                            Some(in_copy),
                                        )?;
                                        // The row's line, still held, is
                                        // drawn for again, held once under
                                        // the floor, and its row marked
                                        // drawn again, the flush having
                                        // cleared the mark: its row ends
                                        // its batch whatever its value
                                        // reads as (`docs/design/decisions.md`,
                                        // "D159").
                                        if let Some((start, charge)) = drawn
                                            .filter(|&(start, _)| start == row.offset)
                                        {
                                            loop {
                                                match holder.draw(start, charge).await {
                                                    Admission::Held => break,
                                                    Admission::GiveUp => {}
                                                    Admission::Refused(floor) => {
                                                        Err(floor.refusal(start, charge))?;
                                                    }
                                                }
                                            }
                                            open.batcher.holds_drawn_line();
                                        }
                                    }
                                    open.batcher.push_row(
                                        open.header_offset,
                                        row.offset,
                                        raw,
                                        &mut split,
                                        &mut chunks,
                                    )?;
                                }
                            }
                        }
                        // A row nothing here reads a field of, counted as
                        // it went by (`reads_no_field`): its filter's answer
                        // is the tree's alone, and its fields are held to
                        // the block's width as a held row's are.
                        Event::StreamedRow(row) => {
                            if let Some(open) = active.as_mut()
                                && open.filter.matches_unread(open.batcher.table(), row.offset)?
                            {
                                open.batcher.push_counted_row(
                                    open.header_offset,
                                    row.offset,
                                    row.fields,
                                    row.len,
                                )?;
                            }
                        }
                        // The block is over, so its batch resumes outside it.
                        Event::CopyEnd(_) => {
                            if let Some(mut open) = active.take()
                                && !open.batcher.is_empty()
                            {
                                let at = chunk.scanner().position();
                                yield emits.emit(
                                    &mut open.batcher,
                                    &mut chunks,
                                    &mut holder,
                                    at,
                                    None,
                                )?;
                            }
                        }
                        // A replay segment covers exactly one block, so
                        // the only non-row lines in range are the `COPY`
                        // header, which arrives as `CopyStart`, and its
                        // `\.`, as `CopyEnd`.
                        Event::Line(_) | Event::DollarQuoteEnd(_) => {}
                        Event::LargeObjectStart(_) | Event::LargeObjectEnd(_) => {}
                    }
                    if counted
                        && let Some(open) = active.as_mut()
                        && open.batcher.should_flush()
                    {
                        let scanner = chunk.scanner();
                        let in_copy = open.in_copy(scanner.in_copy_rows().unwrap_or(0));
                        let at = scanner.position();
                        yield emits.emit(
                            &mut open.batcher,
                            &mut chunks,
                            &mut holder,
                            at,
                            Some(in_copy),
                        )?;
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

                // Every row of the chunk has had its chance to be referenced
                // by a zero-copy view, and what it leaves unconsumed is copied
                // into the carry, so nothing read so far is needed again —
                // a line many chunks long releases each as it is copied.
                // Which chunks that releases is `RetainedChunks`' rule, not
                // this loop's.
                chunks.release_through(chunk_end);

                if past_limit || chunked.at_end() {
                    break;
                }
            }
            // Whatever the segment's last line drew is returned with it.
            holder.release();

            if let Some(stopped_at) = stopped_at {
                let owned_end = seg_limit.saturating_add(1).min(block.terminator_offset);
                let unread = owned_end.saturating_sub(stopped_at);
                if unread > 0 {
                    let mut early_stops = shared.early_stops.lock().unwrap();
                    record_unread(&mut early_stops, id, unread);
                }
            }
            if let Some(start) = rest_from {
                queue.push_front(segment.rest_from(start));
            }

            // A segment that stopped at its limit rather than at the block's
            // `\.` has no `CopyEnd` to flush it, so it flushes here — and
            // clears the block state either way, the next segment possibly
            // being a different block with a different schema. Its token
            // resumes outside a block, as `CopyEnd`'s does.
            if let Some(mut open) = active.take()
                && !open.batcher.is_empty()
            {
                let at = chunked.scanner().position();
                yield emits.emit(&mut open.batcher, &mut chunks, &mut holder, at, None)?;
            }
        }
    }
}

/// Whether a row of `active`'s block may be streamed — counted, once a
/// whole chunk passes inside it, without its bytes being held: nothing the
/// plan asks of it reads a field — not its batch, its filter or its block's
/// sorted `stop`. **A dynamic filter is not asked**: its state arrives after
/// the line charge was carved (`docs/design/decisions.md`, "D160"), so a row
/// it would read a field of is streamed and kept unread and unstopped, which
/// its contract allows, a row it keeps never being wrong
/// (`docs/design/decisions.md`, "D138").
fn reads_no_field(active: &Active, stop: Option<&SortedStop>) -> bool {
    !active.batcher.decodes_fields() && !active.filter.reads_fields() && stop.is_none()
}

/// Add `unread` to the entry of `early_stops` for `block`, adding the entry
/// in file order where there is none: a dynamic filter's stop is found as the
/// replay runs, not planned.
fn record_unread(early_stops: &mut Vec<EarlyStop>, block: BlockId, unread: u64) {
    match early_stops.iter().position(|e| e.block >= block) {
        Some(at) if early_stops[at].block == block => {
            *early_stops[at].unread_bytes.get_or_insert(0) += unread;
        }
        at => early_stops.insert(
            at.unwrap_or(early_stops.len()),
            EarlyStop { block, unread_bytes: Some(unread) },
        ),
    }
}

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, `QueryOptions::filter` resolved against this
/// block's own schema (a table's blocks can order its columns differently),
/// and the database this block is attributed to.
struct Active {
    block: BlockId,
    /// Where the block's `COPY` header starts: its address, which a refusal
    /// of one of its rows names.
    header_offset: u64,
    header: CopyHeader,
    batcher: RowBatcher,
    /// Each [`ResolvedExpr`] leaf carries the field index it reads — into
    /// the block's **unprojected** column list — plus the typed comparison
    /// it makes. Shared with the block's
    /// [`PlannedBlock`](self::plan::PlannedBlock), so every piece of a block
    /// evaluates one tree.
    filter: Arc<ResolvedExpr>,
    database: Option<String>,
}

impl Active {
    /// Where a token pausing inside this block stands, `rows_in_block` of
    /// its rows read.
    fn in_copy(&self, rows_in_block: u64) -> InCopyResume {
        InCopyResume {
            header: self.header.clone(),
            block: self.block,
            header_offset: self.header_offset,
            rows_in_block,
            database: self.database.clone(),
        }
    }
}

/// Take one block's resolution from `plan` — resolving it here where the
/// plan holds none (`plan_blocks`) — and build the [`RowBatcher`] that will
/// hold its rows.
///
/// The three places a block becomes active — a `COPY` header the scanner
/// read, a partition that started inside a block, and a resumed token —
/// differ only in where the block's identity, header and address come from.
/// None publishes a schema: a
/// stream holds its plan's from before its first poll
/// ([`TableStream::resolved_schema`](crate::stream::TableStream::resolved_schema)).
fn activate(
    id: BlockId,
    header: CopyHeader,
    header_offset: u64,
    database: Option<String>,
    plan: &ReplayPlan,
    max_rows: usize,
) -> Result<Active> {
    let resolved_here;
    let block = match plan
        .blocks
        .get(&id)
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
        max_rows,
        plan.read.chunk(),
        block.field_targets.clone(),
        block.unrepresentable.clone(),
    );
    Ok(Active {
        block: id,
        header_offset,
        header,
        batcher,
        filter: Arc::clone(&block.filter),
        database,
    })
}

/// Reconstruct the in-progress block state a [`ResumeToken`] captured, if any:
/// the scanner's row counter (so a later `CopyEnd` reports the block's true
/// total) and a fresh [`RowBatcher`](crate::batch::RowBatcher) on the original block's schema.
fn resume_state(
    token: &ResumeToken,
    plan: &ReplayPlan,
    max_rows: usize,
) -> Result<(CopyScanner, Option<Active>)> {
    let scanner = CopyScanner::resume(
        token.offset,
        token.in_copy.as_ref().map(|ic| (ic.header_offset, ic.rows_in_block)),
    );
    let active = token
        .in_copy
        .as_ref()
        .map(|ic| {
            activate(
                ic.block,
                ic.header.clone(),
                ic.header_offset,
                ic.database.clone(),
                plan,
                max_rows,
            )
        })
        .transpose()?;
    Ok((scanner, active))
}

/// What a replay tells its owner as each batch goes: how many rows its
/// stream has emitted, where the batch begins and the token resuming past
/// it ([`TableStream::batch_source_offset`](crate::stream::TableStream::batch_source_offset),
/// [`TableStream::resume_token`](crate::stream::TableStream::resume_token)).
struct Emits {
    batch_offset: Arc<Mutex<u64>>,
    position: Arc<Mutex<ResumeToken>>,
    rows_emitted: u64,
    fingerprint: u64,
}

impl Emits {
    fn new(shared: &StreamShared, rows_emitted: u64, fingerprint: u64) -> Self {
        Self {
            batch_offset: Arc::clone(&shared.batch_offset),
            position: Arc::clone(&shared.position),
            rows_emitted,
            fingerprint,
        }
    }

    /// **The one step every batch is emitted by**: `batcher`'s batch
    /// flushed, the block indices `chunks` cached for its builders cleared,
    /// the line `holder` drew returned and its reader stood aside, so a
    /// caller holding the stream back at the yield stalls no line
    /// (`docs/design/decisions.md`, "D159", "D163"); then published. The
    /// sites differ only in `offset`, where the next row starts, and
    /// `in_copy`, the block a token resumes inside, `None` where the batch
    /// ends at its block's `\.` or at its segment's limit. A batch holding
    /// no row — a `max_rows` or `max_bytes` of 0 — begins at `offset`.
    fn emit(
        &mut self,
        batcher: &mut RowBatcher,
        chunks: &mut RetainedChunks,
        holder: &mut Holder,
        offset: u64,
        in_copy: Option<InCopyResume>,
    ) -> Result<RecordBatch> {
        let begins_at = batcher.batch_start().unwrap_or(offset);
        let batch = batcher.flush()?;
        chunks.invalidate_block_cache();
        holder.release();
        self.rows_emitted += batch.num_rows() as u64;
        *self.batch_offset.lock().unwrap() = begins_at;
        *self.position.lock().unwrap() = ResumeToken {
            offset,
            rows_emitted: self.rows_emitted,
            query_fingerprint: self.fingerprint,
            generation: 0,
            in_copy,
        };
        holder.stand_aside();
        Ok(batch)
    }
}

/// Where `row` sits inside `prefix`, the UTF-8-validated leading part of the
/// span it was scanned out of — `None` when the row runs past it, which is
/// every row of a span whose validation failed, an empty one at its start
/// aside, and none of one whose validation held. `str::get` rather than an index, so a mis-derived offset
/// costs the row its fast path instead of panicking.
fn row_text<'a>(prefix: &'a str, span_base: u64, row: &Row<'_>) -> Option<&'a str> {
    let start = usize::try_from(row.offset.checked_sub(span_base)?).ok()?;
    prefix.get(start..start.checked_add(row.raw.len())?)
}

#[cfg(test)]
mod tests;
