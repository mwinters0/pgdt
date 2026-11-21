//! The eager pass `pgdt_mapping::map_file` is held to, behind `mapping`: a
//! whole file mapped serially in one scan ([`build_index`], [`build_map`]),
//! and one block the map holds back-filled outside any pass
//! ([`backfill_block`]). Tests alone call them, so they are no part of the
//! mapping pass's designed surface (`docs/design/decisions.md`, "D106"); each
//! stays an independent oracle — serial, whole-file, no interior, no resume,
//! no saves — sharing with the passes it checks only the block's open and
//! its recorder, the mechanism under test (`docs/design/decisions.md`,
//! "D170").

use std::ops::ControlFlow;
use std::sync::Arc;

use pgdt_core::{Error, Result};
use pgdt_dump::internal::{Builder, derived_diagnostics, finish_block};
use pgdt_dump::{Event, ReadOptions, ScanOptions, attach_text, dump_metadata_from_spans, scan};
use pgdt_io::{Arrangement, ByteRangeSource};
use pgdt_mapping::internal::{OpenBlock, Policy, ReadContext, backfill_one, open};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{BlockRecord, CopyBlock, DumpIndex, DumpMetadata, PostgresInvalidValues, Span};
use tokio_util::sync::CancellationToken;

/// Scan `source` end to end and build its full file map, every block
/// censused and counted and none gathering statistics — the eager, serial
/// pass the incremental one (`pgdt_mapping::map_file`) is held to. One pass:
/// [`Builder`] is fed the whole [`Event`] stream, and `DumpIndex::metadata`
/// is [`dump_metadata_from_spans`] over the result and [`DumpIndex::blocks`]
/// a filter over it (`docs/design/decisions.md`, "D34"). It reads with
/// `options`, and nothing stops it from outside ([`scan`]).
///
/// Counting a block's values resolves its columns' declared types, so the
/// pass states each database's DDL at its first `COPY` block, as
/// `pgdt_mapping`'s forward pass does (`docs/design/decisions.md`, "D96").
pub async fn build_index(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<DumpIndex> {
    let (builder, size) = eager_pass(source, options).await?;
    let roles = builder.roles().clone();
    let tablespaces = builder.tablespaces().clone();
    let mut spans = builder.finish(size);
    let metadata = Some(Arc::new(dump_metadata_from_spans(&spans)));
    attach_text(source, &mut spans).await?;
    let mut index = DumpIndex {
        spans,
        scanned_through: size,
        metadata,
        roles,
        tablespaces,
        diagnostics: Vec::new(),
    };
    index.diagnostics = derived_diagnostics(&index, size, source.compression_index().as_ref());
    Ok(index)
}

/// Scan `source` end to end and build its full file map as a span list —
/// [`build_index`]'s spans, alone. Always scans to EOF, so
/// [`pgdt_model::SpanBody::Unscanned`] never appears in the result.
pub async fn build_map(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<Vec<Span>> {
    let (builder, end) = eager_pass(source, options).await?;
    let mut spans = builder.finish(end);
    attach_text(source, &mut spans).await?;
    Ok(spans)
}

/// [`build_index`] and [`build_map`]'s one scan: every event to one builder,
/// each block opened under the eager policy ([`Policy::Eager`]) against the
/// DDL above it, restated once per database, and the end of the file it
/// reached.
async fn eager_pass(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<(Builder, u64)> {
    let mut builder = Builder::new();
    // What stopped the scan, a block's close failing it. None can: this pass
    // gathers no statistics, so no block meets a refused field.
    let mut failed: Option<Error> = None;
    // The database the metadata in hand was stated for, and that metadata.
    let mut stated: Option<(Option<String>, DumpMetadata)> = None;
    // The open `COPY` block's header and what records its rows.
    let mut opened = None;
    let reached = scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => {
                let boundary = builder.pending_comment_start().unwrap_or(start.header_offset);
                let header = start.header.clone();
                let data_offset = start.data_offset;
                builder.on_copy_start(*start);
                let db = builder.database().map(str::to_owned);
                if stated.as_ref().is_none_or(|(covers, _)| *covers != db) {
                    let metadata = dump_metadata_from_spans(&builder.snapshot(boundary));
                    stated = Some((db.clone(), metadata));
                }
                let metadata = stated.as_ref().map(|(_, metadata)| metadata);
                let OpenBlock { recorder, .. } =
                    open(&header, data_offset, metadata, db.as_deref(), Policy::Eager);
                opened = Some((header, recorder));
            }
            Event::Row(row) => {
                if let Some((_, recorder)) = &mut opened {
                    recorder.row(row.offset, row.raw);
                }
            }
            // `scan` streams no row.
            Event::StreamedRow(_) => unreachable!("an eager pass streams no row"),
            Event::CopyEnd(end) => {
                let record = match opened.take() {
                    Some((header, recorder)) => finish_block(recorder, &header, &end),
                    None => Ok(BlockRecord::default()),
                };
                match record {
                    Ok(record) => builder.on_copy_end(*end, record),
                    Err(e) => {
                        failed = Some(e);
                        return ControlFlow::Break(());
                    }
                }
            }
            Event::Line(line) => builder.feed_line(line.offset, line.raw),
            Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
            Event::LargeObjectStart(start) => builder.on_large_object_start(start.start_offset),
            Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
        }
        ControlFlow::Continue(())
    })
    .await?;
    match failed {
        Some(e) => Err(e),
        None => Ok((builder, reached.expect("only a failed block stops this scan short"))),
    }
}

/// **Back-fill one block the map already holds**, outside any pass: what
/// `pgdt_mapping::map_file`'s back-fill does to every block lacking what its
/// request asks, through the same per-block step
/// ([`backfill_one`]), read with `read` under `arrangement` as a pass's
/// readers read — its parallelism, its ledger and its statistics allowance —
/// and stopped by nothing. Answers whether it read the block, as that step
/// does, a refusal included; it is handed no cache, so persists nothing and
/// names none.
#[allow(clippy::too_many_arguments)]
pub async fn backfill_block(
    source: &dyn ByteRangeSource,
    read: ReadOptions,
    arrangement: &Arrangement,
    invalid: PostgresInvalidValues,
    request: &StatisticsRequest,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    block: &mut CopyBlock,
) -> Result<bool> {
    let scan = ScanOptions { read, parallelism: arrangement.parallelism() };
    let extent = source.extent().await?;
    let cancel = CancellationToken::new();
    let ledger = arrangement.ledger();
    let ctx = ReadContext { source, scan, ledger, cancel: &cancel, extent };
    let allowance = arrangement.statistics_allowance();
    backfill_one(ctx, allowance, invalid, request, metadata, database, block).await
}
