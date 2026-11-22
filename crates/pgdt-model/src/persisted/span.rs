//! The file map as the cache persists it: the [`Span`]s tiling a dump, what
//! each holds, the `COPY` blocks among them and the [`DumpIndex`] carrying
//! them (`docs/design/decisions.md`, "D30", "D34"). What reads them is
//! `crate::span`'s.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::block::BlockCensus;
use super::catalog::{CollationDef, DumpMetadata, TableDef, TableReference, TypeKind};
use super::statistics::{BlockStatistics, IgnoredRefusals, deserialize_block_statistics};
use super::timescaledb::RecordedRows;
use crate::Diagnostic;
use pgdt_core::CopyHeader;

/// One tile of the full file map. `start`/`end` are absolute file offsets;
/// `[start, end)` never overlaps another span's range, and every span
/// together tiles the file, a partial scan's unread rest one `Unscanned`
/// span — or tiles it to the scan's watermark where the file's end is not
/// known yet — see `pgdt_dump::check_tiling`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u64,
    pub end: u64,
    /// The database this span belongs to — the name from the governing
    /// `\connect`, or `None` for a plain dump (or content before any
    /// `\connect`), which falls back to the single unnamed
    /// [`crate::DatabaseMetadata`]. Not an ordinal into `metadata.databases`:
    /// an incremental scan's metadata can hold one entry however many
    /// databases the file contains. **Stored here alone**, a `COPY` block's
    /// included (`docs/design/decisions.md`, "D34"):
    /// [`DumpIndex::blocks_with_database`] pairs a block with it.
    pub database: Option<String>,
    /// The span's own bytes, sliced from the file by offset and stored in the
    /// cache so `pgdt info` answers without re-reading the dump
    /// (`docs/design/decisions.md`, "D30"). `None` until `pgdt_dump::attach_text` has
    /// run, and permanently `None` for [`SpanBody::Data`] and
    /// [`SpanBody::Unscanned`] spans, whose bytes are unbounded.
    pub text: Option<SpanText>,
    /// The TOC entry this span **belongs to** — not necessarily the comment
    /// this span's own bytes start with. A follow-on statement with no TOC
    /// comment of its own **inherits** the governing entry's header rather
    /// than carrying `None`, and [`toc_owned`](Self::toc_owned) distinguishes
    /// the two (`docs/design/decisions.md`, "D31"). `None` for a span with no
    /// governing entry at all: the header-less-input fallback, or a span of
    /// one of the kinds inheritance never crosses (`Framing`, `Connect`,
    /// `VersionHeader`) carrying no TOC comment of its own, or a `COPY` block's or large-object region's, with
    /// no TOC comment of its own, or one whose own TOC-shaped comment did not
    /// parse, or a `Framing` statement's, whose header is vetoed even where it
    /// has its own. Not a substitute for
    /// [`SpanBody`]'s own per-kind
    /// fields: the two are separately-sourced observations of one object
    /// (`docs/design/decisions.md`, "D30").
    pub toc: Option<TocHeader>,
    /// Whether *this span's own* preceding comment carried the TOC header
    /// text (`true`), as opposed to `toc` being inherited from an earlier
    /// entry's span (`false`) — `false` when `toc` is `None`, but for a comment
    /// block shaped like a TOC entry whose header did not parse and that a
    /// statement closes, and `false` for a statement classified `Framing`,
    /// whose header is vetoed (`docs/design/decisions.md`, "D31"). An object
    /// census (`pgdt info`'s `object kinds:`) counts `toc_owned` spans, one
    /// per archive entry, while TOC coverage counts every attributed span
    /// (`toc.is_some()`), inherited ones included
    /// (`docs/design/decisions.md`, "D31").
    pub toc_owned: bool,
    pub body: SpanBody,
}

/// The fields of a TOC header comment (I3/I16) — `-- Name: <name>; Type:
/// <kind>; Schema: <schema>; Owner: <owner>[; Tablespace: <tablespace>]`, or
/// its `-- Data for Name: ...` sibling ahead of a `COPY` block. Parsed by
/// `pgdt_dump::map::toc::parse_toc_header_line`; see pgdt-dump's `map.rs`
/// module doc, its "TOC enrichment" bullet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TocHeader {
    /// `te->tag`, sanitized — the object's own name as `pg_dump` wrote it,
    /// which for most kinds is unqualified (just `widgets`, not
    /// `objects.widgets`) unlike a classified [`SpanBody`]'s own `name` field.
    pub name: String,
    /// `te->desc` — one of the closed vocabulary I16 documents (`TABLE`,
    /// `FK CONSTRAINT`, `POLICY`, `TABLE DATA`, ...). Never `None`:
    /// `_printTocEntry()` always writes a `Type:` field.
    pub kind: String,
    /// `None` for `Schema: -` (no namespace — a database-level or
    /// namespace-less object).
    pub schema: Option<String>,
    /// `None` for `Owner: -` (`--no-owner`, or an entry with no owner) and
    /// for an empty `Owner: ` (`COMMENT ON EXTENSION`'s, I18): "no owner
    /// recorded here" to a reader.
    pub owner: Option<String>,
    /// The `; Tablespace: <name>` suffix, present only when the entry has a
    /// non-default tablespace and `--no-tablespaces` was not given.
    pub tablespace: Option<String>,
}

/// A span's stored bytes. Capped at
/// [`SPAN_STORED_TEXT_MAX_BYTES`](crate::SPAN_STORED_TEXT_MAX_BYTES): one
/// pathological function body must not make the cache unbounded, and the
/// offsets are kept regardless, so a caller that needs the rest can always
/// read the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanText {
    /// Lossy UTF-8 of the span's bytes — truncated to the cap, if it hit one.
    pub text: String,
    /// Whether bytes were dropped to fit the cap.
    pub truncated: bool,
}

/// The payload behind [`SpanBody::Data`] — see that variant's docs for why
/// one `Data` span kind covers all three producers despite them not sharing a
/// shape. `Copy` holds its block unboxed: a block is smaller than
/// [`SpanBody::Table`], so a box would not shrink a span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataBlock {
    Copy(CopyBlock),
    InsertRun(InsertRun),
    LargeObjects(LargeObjectRegion),
}

/// A run of `pg_dump --inserts`/`--column-inserts` output for one table —
/// `INSERT INTO <table> ...;` statements, of one row each or, under
/// `--rows-per-insert`, several, merged into a single
/// `Data` span instead of one `Unparsed` span per statement. No inner
/// offsets, nothing reading rows out of this yet
/// (`docs/design/decisions.md`, "D33").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertRun {
    /// The table every statement in this run targets, as its own first
    /// `INSERT INTO` line named it — schema-qualified when the line was,
    /// matching [`CopyHeader::qualified_name`](pgdt_core::CopyHeader::qualified_name)'s
    /// convention for the same object.
    pub table: String,
    /// What the pass found: its `row_count` the `INSERT` statements folded
    /// into this span — free to compute, finding the run's end meaning
    /// recognizing each completion — and nothing else yet.
    pub record: BlockRecord,
}

/// The large-object data region (I12) — every `BEGIN;`/`COMMIT;`-wrapped
/// `lo_open`/`lowrite`/`lo_close` run in the file, merged into a single `Data`
/// span regardless of how many archive entries `pg_dump` split it across (one
/// on v13-16, one per object on v17+ — see `pgdt_dump::internal::Builder::on_large_object_start`).
/// No identity at all: the region's only identity is the OID in each
/// `lo_create('<oid>')` opener, and recovering it costs a walk of the whole
/// region (`docs/design/decisions.md`, "D33").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LargeObjectRegion {}

/// What a span is, at the granularity the statement-driven pass (no
/// TOC enrichment) can tell — see `pgdt_dump::map`'s module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanBody {
    Table {
        name: String,
        definition: TableDef,
    },
    TypeDef {
        name: String,
        kind: TypeKind,
    },
    Extension {
        name: String,
        schema: Option<String>,
    },
    /// A `CREATE COLLATION` — distinct from [`Unparsed`](SpanBody::Unparsed)
    /// because `texteq` on a column of a non-deterministic collation is not a
    /// byte comparison, and this statement is the only place a plain dump says
    /// a collation is one (I42).
    Collation {
        collation: CollationDef,
    },
    /// A bulk region — a `COPY` block, an `INSERT` run, or the large-object
    /// data region. One span kind for all three; [`DataBlock`] is where they
    /// stop sharing a shape, only [`DataBlock::Copy`] carrying the inner
    /// offsets a row reader seeks by (`docs/design/decisions.md`, "D33").
    Data(DataBlock),
    /// A `\connect <name>` meta-command — distinct from
    /// [`Framing`](SpanBody::Framing) because
    /// `pgdt_dump::dump_metadata_from_spans` needs the database name
    /// itself to reconstruct `DumpMetadata`'s per-database segmenting.
    Connect {
        database: String,
    },
    /// The dump's (or a `\connect`ed database's) own two-line version-header
    /// comment block (I9): `-- Dumped from database version ...` / `-- Dumped
    /// by pg_dump version ...`. Distinct from [`Framing`](SpanBody::Framing)
    /// for the same reason [`Connect`](SpanBody::Connect) is: the derived view
    /// needs the strings themselves.
    VersionHeader {
        server_version: Option<String>,
        pg_dump_version: Option<String>,
    },
    /// A `--binary-upgrade` dump's `ALTER TYPE <name> ADD VALUE '<label>';`
    /// (I6), or a hand-written one placing the label `BEFORE` or `AFTER`
    /// another — recognized so `pgdt_dump::dump_metadata_from_spans` can
    /// fold the label into the [`TypeDef`](SpanBody::TypeDef) span it
    /// targets. `placement` is `None` for the label appended, which is all
    /// `pg_dump` writes.
    AlterTypeAddValue {
        type_name: String,
        label: String,
        placement: Option<LabelPlacement>,
    },
    /// A `--binary-upgrade` dump's `ALTER TYPE <name> DROP ATTRIBUTE
    /// <attr>;`, dropping a composite's placeholder for an attribute the type
    /// has dropped — recognized so
    /// `pgdt_dump::dump_metadata_from_spans` can remove the field from
    /// the [`TypeDef`](SpanBody::TypeDef) span it targets.
    AlterTypeDropAttribute {
        type_name: String,
        attribute: String,
    },
    /// An `ALTER TYPE` that could change an enum's labels and that
    /// `pgdt_dump::ddl` does not read — `RENAME VALUE`, `ADD VALUE IF NOT
    /// EXISTS`, a label no plain `'…'` literal spells — naming the type it alters, or `None`
    /// where it names none this grammar reads. No `pg_dump` writes one; it is
    /// recognized so `pgdt_dump::dump_metadata_from_spans` can mark
    /// the labels it may have changed inexact.
    EnumLabelsUnread {
        type_name: Option<String>,
    },
    /// An `ALTER TABLE` adding to a table what
    /// `pgdt_dump`'s `ddl::table::parse_alter_table_reference` reads — a
    /// `--binary-upgrade` dump's `INHERIT <parent>`, `OF <type>` or `ADD
    /// CONSTRAINT … CHECK`, a pre-v18 `SET NOT NULL`, an `ATTACH PARTITION`,
    /// whose `table` is the partition it attaches — recognized so
    /// `pgdt_dump::dump_metadata_from_spans` can fold the reference
    /// into the [`Table`](SpanBody::Table) span it targets.
    AlterTableReference {
        table: String,
        reference: TableReference,
    },
    /// File prologue/epilogue framing (the `PostgreSQL database dump`
    /// banner, `\restrict`/`\unrestrict`, the `SET`/`set_config` preamble
    /// block, ...) — never a real database object, and none of the more
    /// specific framing-adjacent kinds above.
    Framing,
    /// A statement — complete (trailing `;`, parens/quotes balanced), or
    /// closed early by a `--` line or a dollar body's end — or a TOC comment
    /// no statement followed, that is none of this module's classified shapes — including every object kind
    /// TOC enrichment would otherwise label, and any statement not grouped
    /// into its owning entry's span (`pgdt_dump::map`'s module docs, "no grouping").
    Unparsed,
    /// Bytes no scan has walked yet — always a single trailing span
    /// (`docs/design/decisions.md`, "D30"). Never produced by `pgdt_fixtures::build_map`,
    /// which always scans to EOF; an incremental scan is what emits one.
    Unscanned,
}

/// Where an `ALTER TYPE … ADD VALUE` places its label: `BEFORE` or `AFTER`
/// the existing label named, which the server places it beside in the type's
/// sort order. No `pg_dump` writes one; a hand-written dump can.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LabelPlacement {
    Before(String),
    After(String),
}

/// One located COPY block: what it holds rows *of*, where it is, and what a
/// pass found reading it — each its own part, so a block can be addressed and
/// recorded apart (`docs/design/decisions.md`, "D168").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyBlock {
    pub header: CopyHeader,
    /// The root table named by this block's `-- load via partition root
    /// <name>` marker, if it carried one (I2) — meaning `header` names that
    /// root rather than the partition whose rows follow, and **other blocks
    /// in this same dump carry the same header name**. Stored, not concluded
    /// from (`docs/design/decisions.md`, "D74"), and read by
    /// `pgdt_mapping`'s `target_settled` to decide whether a cold query may stop
    /// once the queried table's block closes or must run to EOF, those blocks
    /// not being adjacent (`docs/design/decisions.md`, "D49").
    pub partition_root: Option<String>,
    /// Where the block is in the dump.
    pub address: BlockAddress,
    /// What the passes that read the block found. **Shared, and changed by
    /// copying on write** ([`Arc::make_mut`]): a clone of the block or the map
    /// copies a reference, and each site changing a record — the back-fill, a
    /// query's census, the cache dropping statistics it loaded — copies that
    /// record alone (`docs/design/decisions.md`, "D168").
    pub record: Arc<BlockRecord>,
}

/// **Where a `COPY` block is in a plain dump**: its four absolute file
/// offsets, its identity derived from the header's ([`BlockAddress::id`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockAddress {
    /// Absolute file offset of the `C` in `COPY`.
    pub header_offset: u64,
    /// Absolute file offset of the block's first data byte.
    pub data_offset: u64,
    /// Absolute file offset of the `\.` terminator line.
    pub terminator_offset: u64,
    /// Absolute file offset just past the terminator line.
    pub end_offset: u64,
}

/// **What a pass found reading a block's rows**: their count, and whatever
/// else the pass recorded of them. Carries no address: every offset in it is
/// relative to the block's first data byte, so it means the same wherever
/// the block is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRecord {
    pub row_count: u64,
    /// This block's per-row-group column statistics, where the mapping pass
    /// that recorded it was asked to gather them (`pgdt_mapping::statistics`).
    /// Shared, so a copy of the record copies a reference
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
    /// **The fields PostgreSQL refuses, by type or `NOT NULL`, that a parse
    /// told to ignore them went past in this block** — per column, its first, quoted, and a count
    /// — and `None` where no pass gathering it met one so. Recorded by the
    /// mapping pass and merged with a back-fill's re-read
    /// ([`IgnoredRefusals::merge`]); a default parse over this map fails
    /// with it where it tracks such a column, a strict one re-reading the block
    /// ([`pgdt_core::Error::FieldRefusedRecorded`]).
    #[serde(default)]
    pub ignored_refusals: Option<Box<IgnoredRefusals>>,
    /// **Whether a strict parse checked every field of this block**
    /// ([`crate::PostgresInvalidValues::Strict`]), mapping or re-reading it,
    /// and found none refused — a fact about the dump's bytes, which the
    /// cache's identity binds it to. A strict parse re-reads every block
    /// holding `false`, so a clean one has checked every field whichever runs
    /// built the map; `info --detail` and `--json` report it.
    #[serde(default)]
    pub checked_in_full: bool,
    /// What a pass reading every row of this block found — its array-shape
    /// census and its unrepresentable count — and `None` for a block mapped
    /// at the `pgdt_mapping::statistics::StatisticsLevel::Metadata` level,
    /// which records no census of its rows
    /// (`docs/design/decisions.md`, "D35", "D96"). Recording one puts the
    /// block at the data level.
    pub census: Option<BlockCensus>,
    /// **The block's rows as text, where a pass records them**: TimescaleDB's
    /// catalog ([`crate::timescaledb::records_rows`]), recorded at every
    /// level by every pass that maps the block, and read by
    /// [`crate::timescaledb::TimescaleDb::of`] (`docs/design/decisions.md`,
    /// "D133"). `None` for every other block.
    #[serde(default)]
    pub recorded_rows: Option<Box<RecordedRows>>,
    /// **The block's longest data line**, recorded at every level by every
    /// pass that maps the block, which finds each line's end whether or not
    /// it holds the line; `None` for a block holding no row. A fact of the
    /// dump's bytes, never what reading the line costs
    /// (`docs/design/decisions.md`, "D140").
    pub longest_line: Option<LongestLine>,
}

/// **A `COPY` block's longest data line**: where it starts and its length as
/// I71 measures a line, its line ending included — the earliest of them where
/// several are as long (`docs/design/decisions.md`, "D140").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LongestLine {
    /// Where the line's first byte is, relative to the block's first data
    /// byte.
    pub offset: u64,
    /// The line's bytes, its line ending included.
    pub len: u64,
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
    /// Dump-level preamble metadata — see [`DumpMetadata`]. `None` for a
    /// `DumpIndex` built by hand without it, and for a mapping run interrupted before
    /// its preamble prepass was whole. A derived
    /// view over `spans` (`pgdt_dump::dump_metadata_from_spans`),
    /// computed once (`docs/design/decisions.md`, "D34"), and shared by `Arc`
    /// with every table and plan read over the map rather than copied into
    /// each.
    pub metadata: Option<Arc<DumpMetadata>>,
    /// Roles referenced anywhere the scan has reached — the TOC `Owner:`
    /// field, `ALTER ... OWNER TO`, and `GRANT`/`REVOKE`/`ALTER DEFAULT
    /// PRIVILEGES FOR ROLE`. The pseudo-role `PUBLIC` is never included. Flat and per-file — a
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
    /// wherever a whole index is mapped or loaded; an interrupted mapping
    /// run's carries none. A preamble-only run maps
    /// only the preamble, so it returns a loaded cache's figures as they were, or none
    /// but its source's. See [`crate::Diagnostic`].
    #[serde(skip)]
    pub diagnostics: Vec<Diagnostic>,
}
