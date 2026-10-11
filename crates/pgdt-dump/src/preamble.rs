//! The preamble — the region before the first data — answered before a scan
//! runs: the bounded prepass reading it ([`scan_preamble`]), and the
//! metadata it declares from a cache or the file ([`preamble_only`]). The map
//! itself is [`pgdt_model::DumpIndex`].

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use crate::cache::{CacheLoad, CacheMode, SourceWatch};
use crate::ddl::dump_metadata_from_spans;
use crate::map::{Builder, attach_text, non_seekable_compression_diagnostic};
use crate::scan::{Event, ScanOptions, scan};
use pgdt_core::Result;
use pgdt_io::ByteRangeSource;
use pgdt_model::{Diagnostic, DumpIndex, DumpMetadata, Span, SpanBody};

/// Scan only far enough to recover the first database's preamble — up to (not
/// including) the first `COPY` block, its TOC comment included where one
/// precedes the header, or to EOF if none exists. Per I1 nothing `crate::ddl`
/// cares about can follow that point for the database open when it is reached,
/// and no earlier database in a multi-`\connect` dump has a `COPY` block before
/// it, so this one offset closes out the *first* database's preamble. The
/// bounded prepass is what lets a scan that may stop anywhere still state this
/// metadata (`docs/design/decisions.md`, "D30", "D36").
///
/// Returns the recovered metadata, the spans tiling `[0, preamble_end)` (no
/// `COPY` block among them, the scan stopping before the first `COPY` block
/// rather than walking into it — though a file with large objects and
/// no `COPY` block maps that region as a `Data` span), that offset itself — a safe watermark
/// for a later scan to continue from — and whatever roles/tablespaces the
/// preamble region referenced (`DumpIndex::roles`/`tablespaces`'s own
/// partial-scan caveat applies).
pub async fn scan_preamble(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
) -> Result<(DumpMetadata, Vec<Span>, u64, BTreeSet<String>, BTreeSet<String>)> {
    let mut spans = Builder::new();
    let mut end = source.size().await?;
    scan(source, options, |event| match event {
        Event::CopyStart(start) => {
            // Not fed to the map builder: it would open a `Data` span this
            // scan never closes. The stop retreats to a pending TOC
            // comment's own start rather than guessing the comment's kind,
            // leaving it for the later scan that absorbs it into the `Data`
            // span (`docs/design/decisions.md`, "D31", "D33").
            end = spans.pending_comment_start().unwrap_or(start.header_offset);
            ControlFlow::Break(())
        }
        Event::Line(line) => {
            spans.feed_line(line.offset, line.raw);
            ControlFlow::Continue(())
        }
        Event::DollarQuoteEnd(end) => {
            spans.on_dollar_quote_end(end.offset);
            ControlFlow::Continue(())
        }
        // I1/I12: the first `COPY` header this scan stops for cannot follow
        // a large-object region, so this scan always breaks first in practice.
        // Fed through anyway, so a file with large objects but no `COPY`
        // blocks still maps that region as its own `Data` span.
        Event::LargeObjectStart(start) => {
            spans.on_large_object_start(start.start_offset);
            ControlFlow::Continue(())
        }
        Event::LargeObjectEnd(end) => {
            spans.on_large_object_end(end.end_offset);
            ControlFlow::Continue(())
        }
        // Never actually reached: `CopyStart` above always breaks first. Kept
        // explicit rather than a bare `_` so a new `Event` variant fails to
        // compile here instead of silently falling through.
        Event::Row(_) | Event::CopyEnd(_) => ControlFlow::Continue(()),
    })
    .await?;
    let roles = spans.roles().clone();
    let tablespaces = spans.tablespaces().clone();
    let spans = spans.finish(end);
    let metadata = dump_metadata_from_spans(&spans);
    Ok((metadata, spans, end, roles, tablespaces))
}

/// Answer from the preamble alone (`docs/design/decisions.md`,
/// "D61", `--preamble-only`): reuse a cache's already-known metadata when
/// present, falling back to a fresh [`scan_preamble`] otherwise and
/// persisting the result when the cache is enabled (a no-op when it isn't —
/// see [`CacheMode::save`]). Bounded to the file's first `COPY` block
/// regardless of dump size (I1), independent of `build_index`'s full
/// structural scan.
///
/// A fresh scan's spans are persisted alongside the metadata, with a
/// trailing [`SpanBody::Unscanned`] span covering the rest of the file — a
/// genuinely partial scan, unlike `build_index`, which always reaches EOF
/// (`docs/design/decisions.md`, "D30").
///
/// Also returns whatever [`CacheMode::load`] reported on the loaded index
/// (e.g. a `CacheMtimeChanged` warning): answering with `DumpMetadata` alone
/// rather than a whole `DumpIndex`, its diagnostics have nowhere else to go.
pub async fn preamble_only(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    cache: &CacheMode,
) -> Result<(DumpMetadata, Vec<Diagnostic>)> {
    let watch = SourceWatch::open(source, cache.strict_identity()).await?;
    let answer = preamble_only_watched(source, options, cache, &watch).await;
    watch.attribute(source, answer).await
}

/// [`preamble_only`] once its watch is open, which then attributes any
/// failure.
async fn preamble_only_watched(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    cache: &CacheMode,
    watch: &SourceWatch,
) -> Result<(DumpMetadata, Vec<Diagnostic>)> {
    let mut base_index = match cache.load(source).await? {
        CacheLoad::Index(index) => index,
        // Nothing to build forward from, and nothing at the path to keep.
        CacheLoad::Disabled | CacheLoad::Missing => DumpIndex::default(),
        // Refused before the dump is read past its magic, unless the caller said
        // this cache may be replaced (`docs/design/decisions.md`, "D20").
        CacheLoad::Unusable(unusable) => match cache.refusal(&unusable) {
            Some(refusal) => return Err(refusal),
            None => DumpIndex::default(),
        },
    };
    let known = base_index
        .metadata
        .as_ref()
        .and_then(|m| m.databases.first())
        .is_some_and(|db| db.preamble_complete);
    if !known {
        let (metadata, mut spans, preamble_end, roles, tablespaces) =
            scan_preamble(source, options).await?;
        base_index.metadata = Some(metadata);
        base_index.roles.extend(roles);
        base_index.tablespaces.extend(tablespaces);
        base_index.scanned_through = base_index.scanned_through.max(preamble_end);
        let file_size = source.size().await?;
        if preamble_end < file_size {
            spans.push(Span {
                start: preamble_end,
                end: file_size,
                database: None,
                text: None,
                toc: None,
                toc_owned: false,
                body: SpanBody::Unscanned,
            });
        }
        base_index.spans.extend(spans);
        attach_text(source, &mut base_index.spans).await?;
        // Before the save, which records the preamble as complete, so a later
        // run never re-reads bytes that failed their own check
        // (`crate::cache::SourceWatch::finish`).
        watch.finish(source).await?;
        cache.save(watch, source, &base_index).await?;
    }
    // A preamble-only run over a complete cache saves nothing, so this is
    // where it says the file did not move underneath it
    // (`crate::cache::SourceWatch`).
    watch.check(source).await?;
    // A loaded cache already carries this diagnostic, computed by
    // `cache::store::status_from_file` off the persisted table — so this is
    // idempotent rather than gated on `!known`, which would add it twice to
    // a cache that exists but whose preamble was not yet complete.
    if let Some(d) = non_seekable_compression_diagnostic(source.seek_table().as_ref())
        && !base_index.diagnostics.contains(&d)
    {
        base_index.diagnostics.push(d);
    }
    Ok((base_index.metadata.unwrap_or_default(), base_index.diagnostics))
}
