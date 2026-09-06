//! The structural summary a scan produces: where every COPY block lives.
//!
//! This is what the eager `pgdq parse` scan yields today and what the
//! structure cache will persist once it exists.

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::cache::CacheMode;
use crate::copy::CopyHeader;
use crate::diagnostic::Diagnostic;
use crate::io::ByteRangeSource;
use crate::map::{Builder, DataBlock, Span, SpanBody, attach_text, check_tiling};
use crate::preamble::dump_metadata_from_spans;
use crate::scan::{Event, ScanOptions, scan};

/// A block's sparse row index: the byte offset of every `interval`-th data
/// row, letting a later reader seek into the middle of a large block instead
/// of scanning from its start. Reserved in the cache format from the first
/// release and **not populated yet**. Its interval, serialization and
/// invalidation rules are defined by whichever of the parallel-scan or
/// row-group-statistics work runs first, because those are the first readers
/// of one and the second inherits the interval as a decision already made
/// (`docs/design/roadmap.md`). Reserving the slot is what keeps that additive
/// rather than a cache-format break.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SparseRowIndex {
    /// Rows between checkpoints (matches the default batch size, 8192).
    pub interval: u64,
    /// `checkpoints[i]` is the byte offset of data row `i * interval` within
    /// the block.
    pub checkpoints: Vec<u64>,
}

/// Per-row-group column statistics for one block, keyed to its
/// [`SparseRowIndex`] checkpoints. Reserved in the cache format from the
/// first release; not populated yet — `docs/design/roadmap.md`, "P10 —
/// Per-row-group column statistics", defines its real shape (null counts,
/// sortedness, min/max, the type each was computed as).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowGroupStats {}

/// Dump-level preamble: source server version, `pg_dump` version, extension
/// list, user-defined type definitions. Populated by [`build_index`] via
/// `crate::preamble` (`docs/design/architecture.md`, "The preamble grammar
/// and `DumpMetadata`").
pub use crate::preamble::DumpMetadata;

/// The most dimensions PostgreSQL can give an array — `MAXDIM`, 6 in every
/// supported version (`src/include/utils/array.h` on v14+, `src/include/c.h`
/// on v13; see I25). A literal whose
/// leading brace run is longer than this did not come out of `array_out`
/// (I25), so a consumer must treat it as unusable rather than as a depth.
pub const MAX_ARRAY_DIMS: u8 = 6;

/// What one column's array values look like within one `COPY` block — the
/// shape census (`docs/design/architecture.md`, "The array shape census").
///
/// Recorded per column because an array's dimensionality belongs to the
/// *value* (I21), so a column's Arrow type cannot be settled from the DDL
/// alone. Recorded per block because that is what the map already extends
/// incrementally; combining blocks is min-of-mins, max-of-maxes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrayShape {
    /// Fewest and most dimensions any value in this column carried, as
    /// `(min, max)`. `None` until a value contributes one — a SQL NULL never
    /// does, and neither does `{}`, which is what `array_out` writes for an
    /// empty array of *any* dimensionality (I25) and so fits every depth.
    ///
    /// **Both bounds, not just the maximum**, is the whole reason this is a
    /// pair. A column holding `{1,2}` and `{{1,2},{3,4}}` records `(1, 2)`
    /// where a column holding only 2-D values records `(2, 2)`; with a
    /// maximum alone the two are indistinguishable, and the mixed column
    /// would resolve to a confidently wrong `List<List<T>>` that then fails
    /// on every 1-D value in it.
    pub dims: Option<(u8, u8)>,
    /// Whether any value carried an explicit `[lb:ub]=` lower-bound prefix.
    /// Kept apart from `dims` because it disqualifies a column on its own:
    /// Arrow lists are 0-based and have nowhere to record an index origin,
    /// however uniform the dimensionality is.
    pub lower_bound_prefix: bool,
}

impl ArrayShape {
    /// Fold one still-COPY-escaped field into this column's census.
    ///
    /// **The field is not decoded first, and does not need to be.** Neither
    /// `{` nor anything the `[lb:ub]=` prefix is made of is in COPY TEXT's
    /// escape set (I15), so both the prefix and the leading brace run are
    /// visible in the raw bytes. A field that is not an array literal — a
    /// number, a `\N`, a composite's `(…)` — contributes nothing and costs
    /// one byte comparison.
    ///
    /// Dimensionality is the **leading brace run** (I25): `array_out` opens
    /// with exactly `ndim` braces and force-quotes any element containing a
    /// `{`, so no element can extend the run.
    pub(crate) fn observe(&mut self, field: &[u8]) {
        let mut rest = field;
        if rest.first() == Some(&b'[') {
            // `[lb:ub]…=` — the prefix runs to the `=` that introduces the
            // value proper. `array_out` returns a bare `{}` before it ever
            // formats dimensions, so a prefix always has a real array after
            // it; anything else here is not array output and is left alone.
            let Some(eq) = rest.iter().position(|&b| b == b'=') else { return };
            if rest.get(eq + 1) != Some(&b'{') {
                return;
            }
            self.lower_bound_prefix = true;
            rest = &rest[eq + 1..];
        }
        if rest.first() != Some(&b'{') {
            return;
        }
        // The empty array fits any depth, so it constrains neither bound.
        if rest == b"{}" {
            return;
        }
        let depth = rest.iter().take_while(|&&b| b == b'{').count().min(u8::MAX as usize) as u8;
        let (min, max) = self.dims.unwrap_or((depth, depth));
        self.dims = Some((min.min(depth), max.max(depth)));
    }

    /// Fold another block's census for the same column into this one —
    /// min-of-mins, max-of-maxes, and a lower-bound prefix anywhere counts
    /// everywhere. One table's data can occupy several blocks (I2), so a
    /// consumer that resolves a *table*'s type combines them this way.
    pub fn merge(&mut self, other: &ArrayShape) {
        self.dims = match (self.dims, other.dims) {
            (Some((a_min, a_max)), Some((b_min, b_max))) => {
                Some((a_min.min(b_min), a_max.max(b_max)))
            }
            (some, None) | (None, some) => some,
        };
        self.lower_bound_prefix |= other.lower_bound_prefix;
    }
}

/// The census of several blocks read as one — what a table spanning more than
/// one `COPY` block (I2) resolves against, and what a query's schema commits
/// to over exactly the blocks it will replay
/// (`docs/design/architecture.md`, "The array shape census").
///
/// The result is as long as the longest census in `blocks`; a column absent
/// from a shorter one contributes nothing, which is what a default
/// [`ArrayShape`] already means.
pub fn union_census<'a>(blocks: impl IntoIterator<Item = &'a CopyBlock>) -> Vec<ArrayShape> {
    let mut out: Vec<ArrayShape> = Vec::new();
    for block in blocks {
        if block.array_shapes.len() > out.len() {
            out.resize(block.array_shapes.len(), ArrayShape::default());
        }
        for (slot, shape) in out.iter_mut().zip(&block.array_shapes) {
            slot.merge(shape);
        }
    }
    out
}

/// One located COPY block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyBlock {
    pub header: CopyHeader,
    /// The database this block belongs to — the name from the `\connect`
    /// governing it, read straight off the line as either scan pass sees it.
    /// `None` means the file had no `\connect` at all (a plain dump), which
    /// falls back to the single unnamed [`DatabaseMetadata`]. Not an ordinal
    /// into `metadata.databases`: an incremental scan's metadata can hold
    /// just one entry no matter how many databases the file contains, so an
    /// index would be unresolvable in exactly the case this field exists for
    /// (`docs/design/architecture.md`, "One target per query").
    pub database: Option<String>,
    /// Absolute file offset of the `C` in `COPY`.
    pub header_offset: u64,
    /// Absolute file offset of the block's first data byte.
    pub data_offset: u64,
    /// Absolute file offset of the `\.` terminator line.
    pub terminator_offset: u64,
    /// Absolute file offset just past the terminator line.
    pub end_offset: u64,
    pub row_count: u64,
    /// The root table named by this block's `-- load via partition root
    /// <name>` marker, if it carried one (I2) — meaning `header` names that
    /// root rather than the partition whose rows follow, and **other blocks
    /// in this same dump carry the same header name**. Stored rather than
    /// concluded from: it is a line the dump wrote, which is what
    /// `layering.md` rule 5 asks L1 to keep.
    ///
    /// `crate::stream::table_stream` reads it to decide whether a cold query
    /// may stop once the queried table's block closes, or must run to EOF
    /// because more blocks can share the name — the blocks are *not*
    /// adjacent, so nothing cheaper than EOF enumerates them
    /// (`docs/design/architecture.md`, "Query: mapping and streaming are separate passes").
    pub partition_root: Option<String>,
    /// Reserved — see [`SparseRowIndex`]. Always `None`.
    pub sparse_index: Option<SparseRowIndex>,
    /// Reserved — see [`RowGroupStats`]. Always `None`.
    pub column_stats: Option<RowGroupStats>,
    /// This block's array-shape census, one [`ArrayShape`] per column in
    /// `header.columns` order.
    ///
    /// **Every mapping pass censuses**, under either
    /// `crate::batch::ScanExtent`, so a block in the map always carries one:
    /// `scanned_through` advances at a `CopyEnd` watermark or at EOF and
    /// nowhere else, which means a block that reached the map was walked end
    /// to end. An empty vector is therefore "censused, saw no array-shaped
    /// literal", the same answer as a vector of unconstrained
    /// [`ArrayShape`]s, and needs no separate representation.
    ///
    /// Shorter than `header.columns` never happens; *longer* only for a
    /// header-less block, whose field count comes from the rows themselves.
    pub array_shapes: Vec<ArrayShape>,
}

/// The full file map discovered in a dump, in file order
/// (`docs/design/architecture.md`, "`DumpIndex`: one owner per fact"). [`DumpIndex::blocks`] is a derived
/// filter over it, not a second stored structure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpIndex {
    pub spans: Vec<Span>,
    /// How much of the file the scan covered. Equal to the file size after an
    /// eager scan.
    pub scanned_through: u64,
    /// Dump-level preamble metadata — see [`DumpMetadata`]. Always `None`
    /// for a `DumpIndex` a caller built by hand rather than through
    /// [`build_index`], but every `build_index` scan
    /// populates it. A derived view over `spans`
    /// (`crate::preamble::dump_metadata_from_spans`), computed once when the
    /// scan that produced `spans` finishes rather than stored twice — see
    /// `docs/design/architecture.md`, "`DumpIndex`: one owner per fact".
    pub metadata: Option<DumpMetadata>,
    /// Roles referenced anywhere the scan has reached — the TOC `Owner:`
    /// field, `ALTER ... OWNER TO`, and `GRANT`/`REVOKE`/`ALTER DEFAULT
    /// PRIVILEGES FOR ROLE` (`docs/design/architecture.md`,
    /// "TOC enrichment"). `PUBLIC` is never included. Flat and per-file
    /// — a per-database view is a filter over `Span::database`, not a second
    /// stored structure. Persisted: unlike `metadata`/`diagnostics`, there's
    /// no cheaper way to answer "which roles does this dump need" than
    /// keeping what the scan already found, since re-deriving it means
    /// re-parsing every span's raw text. Complete only once `scanned_through`
    /// reaches the file's size — a query that stops at its target (`crate::stream`)
    /// never sees a reference past the stopping point, the same partiality
    /// `metadata`'s `preamble_complete` flags for its own data.
    pub roles: BTreeSet<String>,
    /// Tablespaces referenced anywhere the scan has reached — the TOC
    /// `Tablespace:` field and `SET default_tablespace = ...;`. `pg_default`
    /// is never included. Same flatness, persistence and partial-scan caveat
    /// as [`roles`](Self::roles).
    pub tablespaces: BTreeSet<String>,
    /// Things worth telling the caller that have no `Result` to travel in —
    /// a tiling failure, a cache mtime mismatch. **Not persisted**
    /// (`#[serde(skip)]`): a cached diagnostic would replay a warning about a
    /// check *this* run performed successfully. Recomputed wherever an index
    /// is produced or loaded; see [`crate::diagnostic`].
    #[serde(skip)]
    pub diagnostics: Vec<Diagnostic>,
}

impl DumpIndex {
    /// The `COPY` blocks among `spans`, in file order — a filtered view, not
    /// a stored field, so a block's byte offsets have exactly one owner
    /// (`docs/design/architecture.md`, "`DumpIndex`: one owner per fact").
    pub fn blocks(&self) -> impl Iterator<Item = &CopyBlock> {
        self.spans.iter().filter_map(|s| match &s.body {
            SpanBody::Data(DataBlock::Copy(block)) => Some(block),
            _ => None,
        })
    }

    /// Blocks whose table matches `name`, given qualified (`schema.table`) or
    /// bare (`table`, any schema).
    pub fn blocks_for(&self, name: &str) -> impl Iterator<Item = &CopyBlock> {
        self.blocks().filter(move |b| b.header.matches(name))
    }

    pub fn total_rows(&self) -> u64 {
        self.blocks().map(|b| b.row_count).sum()
    }

    /// Whether this scan reached the end of a `size`-byte file — the test
    /// that decides when a whole-file fact may be believed.
    ///
    /// [`roles`](Self::roles) and [`tablespaces`](Self::tablespaces) carry
    /// the same partiality and share this one rule: a query that stops at
    /// its target (`crate::batch::ScanExtent::UntilTargetSettled`, the
    /// default) never reaches what lies past the stopping point.
    ///
    /// It qualifies a **reported** table schema for the same reason — one
    /// table's data can occupy several blocks (I2), so a partial map cannot
    /// speak for the census of a block past its frontier. It does *not*
    /// qualify a **streamed** schema: that commits over exactly the blocks
    /// it will replay, every one of which is in the map and so censused
    /// (`docs/design/architecture.md`, "The array shape census").
    ///
    /// The caller always has `size` already — `crate::stream` stats the
    /// source before mapping, and the CLI before listing.
    pub fn is_complete(&self, size: u64) -> bool {
        self.scanned_through >= size
    }
}

/// Scan `source` end to end and build its full file map — [`crate::map::Builder`]
/// is fed the same [`Event`] stream as `CopyBlock` discovery, so this is one
/// pass, not two. `DumpIndex::metadata` is then [`crate::preamble::dump_metadata_from_spans`]
/// over the result, and [`DumpIndex::blocks`] a filter over it — neither is a
/// second scan (`docs/design/architecture.md`, "`DumpIndex`: one owner per fact").
pub async fn build_index(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<DumpIndex> {
    let mut spans = Builder::new();

    scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => spans.on_copy_start(start),
            Event::Row(row) => spans.on_row(row.raw),
            Event::CopyEnd(end) => spans.on_copy_end(end),
            Event::Line(line) => spans.feed_line(line.offset, line.raw),
            Event::DollarQuoteEnd(end) => spans.on_dollar_quote_end(end.offset),
            Event::LargeObjectStart(start) => spans.on_large_object_start(start.start_offset),
            Event::LargeObjectEnd(end) => spans.on_large_object_end(end.end_offset),
        }
        ControlFlow::Continue(())
    })
    .await?;

    let size = source.size().await?;
    let roles = spans.roles().clone();
    let tablespaces = spans.tablespaces().clone();
    let mut spans = spans.finish(size);
    let metadata = Some(dump_metadata_from_spans(&spans));
    attach_text(source, &mut spans).await?;
    let mut diagnostics = tiling_diagnostics(&spans, size);
    diagnostics.push(toc_coverage_diagnostic(&spans));
    Ok(DumpIndex { spans, scanned_through: size, metadata, roles, tablespaces, diagnostics })
}

/// Run the tiling check over a finished map and turn any failure into a
/// diagnostic (`docs/design/architecture.md`, "Testing philosophy").
///
/// A hole means *we* have a bug, not that the dump is bad, so the map is
/// still returned: refusing to answer "which roles does this file need" over
/// an accounting discrepancy serves nobody. The check is O(spans) against a
/// scan that just read the whole region, so it is free — and what it guards
/// is a silently dropped region on a dump shape no fixture covers, which is
/// exactly what a test cannot catch.
pub(crate) fn tiling_diagnostics(spans: &[Span], expected_end: u64) -> Vec<Diagnostic> {
    let issues = check_tiling(spans, expected_end);
    if issues.is_empty() { Vec::new() } else { vec![Diagnostic::tiling_broken(issues)] }
}

/// The TOC-coverage figure for a finished map: how many `spans` are
/// attributed to a TOC entry (`crate::map::Span::toc.is_some()` — a follow-on
/// statement that inherited its governing entry's header counts the same as
/// one whose own comment carried it, per "Span boundaries: statement-anchored,
/// object-attributed, greedy") against how many spans exist at all
/// (`docs/design/architecture.md`, "TOC enrichment"). Always produced, never conditionally — a
/// `pg_dump`-compatible file with zero TOC comments is a normal, reported
/// state (the map running in header-less degraded mode), not an error, so
/// `attributed == 0` is a legitimate value here rather than something this
/// function special-cases away.
pub(crate) fn toc_coverage_diagnostic(spans: &[Span]) -> Diagnostic {
    let attributed = spans.iter().filter(|s| s.toc.is_some()).count();
    Diagnostic::toc_coverage(attributed, spans.len())
}

/// Scan only far enough to recover the first database's preamble — up to
/// (not including) the first `COPY` block header in the file, or to EOF if
/// none exists. Per I1 (`docs/design/postgres-invariants.md`), nothing
/// `crate::preamble` cares about can follow that point for whichever
/// database is open when it's reached, and no database earlier in the file
/// (in a multi-`\connect` dump) can have a `COPY` block of its own before it
/// either — so this one offset always closes out the *first* database's
/// preamble, incidentally finishing any earlier, table-less database's too.
///
/// This bounded prepass (`docs/design/architecture.md`, "Bounded
/// preamble-only reads") exists so a scan that may stop anywhere still states
/// this metadata. `crate::stream::table_stream` needs it because the query's
/// own target table may start later in the file, or never appear at all;
/// `crate::stream::map_file` needs it because an interrupted `parse` would
/// otherwise bank blocks with no DDL behind them, and every column of them
/// would report `not declared` — the final answer — where the truth is
/// "finish the parse". See also `docs/design/architecture.md`, "The preamble
/// grammar and `DumpMetadata`".
///
/// Returns the recovered metadata, the spans tiling `[0, preamble_end)` (per
/// `crate::map::Builder` — no `Data` span among them, since the scan stops at
/// the first `COPY` header rather than walking into the block), that offset
/// itself — a safe watermark for a later scan to continue from, since no
/// `COPY` block starts before it — and whatever roles/tablespaces the
/// preamble region referenced (`DumpIndex::roles`/`tablespaces`'s own
/// partial-scan caveat applies here too).
pub(crate) async fn scan_preamble(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
) -> Result<(DumpMetadata, Vec<Span>, u64, BTreeSet<String>, BTreeSet<String>)> {
    let mut spans = Builder::new();
    let mut end = source.size().await?;
    scan(source, options, |event| match event {
        Event::CopyStart(start) => {
            // The map builder is deliberately not fed this event: it would
            // open a `Data` span this scan never closes (it stops here
            // rather than walking the block). If a TOC comment precedes the
            // header, `finish` below must not swallow it either — since
            // `map::Builder` absorbs such a comment straight into
            // the `Data` span a later, unfed-truncated scan produces
            // (`docs/design/architecture.md`, "Bulk regions: one span kind, three payloads"), so retreating to the comment's own start
            // leaves it for that scan rather than guessing it here as its
            // own `Framing`/`Unparsed` span.
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
        // I1: nothing this scan stops for (the first `COPY` header) can
        // follow a large-object region either — it sits even later in the
        // file (I12) — so in practice this scan always breaks before
        // reaching one. Fed through anyway rather than dropped by a bare `_`,
        // so a file with large objects but no `COPY` blocks at all still maps
        // that region as its own `Data` span instead of losing it into
        // whatever DDL span precedes it.
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

/// Answer from the preamble alone (`docs/design/architecture.md`,
/// "CLI surface", `--preamble-only`): reuse a cache's already-known metadata when
/// present, falling back to a fresh [`scan_preamble`] otherwise and
/// persisting the result when the cache is enabled (a no-op when it isn't —
/// see [`CacheMode::save`]). Bounded to the file's first `COPY` block
/// regardless of dump size (I1), independent of `build_index`'s full
/// structural scan.
///
/// A fresh scan's spans are persisted alongside the metadata, with a
/// trailing [`SpanBody::Unscanned`] span covering the rest of the file — this
/// is a genuinely partial scan (unlike `build_index`, which always reaches
/// EOF), so it's the one place today that produces that variant for real
/// rather than only in `crate::map`'s own unit tests
/// (`docs/design/architecture.md`, "The file map").
///
/// Also returns whatever [`CacheMode::load`] reported on the loaded index
/// (e.g. a `CacheMtimeChanged` warning) — the one library entry point that
/// answers with `DumpMetadata` alone rather than a whole `DumpIndex`, so its
/// diagnostics have nowhere else to travel back to the caller
/// (`docs/design/architecture.md`, "Diagnostics: one severity scale, two types").
pub async fn preamble_only(
    source: &dyn ByteRangeSource,
    options: &ScanOptions,
    cache: &CacheMode,
) -> Result<(DumpMetadata, Vec<Diagnostic>)> {
    let mut base_index = cache.load(source).await?.unwrap_or_default();
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
        cache.save(source, &base_index).await?;
    }
    Ok((base_index.metadata.unwrap_or_default(), base_index.diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape_of(fields: &[&str]) -> ArrayShape {
        let mut shape = ArrayShape::default();
        for field in fields {
            shape.observe(field.as_bytes());
        }
        shape
    }

    #[test]
    fn dimensionality_is_the_leading_brace_run() {
        assert_eq!(shape_of(&["{1,2}"]).dims, Some((1, 1)));
        assert_eq!(shape_of(&["{{1,2},{3,4}}"]).dims, Some((2, 2)));
        assert_eq!(shape_of(&["{{{1}}}"]).dims, Some((3, 3)));
    }

    /// The case a naive brace count gets wrong. `array_out` force-quotes any
    /// element containing a `{` (I25), so an element that *looks* like a
    /// nested array cannot extend the run — `{"c{d}"}` is one-dimensional
    /// and its single element is the four-character text `c{d}`.
    #[test]
    fn a_quoted_element_containing_a_brace_does_not_deepen_the_run() {
        assert_eq!(shape_of(&[r#"{"a,b","c{d}"}"#]).dims, Some((1, 1)));
        assert_eq!(shape_of(&[r#"{"{a}"}"#]).dims, Some((1, 1)));
    }

    /// The census's reason for storing both bounds: these two columns are
    /// indistinguishable by maximum alone, and only the first may become
    /// `List<List<T>>`.
    #[test]
    fn a_mixed_column_is_told_apart_from_a_uniformly_deep_one() {
        assert_eq!(shape_of(&["{{1,2},{3,4}}", "{{5,6}}"]).dims, Some((2, 2)));
        assert_eq!(shape_of(&["{1,2}", "{{1,2},{3,4}}"]).dims, Some((1, 2)));
    }

    /// Neither a SQL NULL nor the empty array constrains a column: `{}` is
    /// what `array_out` writes for an empty array of *any* dimensionality
    /// (I25), so a column holding only those two is still unconstrained —
    /// and a real value alongside them settles it alone.
    #[test]
    fn nulls_and_empty_arrays_contribute_no_dimensionality() {
        assert_eq!(shape_of(&["\\N", "{}"]).dims, None);
        assert_eq!(shape_of(&["{}", "{1,2,3}", "\\N"]).dims, Some((1, 1)));
    }

    #[test]
    fn a_lower_bound_prefix_is_recorded_beside_the_dimensionality() {
        let shape = shape_of(&["[0:2]={7,8,9}"]);
        assert!(shape.lower_bound_prefix);
        assert_eq!(shape.dims, Some((1, 1)));
        // Multi-dimensional, one bracket pair per dimension.
        let shape = shape_of(&["[1:2][0:1]={{1,2},{3,4}}"]);
        assert!(shape.lower_bound_prefix);
        assert_eq!(shape.dims, Some((2, 2)));
    }

    /// Combining blocks is min-of-mins, max-of-maxes, and a prefix anywhere
    /// counts everywhere — what a table occupying several blocks (I2)
    /// resolves against.
    #[test]
    fn merging_two_blocks_widens_the_bounds_rather_than_replacing_them() {
        let mut a = shape_of(&["{1,2}"]);
        a.merge(&shape_of(&["{{1,2},{3,4}}"]));
        assert_eq!(a.dims, Some((1, 2)));
        assert!(!a.lower_bound_prefix);

        // A block that constrained nothing leaves the answer alone, from
        // either side.
        let mut b = shape_of(&["{1,2}"]);
        b.merge(&shape_of(&["\\N", "{}"]));
        assert_eq!(b.dims, Some((1, 1)));
        let mut c = shape_of(&["\\N"]);
        c.merge(&shape_of(&["{{1,2}}"]));
        assert_eq!(c.dims, Some((2, 2)));

        // A prefix in one block disqualifies the column in every block.
        let mut d = shape_of(&["{1,2}"]);
        d.merge(&shape_of(&["[0:1]={7,8}"]));
        assert!(d.lower_bound_prefix);
    }

    /// The census runs over every field of a row it did not type-check, so
    /// the values of non-array columns reach it too. None of them may
    /// contribute: a composite opens with `(`, and a text value that merely
    /// starts with `[` has no `=`-then-`{` after it.
    #[test]
    fn a_field_that_is_not_an_array_literal_contributes_nothing() {
        for field in ["", "\\N", "42", "(1,2)", "[hello]", "[a=b]", "hello {world}", "[1:2]=x"] {
            assert_eq!(shape_of(&[field]), ArrayShape::default(), "{field}");
        }
    }
}
