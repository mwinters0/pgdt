//! The structural summary a scan produces: where every COPY block lives.
//!
//! This is what a `pgdt parse` scan yields and what the structure cache
//! (`cache.rs`) persists.

use std::collections::BTreeSet;
use std::ops::ControlFlow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::cache::{CacheLoad, CacheMode, SourceWatch};
use crate::copy::CopyHeader;
use crate::diagnostic::Diagnostic;
use crate::io::ByteRangeSource;
use crate::map::{Builder, DataBlock, Span, SpanBody, attach_text, check_tiling};
use crate::preamble::dump_metadata_from_spans;
use crate::scan::{Event, ScanOptions, scan};
use crate::statistics::{BlockStatistics, deserialize_block_statistics};

/// Dump-level preamble: source server version, `pg_dump` version, extension
/// list, user-defined type definitions. Populated by `crate::stream::build_index` via
/// `crate::preamble` (`docs/design/decisions.md`, "D36").
pub use crate::preamble::DumpMetadata;

/// The most dimensions PostgreSQL can give an array — `MAXDIM`, 6 in every
/// supported version (I25). A literal whose leading brace run is longer than
/// this did not come out of `array_out`, so a consumer must treat it as
/// unusable rather than as a depth.
pub const PG_ARRAY_MAX_DIMS: u8 = 6;

/// What one column's array values look like within one `COPY` block — the
/// shape census (`docs/design/decisions.md`, "D35").
///
/// Per column because an array's dimensionality belongs to the *value*
/// (I21), not to the DDL; per block because that is what the map extends
/// incrementally. Combining blocks is min-of-mins, max-of-maxes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrayShape {
    /// Fewest and most dimensions any value in this column carried, as
    /// `(min, max)`. `None` until a value contributes one — a SQL NULL never
    /// does, and neither does `{}`, which is what `array_out` writes for an
    /// empty array of *any* dimensionality (I25) and so fits every depth.
    /// Both bounds, never the maximum alone (`docs/design/decisions.md`,
    /// "D35").
    pub dims: Option<(u8, u8)>,
    /// Whether any value carried an explicit `[lb:ub]=` lower-bound prefix.
    /// Kept apart from `dims` because it disqualifies a column on its own:
    /// Arrow lists are 0-based, with nowhere to record an index origin.
    pub lower_bound_prefix: bool,
}

impl ArrayShape {
    /// Fold one still-COPY-escaped field into this column's census. The
    /// field is not decoded first: neither `{` nor the `[lb:ub]=` prefix is
    /// in COPY TEXT's escape set (I15). A field that does not open with `{`,
    /// past any `[lb:ub]=` prefix, contributes nothing; one that does counts
    /// whatever its column's type, the census being type-blind (D35) and read
    /// only for a column resolved to an array.
    ///
    /// Dimensionality is the **leading brace run** (I25): `array_out` opens
    /// with exactly `ndim` braces and force-quotes any element containing a
    /// `{`, so no element can extend the run.
    pub(crate) fn observe(&mut self, field: &[u8]) {
        let mut rest = field;
        if rest.first() == Some(&b'[') {
            // `[lb:ub]…=` — the prefix runs to the `=` that introduces the
            // value proper. A prefix always has a real array after it
            // (`array_out` writes a bare `{}` before it formats dimensions);
            // anything else here is not array output and is left alone.
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
    /// min-of-mins, max-of-maxes, a lower-bound prefix anywhere counting
    /// everywhere. One table's data can occupy several blocks (I2).
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

/// Which limit a value is past, where its column's Arrow type cannot hold it
/// (`docs/design/decisions.md`, "D96").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnrepresentableTier {
    /// Within Arrow's format spec, and past the calendar [`calendar_end`]
    /// ends — a `date` or timestamp a DataFusion query cannot display, which
    /// only the provider reads as unrepresentable.
    Engine,
    /// Past what Arrow's format spec lets the type hold, which every layer
    /// reads: `±infinity`, `NaN` under a typmod, `time` `24:00:00`, an
    /// `interval` time part past nanoseconds, a timestamp past `i64`
    /// microseconds from 1970.
    Format,
}

/// One column's count of values its Arrow type cannot hold in one `COPY`
/// block, each value in the one tier it is past — the format spec's
/// dominating (`docs/design/decisions.md`, "D96"). Beside the array-shape
/// census, and at the same level: a block mapped at the metadata level holds
/// neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unrepresentable {
    /// Values past Arrow's format spec.
    pub format: u64,
    /// Values within it and past the engine's calendar.
    pub engine: u64,
}

impl Unrepresentable {
    /// Count one value past `tier`.
    pub(crate) fn add(&mut self, tier: UnrepresentableTier) {
        match tier {
            UnrepresentableTier::Format => self.format += 1,
            UnrepresentableTier::Engine => self.engine += 1,
        }
    }

    /// Fold another block's, or a piece's, count for the same column in.
    pub fn merge(&mut self, other: &Unrepresentable) {
        self.format += other.format;
        self.engine += other.engine;
    }

    /// Whether no value was counted in either tier.
    pub fn is_zero(&self) -> bool {
        self.format == 0 && self.engine == 0
    }
}

/// **The last day of the calendar `arrow-cast` formats a `date` or timestamp
/// through**, as a `Date32` day count from 1970 — `chrono`'s, whose end is the
/// engine tier's bound (RT21). A cache records the bound it counted under and
/// is refused by a build whose bound is another
/// (`crate::cache::Unusable::CalendarChanged`), so a `chrono` upgrade moving
/// it reaches no count taken before it.
// upstream: UF2
pub fn calendar_end() -> i32 {
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("the Unix epoch is a date");
    let days = chrono::NaiveDate::MAX.signed_duration_since(epoch).num_days();
    i32::try_from(days).expect("chrono's calendar ends inside Date32's range")
}

/// What one pass over a block's rows records beside its rows' extent: the
/// array-shape census and the unrepresentable count, one entry each per
/// column in `header.columns` order — the data level's two query affordances
/// (`docs/design/decisions.md`, "D35", "D96"). The census grows for a row
/// wider than its header; the count never does, a column past the header
/// having no declared type to be tested against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockCensus {
    pub shapes: Vec<ArrayShape>,
    pub unrepresentable: Vec<Unrepresentable>,
}

impl BlockCensus {
    /// Nothing folded yet, for a block `columns` wide.
    pub(crate) fn new(columns: usize) -> Self {
        BlockCensus {
            shapes: vec![ArrayShape::default(); columns],
            unrepresentable: vec![Unrepresentable::default(); columns],
        }
    }

    /// Fold another piece's, or another read's, census of the same block in —
    /// length-tolerant, since a row wider than its header grows the census.
    pub(crate) fn merge(&mut self, other: &BlockCensus) {
        if other.shapes.len() > self.shapes.len() {
            self.shapes.resize(other.shapes.len(), ArrayShape::default());
        }
        for (slot, shape) in self.shapes.iter_mut().zip(&other.shapes) {
            slot.merge(shape);
        }
        if other.unrepresentable.len() > self.unrepresentable.len() {
            self.unrepresentable.resize(other.unrepresentable.len(), Unrepresentable::default());
        }
        for (slot, count) in self.unrepresentable.iter_mut().zip(&other.unrepresentable) {
            slot.merge(count);
        }
    }
}

/// The census of several blocks read as one, one entry per name in `columns`
/// — what a table spanning more than one `COPY` block (I2) resolves against,
/// and what a query's schema commits to over exactly the blocks it will
/// replay (`docs/design/decisions.md`, "D35").
///
/// **Keyed by column name, not by position**: a leaf partition's block lists
/// its columns in the leaf's own order, which need not be the root's or a
/// sibling's. A name a block carries and `columns` does not contributes
/// nothing, and so does a census entry past its header's list — a block
/// naming no columns copies none (I5). A block holding no census contributes
/// nothing either, so a caller believing the union has first made sure every
/// block holds one.
pub fn union_census<'a>(
    columns: &[String],
    blocks: impl IntoIterator<Item = &'a CopyBlock>,
) -> Vec<ArrayShape> {
    let mut out = vec![ArrayShape::default(); columns.len()];
    for block in blocks {
        let census = block.array_shapes.iter().flatten();
        for (name, shape) in block.header.columns.iter().zip(census) {
            let slot = columns.iter().position(|c| c == name);
            if let Some(slot) = slot.and_then(|slot| out.get_mut(slot)) {
                slot.merge(shape);
            }
        }
    }
    out
}

/// One located COPY block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyBlock {
    pub header: CopyHeader,
    /// The database this block belongs to — the name from the `\connect`
    /// governing it. `None` means the file had no `\connect` at all (a plain
    /// dump), which falls back to the single unnamed [`crate::preamble::DatabaseMetadata`].
    /// Not an ordinal into `metadata.databases`: an incremental scan's
    /// metadata can hold one entry however many databases the file contains
    /// (`docs/design/decisions.md`, "D49").
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
    /// in this same dump carry the same header name**. Stored, not concluded
    /// from (`docs/design/decisions.md`, "D74"), and read by
    /// `crate::stream::table_stream` to decide whether a cold query may stop
    /// once the queried table's block closes or must run to EOF, those blocks
    /// not being adjacent (`docs/design/decisions.md`, "D49").
    pub partition_root: Option<String>,
    /// This block's per-row-group column statistics, where the mapping pass
    /// that recorded it was asked to gather them (`crate::statistics`).
    /// Shared, so a clone of the map copies a reference
    /// (`docs/design/decisions.md`, "D34").
    #[serde(deserialize_with = "deserialize_block_statistics")]
    pub statistics: Option<Arc<BlockStatistics>>,
    /// **The statistics allowance this block declined to gather under**, where
    /// a mapping pass could not hold what it asked for
    /// (`docs/design/decisions.md`, "D85"): an absence and a number, which is
    /// what stops the next pass at the same allocation re-reading the block
    /// and declining again. `None` is a block that declined nothing — every
    /// block a pass gathering under no allowance maps, and every one whose
    /// gather or re-read succeeded.
    ///
    /// **It is not exclusive with `statistics`**: a block already holding
    /// statistics from an earlier pass keeps them when a re-read for a
    /// *different* request declines, and carries this beside them.
    #[serde(default)]
    pub statistics_declined: Option<u64>,
    /// This block's array-shape census, one [`ArrayShape`] per column in
    /// `header.columns` order, and `None` for a block mapped at the
    /// [`crate::StatisticsLevel::Metadata`] level, which records nothing drawn
    /// from its rows (`docs/design/decisions.md`, "D35"). A column that saw no
    /// array-shaped literal holds the default shape, and the vector is empty
    /// only for a header naming no columns. Shorter than `header.columns`
    /// never happens; *longer* only where a row is wider than its header,
    /// which a query refuses.
    pub array_shapes: Option<Vec<ArrayShape>>,
    /// This block's unrepresentable count, one [`Unrepresentable`] per column
    /// in `header.columns` order, and `None` exactly where `array_shapes` is:
    /// every pass that censuses a block counts it
    /// (`docs/design/decisions.md`, "D96").
    pub unrepresentable: Option<Vec<Unrepresentable>>,
}

impl CopyBlock {
    /// Record `census` — what a pass reading every row of this block found —
    /// which puts the block at the data level.
    pub fn set_census(&mut self, census: BlockCensus) {
        self.array_shapes = Some(census.shapes);
        self.unrepresentable = Some(census.unrepresentable);
    }
}

/// One table as its `COPY` headers name it: the database whose `\connect`
/// governs the block, the schema qualifier and the table, each exactly as the
/// header spelled it once unquoted.
///
/// **An exact key, never a pattern.** [`DumpIndex::blocks_for`] matches a name
/// a person typed, bare or qualified, and splits it at its first `.`; this is
/// what a caller that read the table off the map already holds, so nothing is
/// split and no two tables can answer to it (`docs/design/decisions.md`,
/// "D49").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableName {
    pub database: Option<String>,
    pub schema: Option<String>,
    pub table: String,
}

impl TableName {
    /// The table `block` holds rows for.
    pub fn of(block: &CopyBlock) -> Self {
        Self {
            database: block.database.clone(),
            schema: block.header.schema.clone(),
            table: block.header.table.clone(),
        }
    }

    /// Whether `block` holds rows for this table.
    pub fn names(&self, block: &CopyBlock) -> bool {
        self.database == block.database
            && self.schema == block.header.schema
            && self.table == block.header.table
    }

    /// `schema.table`, or `table` where no schema qualifies it — the header's
    /// own [`CopyHeader::qualified_name`], not re-quoted.
    pub fn qualified(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.table),
            None => self.table.clone(),
        }
    }
}

/// The full file map discovered in a dump, in file order
/// (`docs/design/decisions.md`, "D34"). [`DumpIndex::blocks`] is a derived
/// filter over it, not a second stored structure.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpIndex {
    pub spans: Vec<Span>,
    /// How much of the file the scan covered. Equal to the file size after an
    /// eager scan.
    pub scanned_through: u64,
    /// Dump-level preamble metadata — see [`DumpMetadata`]. `None` only for
    /// a `DumpIndex` built by hand rather than by `crate::stream::build_index`. A derived
    /// view over `spans` (`crate::preamble::dump_metadata_from_spans`),
    /// computed once (`docs/design/decisions.md`, "D34").
    pub metadata: Option<DumpMetadata>,
    /// Roles referenced anywhere the scan has reached — the TOC `Owner:`
    /// field, `ALTER ... OWNER TO`, and `GRANT`/`REVOKE`/`ALTER DEFAULT
    /// PRIVILEGES FOR ROLE` (`docs/design/decisions.md`,
    /// "D31"). `PUBLIC` is never included. Flat and per-file — a
    /// per-database view is a filter over `Span::database`. Persisted, unlike
    /// `diagnostics` (`docs/design/decisions.md`, "D34"), and
    /// complete only once `scanned_through` reaches the file's size.
    pub roles: BTreeSet<String>,
    /// Tablespaces referenced anywhere the scan has reached — the TOC
    /// `Tablespace:` field and `SET default_tablespace = ...;`. `pg_default`
    /// is never included. Same flatness, persistence and partial-scan caveat
    /// as [`roles`](Self::roles).
    pub tablespaces: BTreeSet<String>,
    /// Things worth telling the caller that have no `Result` to travel in —
    /// a tiling failure, a cache mtime mismatch. **Not persisted**
    /// (`#[serde(skip)]`, `docs/design/decisions.md`, "D34"): recomputed
    /// wherever an index is mapped or loaded. A preamble-only run maps
    /// only the preamble, so it returns a loaded cache's figures as they were, or none
    /// but its source's. See [`crate::diagnostic`].
    #[serde(skip)]
    pub diagnostics: Vec<Diagnostic>,
}

impl DumpIndex {
    /// The `COPY` blocks among `spans`, in file order — a filtered view, not
    /// a stored field, so a block's byte offsets have exactly one owner
    /// (`docs/design/decisions.md`, "D34").
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

    /// Every table this map holds a `COPY` block for, each once, in the order
    /// its first block appears.
    pub fn tables(&self) -> Vec<TableName> {
        let mut seen = BTreeSet::new();
        self.blocks().map(TableName::of).filter(|name| seen.insert(name.clone())).collect()
    }

    /// The blocks of exactly `table`, in file order.
    pub fn blocks_of<'a>(&'a self, table: &'a TableName) -> impl Iterator<Item = &'a CopyBlock> {
        self.blocks().filter(move |b| table.names(b))
    }

    /// The heap every block's statistics hold
    /// ([`BlockStatistics::heap_bytes`]) — what a mapping pass bills
    /// for the statistics a cache handed it, and what a caller holding this
    /// map resident bills for keeping them.
    pub fn statistics_heap_bytes(&self) -> u64 {
        self.blocks()
            .filter_map(|block| block.statistics.as_deref())
            .map(BlockStatistics::heap_bytes)
            .sum()
    }

    pub fn total_rows(&self) -> u64 {
        self.blocks().map(|b| b.row_count).sum()
    }

    /// Whether this scan reached the end of a `size`-byte file — the test
    /// that decides when a whole-file fact may be believed.
    /// [`roles`](Self::roles), [`tablespaces`](Self::tablespaces) and a
    /// **reported** table schema carry that partiality; a **streamed** schema
    /// is not gated on it, committing over exactly the blocks it will replay
    /// (`docs/design/decisions.md`, "D34", "D35"). The caller always has
    /// `size` already.
    pub fn is_complete(&self, size: u64) -> bool {
        self.scanned_through >= size
    }
}

/// Run the tiling check over a finished map and turn any failure into a
/// diagnostic; the map is still returned
/// (`docs/design/decisions.md`, "D30").
pub(crate) fn tiling_diagnostics(spans: &[Span], expected_end: u64) -> Vec<Diagnostic> {
    let issues = check_tiling(spans, expected_end);
    if issues.is_empty() { Vec::new() } else { vec![Diagnostic::tiling_broken(issues)] }
}

/// The TOC-coverage figure for a finished map: how many `spans` are
/// attributed to a TOC entry (`crate::map::Span::toc.is_some()` — an
/// inherited entry counts the same as an owned one) against how many spans
/// exist at all (`docs/design/decisions.md`, "D31"). Always produced:
/// `attributed == 0` is a legitimate value, a file with zero TOC comments
/// being the map's header-less degraded mode rather than an error.
pub(crate) fn toc_coverage_diagnostic(spans: &[Span]) -> Diagnostic {
    let attributed = spans.iter().filter(|s| s.toc.is_some()).count();
    Diagnostic::toc_coverage(attributed, spans.len())
}

/// D19's warning for a `.xz` source with no usable seek structure — `None`
/// when there is no compression layer at all (`table` is `None`) or the table
/// already has more than one block. One function for both callers: a live
/// [`ByteRangeSource`] (`crate::stream::build_index`, [`preamble_only`]) and a persisted
/// cache (`crate::cache::status_from_file`).
pub(crate) fn non_seekable_compression_diagnostic(
    table: Option<&xz_seek::SeekTable>,
) -> Option<Diagnostic> {
    let table = table?;
    if table.is_seekable() {
        return None;
    }
    Some(Diagnostic::non_seekable_compressed_source(table.block_count()))
}

/// Scan only far enough to recover the first database's preamble — up to
/// (not including) the first `COPY` block header in the file, or to EOF if
/// none exists. Per I1 nothing `crate::preamble` cares about can follow that
/// point for the database open when it is reached, and no earlier database in
/// a multi-`\connect` dump has a `COPY` block before it, so this one offset
/// closes out the *first* database's preamble. The bounded prepass is what
/// lets a scan that may stop anywhere still state this metadata
/// (`docs/design/decisions.md`, "D30", "D36").
///
/// Returns the recovered metadata, the spans tiling `[0, preamble_end)` (no
/// `COPY` block among them, the scan stopping at the first `COPY` header
/// rather than walking into the block — though a file with large objects and
/// no `COPY` block maps that region as a `Data` span), that offset itself — a safe watermark
/// for a later scan to continue from — and whatever roles/tablespaces the
/// preamble region referenced (`DumpIndex::roles`/`tablespaces`'s own
/// partial-scan caveat applies).
pub(crate) async fn scan_preamble(
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
            // span (`docs/design/decisions.md`, "D32", "D33").
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
        // Refused before a byte of the dump is read, unless the caller said
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
    // A complete cache already carries this diagnostic, computed by
    // `cache::status_from_file` off the persisted table — so this is
    // idempotent rather than gated on `!known`, which would miss a cache
    // that exists but whose preamble was not yet complete.
    if let Some(d) = non_seekable_compression_diagnostic(source.seek_table().as_ref())
        && !base_index.diagnostics.contains(&d)
    {
        base_index.diagnostics.push(d);
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

    /// `array_out` force-quotes any element containing a `{` (I25), so an
    /// element that *looks* like a nested array cannot extend the run:
    /// `{"c{d}"}` is one-dimensional.
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

    /// Neither a SQL NULL nor the empty array constrains a column — `{}`
    /// being what `array_out` writes at *any* dimensionality (I25) — and a
    /// real value alongside them settles it alone.
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
    /// non-array values reach it, and none of these may contribute: a
    /// composite opens with `(`, and a text value starting `[` has no
    /// `=`-then-`{`. A text value opening with `{` does count, and is read
    /// only if its column resolves to an array.
    #[test]
    fn a_field_that_is_not_an_array_literal_contributes_nothing() {
        for field in ["", "\\N", "42", "(1,2)", "[hello]", "[a=b]", "hello {world}", "[1:2]=x"] {
            assert_eq!(shape_of(&[field]), ArrayShape::default(), "{field}");
        }
    }
}
