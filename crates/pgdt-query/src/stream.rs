//! Pull-mode streaming API (`docs/design/decisions.md`, "Batches, streams and
//! the leader").
//!
//! [`table_stream`] is the primitive: an async `Stream<Item =
//! Result<RecordBatch>>` built directly on [`CopyScanner`](pgdt_dump::CopyScanner)/[`RowBatcher`](crate::batch::RowBatcher), the
//! same machinery [`read_table`] (push mode) drives internally.
//! [`ResumeToken`] resumes a consumption within the same process; it holds no
//! public fields (`docs/design/decisions.md`, "D49").
//!
//! **Mapping and streaming are separate passes**
//! (`docs/design/decisions.md`, "D48"). A query runs in two phases, never
//! interleaved:
//!
//! 1. `pgdt_mapping`'s `map_forward` extends the [`DumpIndex`](pgdt_model::DumpIndex)'s map from its own
//!    `scanned_through`, recording every `COPY` block it passes and
//!    classifying the DDL between them through a [`pgdt_dump::internal::Builder`], and
//!    **yields nothing**. It stops as soon as the queried table is settled
//!    ([`ScanExtent::UntilTargetSettled`](crate::options::ScanExtent::UntilTargetSettled)).
//! 2. Every block the map holds for that table is then replayed for its rows,
//!    in file order.
//!
//! So the queried block's bytes are read twice, and a [`ResumeToken`] can only
//! ever point inside already-mapped territory. A [`CacheMode::Enabled`](pgdt_dump::cache::CacheMode::Enabled) cache
//! is persisted where the map advances: completed blocks whose save has earned
//! its cost (`pgdt_mapping`'s save throttle), the block that settles the query, and every
//! exit past the first read but an error. [`CacheMode::Disabled`](pgdt_dump::cache::CacheMode::Disabled) runs the same way with `save` a no-op.
//!
//! **Preamble capture** (`docs/design/decisions.md`, "D30"): before any of
//! that, [`table_stream`] runs [`pgdt_dump::internal::scan_preamble`] once (skipped
//! once a cache already has it), whatever table was queried and under
//! [`CacheMode::Disabled`](pgdt_dump::cache::CacheMode::Disabled) too. It covers the *first* database; every later
//! `\connect`ed one is stated by `map_forward` at that database's first
//! `COPY` block (I1).
//!
//! **Type resolution**: a matching `COPY` block's column list is resolved
//! against that captured metadata into a [`pgdt_codec::ResolvedSchema`]
//! ([`TableStream::resolved_schema`]), and the batches this stream yields are
//! built to it: typed under `SchemaMode::Typed`, all-`Utf8View` under
//! `SchemaMode::Strings`. A table's blocks may list its columns in different
//! orders, and every batch comes out in the table's one order
//! ([`TableColumns`](self::replay::plan::TableColumns)).

mod mapping;
mod note;
mod partitions;
mod replay;
mod resume;
mod table;

pub use self::note::{EarlyStop, PlanLever, PlanNote, PlanNoteKind};
pub use self::partitions::{DynamicPartitions, TablePartitions, table_stream_partitions};
pub use self::replay::dynamic::{DynamicFilter, RowEvaluation};
pub use self::resume::ResumeToken;
pub use self::table::{BlockingTableIter, TableStream, table_schema};

use std::ops::ControlFlow;
use std::sync::Arc;

use arrow::array::RecordBatch;
use async_stream::try_stream;
use futures::StreamExt;

use pgdt_codec::ResolvedSchema;
use pgdt_core::{Error, Result};
use pgdt_dump::ScanOptions;
use pgdt_dump::cache::{CacheMode, SourceWatch};
use pgdt_io::ByteRangeSource;

use self::mapping::{MappedTable, map_for_query, validate_request};
use self::replay::plan::ReplayPlan;
use self::replay::replay;
use self::replay::segment::Segment;
use self::resume::query_fingerprint;
use self::table::StreamShared;
use crate::options::QueryOptions;

/// Pull-mode entry point: stream `RecordBatch`es for every row of
/// every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`](pgdt_core::CopyHeader::matches)). A table with zero rows yields no batches.
///
/// `resume` continues a previous consumption from a [`ResumeToken`] it
/// produced; `None` starts from the beginning of `source`. A token whose query
/// fingerprint disagrees with `query_options` is `Error::ResumeQueryMismatch`.
///
/// `query_options.filter` is the post-parse row filter, one `Expr` tree
/// (`docs/design/decisions.md`, "D54"): the empty conjunction yields every row,
/// and otherwise a row is kept only where the tree is `True`, tested on the raw
/// row, each term unescaping only the field it reads (D28). A term referencing a column absent from a
/// matching block's own schema is `Error::UnknownPredicateColumn`. Every
/// refusal resolving a block raises — schema, filter or projection — is
/// yielded before any row of the table, for the first refusing block in file
/// order (`docs/design/decisions.md`, "D54"), and so is
/// `Error::TableColumnsDisagree`, for a table whose blocks name different
/// columns ([`TableColumns`](self::replay::plan::TableColumns)).
///
/// `query_options.projection` decides which columns are materialized
/// (`docs/design/decisions.md`, "D28"). It cuts the schema
/// [`TableStream::resolved_schema`] reports as well as the batches, may
/// reorder, and may be empty — a zero-column projection yields batches
/// carrying a row count and nothing else. A filter term may name a column the
/// projection does not.
///
/// `cache` controls structure-cache consulting
/// (`docs/design/decisions.md`, "D22"): `CacheMode::Enabled` persists the map
/// as the mapping pass advances, `CacheMode::Disabled` runs identically but
/// writes nothing. Either way rows come from replaying mapped blocks, never
/// from the mapping pass itself. `query_options.scan_extent` decides how much
/// of the file that pass walks before any row comes back; see [`ScanExtent`](crate::options::ScanExtent).
pub fn table_stream<'a>(
    source: &'a dyn ByteRangeSource,
    table: &str,
    scan_options: ScanOptions,
    query_options: QueryOptions,
    resume: Option<ResumeToken>,
    cache: CacheMode,
) -> TableStream<'a> {
    let table = table.to_string();
    let fingerprint = query_fingerprint(&table, &query_options, None);
    let start_token = resume.clone().unwrap_or_else(|| ResumeToken::start(fingerprint));
    let shared = StreamShared::new(start_token);
    let shared_for_stream = shared.clone();

    let inner = try_stream! {
        validate_request(&query_options, resume.as_ref(), fingerprint)?;
        // Taken before the map is read, so a warm query — which reaches no
        // save at all — still has something to compare against when it
        // finishes (`pgdt_dump::cache::SourceWatch`).
        let watch = SourceWatch::open(source, cache.strict_identity()).await?;
        let mapped =
            map_for_query(source, &table, &scan_options, &query_options, &cache, &watch).await;
        let mapped = watch.attribute(source, mapped).await?;

        // Pass 2: replay each matching block for its rows, as one segment
        // per kept run. A resumed stream picks up inside this same list, every
        // resume point being inside a mapped block by construction.
        let MappedTable { matches, metadata } = mapped;
        let plan = Arc::new(ReplayPlan::new(scan_options, query_options, &matches, metadata)?);
        *shared_for_stream.plan_notes.lock().unwrap() = plan.notes().collect();
        let resume_offset = resume.as_ref().map_or(0, |t| t.offset);
        let segments: Vec<Segment> = matches
            .iter()
            .filter(|b| b.end_offset > resume_offset)
            .flat_map(|b| plan.segments(b))
            .filter_map(|segment| segment.resumed_at(resume_offset))
            .collect();
        // Dropped before the replay rather than at the stream's end: the
        // matches hold the blocks' statistics, which the segments do not
        // ([`Segment::new`]).
        drop(matches);
        let mut rows =
            Box::pin(replay(source, plan, segments, shared_for_stream, resume, fingerprint, None));
        while let Some(batch) = rows.next().await {
            yield watch.attribute(source, batch).await?;
        }
        // The run's last word: the rows are out, so what this recovers is a
        // failure naming the cause in place of a silent wrong answer.
        watch.finish(source).await?;
    };

    shared.into_stream(Box::pin(inner))
}

/// Scan `source` as far as `query_options.scan_extent` says, assembling typed
/// `RecordBatch`es for every row
/// of every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`](pgdt_core::CopyHeader::matches)). A table with zero rows produces no batches.
///
/// Push-mode entry point (`docs/design/decisions.md`, "D68"): internally
/// drains the pull-mode [`crate::stream::table_stream`], so the two share one
/// scan loop. The callback may return [`ControlFlow::Break`] to stop early,
/// in which case the returned token resumes from just past the last batch
/// delivered to it — see [`crate::stream::TableStream::resume_token`].
/// `query_options` and `cache` mean what they do on `table_stream`. Rejects
/// `CacheMode::Offline` up front: `source` is mandatory here, and `Span::text`
/// is `None` for every `Data` span regardless, so `query` could never answer
/// from a cache alone (`docs/design/decisions.md`, "D30").
pub async fn read_table<F>(
    source: &dyn ByteRangeSource,
    table: &str,
    scan_options: &ScanOptions,
    query_options: &QueryOptions,
    cache: CacheMode,
    mut on_batch: F,
) -> Result<(ResolvedSchema, Option<ResumeToken>)>
where
    F: FnMut(RecordBatch) -> ControlFlow<()>,
{
    use futures::StreamExt;

    if matches!(cache, CacheMode::Offline(_)) {
        return Err(Error::CacheModeMismatch(
            "query requires a live dump source; CacheMode::Offline is cache-only",
        ));
    }

    let mut stream =
        table_stream(source, table, scan_options.clone(), query_options.clone(), None, cache);
    while let Some(batch) = stream.next().await.transpose()? {
        if on_batch(batch).is_break() {
            return Ok((stream.resolved_schema(), Some(stream.resume_token())));
        }
    }
    Ok((stream.resolved_schema(), None))
}

#[cfg(test)]
mod tests;
