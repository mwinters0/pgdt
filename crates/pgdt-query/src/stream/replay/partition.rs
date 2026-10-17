//! How a replay is cut into sub-streams: the pieces a caller's parallelism
//! and the source allow, and their grouping into byte-balanced runs.

use std::collections::BTreeMap;
use std::ops::Range;

use pgdt_io::{
    ByteRangeSource, Parallelism, Partitioning, RetainedUnit, WorkerMemory,
    internal::{cut, worker_count},
};
use pgdt_model::CopyBlock;

use super::segment::{Segment, SegmentEntry};
use crate::stream::note::PlanNote;

/// Whether this source is a compressed one that *could* be read a
/// block at a time and is not, because the budget in force leaves no room to
/// hold a whole block ([`pgdt_io::Partitioning`], and
/// `docs/design/decisions.md`, "D15").
///
/// **Read off `partitions()`, not off a budget the caller would have to hand
/// down**: a source holding a seek table with more than one block and still
/// advising a *single* partition over the whole of it has declined. What the
/// source *is* asked for is the recourse the message names
/// ([`pgdt_io::ByteRangeSource::block_decode_bytes`]). **Asked over the
/// whole file**, since a query whose rows all sit inside one compressed block
/// would be advised one partition on the block path too. `None` for every
/// plain source and for a file with no more than one block, which is already
/// `DiagnosticKind::NonSeekableCompressedSource`.
fn compressed_block_path_declined(
    source: &dyn ByteRangeSource,
    parallelism: Parallelism,
) -> Option<PlanNote> {
    let table = source.seek_table()?;
    if !table.is_seekable() {
        return None;
    }
    if source.partitions(0..table.uncompressed_size()).max_partitions() != Some(1) {
        return None;
    }
    // The budget actually in force, which is what the source compared its
    // reader against: a caller that stated no number leaves every pool on
    // this one.
    let memory_bytes = parallelism.budget_bytes();
    Some(PlanNote::compressed_block_path_declined(
        table.block_count(),
        table.max_block_uncompressed(),
        // What a reader of this file would have held on the path it declined.
        // The fallback is unreachable, but nothing in the trait obliges the
        // two answers to agree.
        source.block_decode_bytes().unwrap_or(0),
        memory_bytes,
        Parallelism::allowance_raises_budget(source.default_worker_memory(), memory_bytes),
    ))
}

/// Split `matches` into the pieces `parallelism` and the source between them
/// allow, then group those pieces into sub-streams — each internally in file
/// order, and the groups themselves in file order, so concatenating them is
/// the serial replay (`docs/design/decisions.md`, "D51").
///
/// Never empty: a table with no blocks at all is one sub-stream that yields
/// nothing, which is what [`table_stream`](crate::stream::table_stream) does with the same map.
///
/// **`max_source_span` is the second term the stated budget is solved against,
/// not a separate cap of its own** (`docs/design/decisions.md`, "D4"). What one
/// sub-stream costs the caller is its held
/// batch's pin (`max_source_span`, rounded out to the retained unit) *on top
/// of* what the source charges a concurrent reader for decoding
/// (`partition_bytes`) — a discovery worker pays only the second, but a
/// query's sub-stream is handed its batch and pays both, which is why this
/// charge is not `worker_count`'s own to know and is computed here rather
/// than folded into that function. A caller who left the span unbounded
/// (`None`) has already opted out of a batch-size bound, so the charge falls
/// back to the decode footprint alone — the same answer a discovery worker's
/// call gets.
///
/// **And that second term is derived, not taken** ([`derived_source_span`];
/// `docs/design/decisions.md`, "D84"): the stated span is a ceiling, and a
/// budget that cannot seat `jobs` readers beside it narrows the span before it
/// cuts the count, stopping at `chunk_size` — the length this caller's replay
/// will announce to the source, and so the unit a batch actually pins. The
/// third return value is the span charged — the stated one where none was,
/// every block's advice retaining by partition — which the caller writes back onto
/// the sub-streams' own `QueryOptions` so the batches are the size the plan
/// was solved for.
///
/// **The span is charged per source, not universally**, because the second
/// term is honest for one source shape and double-counts for the other: a
/// source retaining by the read chunk pins bytes `partition_bytes` never
/// charged for, and one retaining by the partition pins bytes it did
/// (`pgdt_io::RetainedUnit`). So the term is added only where the advice
/// says [`pgdt_io::RetainedUnit::ReadChunk`], and where it is not, the note
/// below reads back `None` rather than a number that was never charged. An
/// **empty** `matches` has no advice to read, so it keeps the charge: nothing
/// runs either way, and the arm that says nothing is the one that charges
/// more.
///
/// **The second return value is the plan's own notes: at most one naming why
/// `workers` came up short of `parallelism.jobs()`, at most one naming a
/// compressed source that declined the block-decode path under this budget
/// ([`compressed_block_path_declined`]), at most one naming a budget that
/// affords less than a single reader ([`PlanNoteKind::AllocationBelowFloor`](crate::stream::note::PlanNoteKind::AllocationBelowFloor)),
/// and at most one naming a batch span narrowed below what the caller stated
/// ([`PlanNoteKind::BatchSpanNarrowed`](crate::stream::note::PlanNoteKind::BatchSpanNarrowed)).**
/// Empty on every path that limits nothing, so a caller need not special-case
/// "nothing to say" — though an empty `matches`, still charged its span, can
/// name a count the budget cut. The fourth is each block's advice, which a cut
/// made again at a dynamic filter's first poll cuts by ([`cut_blocks`]).
///
/// **What these sub-streams buy is the caller's to collect**: they run
/// concurrently only where every one is polled, as a DataFusion scan polls its
/// partitions, and `pgdt query`'s in-order merge reads one at a time past its
/// first round (`KD57`).
///
/// Deficiency register: `deficiency: KD23` — the cut here is over a whole
/// `CopyBlock` rather than through the leader's window, so a piece spans as
/// many units as the region holds over the worker count and a held batch's
/// `max_source_span` can pin several decoded blocks where
/// [`pgdt_io::RetainedUnit::Partition`] bills one;
/// `BOUNDARIED_PARTITION_UNITS` does not bound this path. **(c) unowned**;
/// promoted by a phase taking up query-path memory, the repair reversing a
/// recorded decision either way.
///
/// Deficiency register: `deficiency: KD41` — `chunk_size` is never announced
/// here, so a plain source's advice is cut from the chunk the last read on it
/// announced, or `POOL_MAX_BYTES` where none has: a `pgdt query` over a
/// complete cache prices its unit at the default chunk whatever
/// `--chunk-size` states, and a provider's first scan after `SET
/// pgdump.chunk_size` at the chunk before it. Only `max_source_span` follows
/// the stated chunk, so the smaller chunk
/// [`PlanNoteKind::ParallelismBudgetLimited`](crate::stream::note::PlanNoteKind::ParallelismBudgetLimited) advises does not seat more on
/// the plan it advises. **(c) unowned**; promoted by a stated chunk seen not
/// to seat what the note promised, the fix being `hint_read_size(chunk_size)`
/// beside `hint_parallelism` — a plan announcing a read size to a source
/// other scans share.
pub(super) fn plan_partitions(
    source: &dyn ByteRangeSource,
    matches: &[CopyBlock],
    kept: &BTreeMap<u64, Vec<Range<u64>>>,
    parallelism: Parallelism,
    max_source_span: Option<usize>,
    chunk_size: usize,
) -> (Vec<Vec<Segment>>, Vec<PlanNote>, Option<usize>, Vec<Partitioning>) {
    // **Announced before the advice is asked for**, since a compressed source
    // decides from the stated budget whether it can decode a whole block at
    // all (`ByteRangeSource::partitions`): asking under the mapping pass's
    // budget would plan against a read path the replay will not take. Each
    // sub-stream re-announces the same value, idempotently.
    source.hint_parallelism(parallelism);
    // A block's own advice, and its footprint, are read once per block; the
    // footprint that decides the sub-stream count is the largest of them.
    let advice: Vec<Partitioning> =
        matches.iter().map(|b| source.partitions(b.data_offset..b.end_offset)).collect();
    let widest = advice.iter().max_by_key(|advice| advice.partition_bytes());
    let footprint = widest.map_or(0, Partitioning::partition_bytes);
    // What the notes below offer, read off the source's cost and the budget
    // it was carved by: both callers carve with this recommendation
    // (`pgdt_io::Arrangement::resolve`).
    let chunk_sized = widest.is_some_and(Partitioning::sized_by_read_chunk);
    let recommended = source.default_worker_memory();
    let raises = |budget| Parallelism::allowance_raises_budget(recommended, budget);
    // The span is a cost of a chunk-shaped source only. `all` over an empty
    // advice would be vacuously true, so the emptiness is tested.
    let retains_partitions =
        !advice.is_empty() && advice.iter().all(|a| a.retained_unit() == RetainedUnit::Partition);
    let stated_span = if retains_partitions { None } else { max_source_span };
    // The charge the count is solved against: the largest per-worker footprint
    // above, carrying that source's shared pool term, plus this caller's span.
    let charge = advice
        .iter()
        .map(Partitioning::worker_memory)
        .max_by_key(|memory| memory.bytes_per_worker())
        .unwrap_or_default();
    // The span is derived from what the budget leaves once the readers asked
    // for are paid for, so `jobs` readers cost batch size rather than being
    // declined (`docs/design/decisions.md`, "D84").
    let charged_span =
        stated_span.map(|span| derived_source_span(charge, parallelism, span, chunk_size));
    // A span is on the chunk floor where it equals the chunk: the derived span
    // is `min(stated, max(room, chunk))`, which is the chunk only where the
    // room is at most the chunk and the stated span at least it, so a smaller
    // chunk lowers it wherever the count note below fires, the room being
    // below the chunk there. A span stated below the chunk is charged as
    // stated, and a smaller chunk moves nothing of it.
    let span_on_chunk = charged_span == Some(chunk_size);
    let charge = match charged_span {
        Some(span) => charge.plus_per_worker(span as u64),
        None => charge,
    };
    let workers = worker_count(parallelism, charge).max(1);
    let requested = parallelism.jobs();
    // The decline comes first because it is the wider fact: how every read of
    // this file is served, where the count below is how many readers there are.
    let mut notes: Vec<PlanNote> =
        compressed_block_path_declined(source, parallelism).into_iter().collect();
    // **Below one reader's worth the floors decide, and they are silent**, the
    // count note below firing only where `requested` exceeds what was planned.
    // Charged against the footprint alone rather than the whole charge: the
    // span term puts an ordinary plain `query` at the default budget over the
    // line (`docs/design/decisions.md`, "D3").
    if let Some(memory_bytes) = parallelism.memory_bytes()
        && footprint > 0
        && memory_bytes < footprint
    {
        notes.push(PlanNote::allocation_below_floor(
            footprint,
            memory_bytes,
            raises(memory_bytes),
            chunk_sized,
        ));
    }
    // **Said whenever the span moved, not only where the count fell short**:
    // the span is a number the caller stated on its own `QueryOptions`, so a
    // plan that charged a smaller one has not delivered what was asked for.
    if let (Some(memory_bytes), Some(stated), Some(planned)) =
        (parallelism.memory_bytes(), stated_span, charged_span)
        && planned < stated
    {
        notes.push(PlanNote::batch_span_narrowed(
            stated as u64,
            planned as u64,
            workers,
            memory_bytes,
            raises(memory_bytes),
            chunk_sized,
        ));
    }
    if let Some(memory_bytes) = parallelism.memory_bytes()
        && workers < requested
    {
        notes.push(PlanNote::parallelism_budget_limited(
            requested,
            workers,
            footprint,
            charged_span.map(|span| span as u64),
            memory_bytes,
            raises(memory_bytes),
            // The chunk moves the charge through either term: the reader's
            // cost where the source sizes it from the chunk, and the span
            // where it sits on the chunk floor. A span stated below the chunk
            // is charged as stated and moves with nothing here.
            chunk_sized || span_on_chunk,
        ));
    }

    let groups = cut_blocks(matches, kept, &advice, workers);
    (groups, notes, charged_span.or(max_source_span), advice)
}

/// Cut `matches` — each whole, or the runs of row groups `kept` holds of it —
/// into the pieces each block's `advice` permits, and group them into at most
/// `workers` sub-streams ([`distribute`]), byte-balanced over what they read.
pub(in crate::stream) fn cut_blocks(
    matches: &[CopyBlock],
    kept: &BTreeMap<u64, Vec<Range<u64>>>,
    advice: &[Partitioning],
    workers: usize,
) -> Vec<Vec<Segment>> {
    let mut segments = Vec::new();
    for (block, advice) in matches.iter().zip(advice) {
        let want = match advice.max_partitions() {
            Some(max) => workers.min(max),
            None => workers,
        };
        // A pruned block's runs are cut instead of its data, each into a
        // share of `want` proportional to its bytes, so the readers are
        // balanced over what remains to be read.
        let Some(runs) = kept.get(&block.header_offset) else {
            // The **data** range is what is cut, not `[header_offset, …)`: a
            // cut inside the header line would give the first piece no rows.
            // The first piece is then extended back over the header, its
            // schema's source.
            for (i, piece) in
                cut(block.data_offset..block.end_offset, advice, want).into_iter().enumerate()
            {
                let (start, entry) = if i == 0 {
                    (block.header_offset, SegmentEntry::Header)
                } else {
                    (piece.start, SegmentEntry::Interior)
                };
                segments.push(Segment::new(block, start, piece.end, entry));
            }
            continue;
        };
        let remaining: u64 = runs.iter().map(|run| run.end - run.start).sum();
        for run in runs {
            let share = (want as u64 * (run.end - run.start)).div_ceil(remaining.max(1));
            let share = usize::try_from(share).unwrap_or(want).clamp(1, want);
            // A run starting on the header's LF is cut from the first data
            // byte, for the reason the whole block's data range is.
            let from = run.start.max(block.data_offset);
            for piece in cut(from..run.end, advice, share) {
                let start = if piece.start == from { run.start } else { piece.start };
                segments.push(Segment::over(block, start..piece.end));
            }
        }
    }
    distribute(segments, workers)
}

/// The span a sub-stream's held batch is charged, given what one reader of
/// this source costs and what the caller stated
/// (`crate::options::QueryOptions::max_source_span`, which is a ceiling).
///
/// **It moves down or not at all** (`docs/design/decisions.md`, "D84"). Where
/// the stated span already affords the count asked for, it is returned
/// unchanged — so a serial replay, whose count is one whatever the charge, is
/// never narrowed. Where it does not, the span becomes what the budget has
/// left once `jobs` readers of the source are paid for:
/// `charge.plus_per_worker(s).at(jobs)` is `charge.at(jobs) + s × jobs`, so
/// the largest `s` that fits is `(budget − charge.at(jobs)) / jobs` — an
/// inversion of [`pgdt_io::WorkerMemory::at`] rather than a second copy of
/// its arithmetic, which keeps the shared pool term out of this function's
/// hands.
///
/// **The floor is `chunk_size`**, the length the replay loop announces to the
/// source (`pgdt_dump::ReadOptions::chunk_size_bytes`,
/// [`pgdt_io::ByteRangeSource::hint_read_size`]), so a budget that affords
/// the count at no span at all narrows to one read chunk and leaves the
/// shortfall to [`worker_count`]; below that the span would cost rows per
/// batch while bounding nothing the retained unit does not already bound.
/// **It is the announced chunk and not the shipped default**: the pin is the
/// span rounded out to what this source actually retains, which is what this
/// caller asked it to read in.
///
/// A caller stating no budget (`Parallelism::default`) derives nothing: there
/// is no number to solve against.
// Deficiency register: `deficiency: KD32` — the budget divided here is the
// read-buffer budget, which a plain source holds at `DEFAULT_MEMORY_BUDGET`
// whatever allowance was stated, so a plain `query`'s span and its sub-stream
// count are both fixed at a number picked to answer a different question.
// **(c) unowned**; closing it means a plain source recommending a per-reader
// cost, which is `KD25`'s reading.
fn derived_source_span(
    charge: WorkerMemory,
    parallelism: Parallelism,
    stated: usize,
    chunk_size: usize,
) -> usize {
    let Some(budget) = parallelism.memory_bytes() else { return stated };
    let jobs = parallelism.jobs();
    if charge.plus_per_worker(stated as u64).affords(budget, jobs) >= jobs {
        return stated;
    }
    let room = budget.saturating_sub(charge.at(jobs)) / jobs.max(1) as u64;
    stated.min(usize::try_from(room.max(chunk_size as u64)).unwrap_or(usize::MAX))
}

/// Group `segments` into at most `streams` contiguous, byte-balanced runs,
/// dropping the empty ones. **Contiguous rather than round-robin**, so
/// concatenating the sub-streams in order equals the serial replay
/// (`docs/design/decisions.md`, "D51"). The group is chosen from a segment's
/// **midpoint** in the running total, so one huge piece beside many small ones
/// does not push everything after it into the last group.
fn distribute(segments: Vec<Segment>, streams: usize) -> Vec<Vec<Segment>> {
    let streams = streams.max(1).min(segments.len().max(1));
    if streams == 1 {
        return vec![segments];
    }
    let total: u64 = segments.iter().map(Segment::weight).sum();
    let mut groups: Vec<Vec<Segment>> = vec![Vec::new(); streams];
    let mut walked = 0u64;
    for segment in segments {
        let weight = segment.weight();
        // Zero total is every piece empty, which is one group's worth of work
        // however many pieces there are.
        let group = ((walked + weight / 2) * streams as u64)
            .checked_div(total)
            .and_then(|g| usize::try_from(g).ok())
            .unwrap_or(0);
        groups[group.min(streams - 1)].push(segment);
        walked += weight;
    }
    groups.retain(|group| !group.is_empty());
    if groups.is_empty() {
        groups.push(Vec::new());
    }
    groups
}

#[cfg(test)]
mod tests;
