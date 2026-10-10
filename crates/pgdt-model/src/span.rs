//! The full file map's vocabulary: the [`Span`]s tiling a dump, what each
//! holds, the `COPY` blocks among them and the [`DumpIndex`] carrying them
//! (`docs/design/decisions.md`, "D30", "D34"). `pgdt_dump::map` builds them and
//! `pgdt_dump::cache` persists them.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    ArrayShape, BlockCensus, BlockStatistics, CollationDef, Diagnostic, DumpMetadata,
    IgnoredRefusals, TableDef, TableReference, TypeKind, Unrepresentable,
    deserialize_block_statistics,
};
use pgdt_core::copy::CopyHeader;

/// One tile of the full file map. `start`/`end` are absolute file offsets;
/// `[start, end)` never overlaps another span's range, and every span
/// together tiles the file, a partial scan's unread rest one `Unscanned`
/// span — see [`pgdt_dump::map::check_tiling`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u64,
    pub end: u64,
    /// The database this span belongs to — same convention as
    /// [`CopyBlock::database`]: the name from the governing `\connect`, or
    /// `None` for a plain dump (or content before any `\connect`).
    pub database: Option<String>,
    /// The span's own bytes, sliced from the file by offset and stored in the
    /// cache so `pgdt info` answers without re-reading the dump
    /// (`docs/design/decisions.md`, "D30"). `None` until [`pgdt_dump::map::attach_text`] has
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
/// `pgdt_dump::map::parse_toc_header_line`; see the module docs' "TOC enrichment" bullet.
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
    /// `None` for `Owner: -` (`--no-owner`, or an owner-less kind like
    /// `COMMENT`) or `Owner: ` (empty — some entry kinds pass an empty string
    /// rather than `NULL` through `sanitize_line`). Both mean "no owner
    /// recorded here" to a reader.
    pub owner: Option<String>,
    /// The `; Tablespace: <name>` suffix, present only when the entry has a
    /// non-default tablespace and `--no-tablespaces` was not given.
    pub tablespace: Option<String>,
}

/// A span's stored bytes. Capped at [`SPAN_STORED_TEXT_MAX_BYTES`]: one pathological function
/// body must not make the cache unbounded, and the offsets are kept
/// regardless, so a caller that needs the rest can always read the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanText {
    /// Lossy UTF-8 of the span's bytes — truncated to the cap, if it hit one.
    pub text: String,
    /// Whether bytes were dropped to fit the cap.
    pub truncated: bool,
}

/// Per-span cap on stored text (`docs/design/decisions.md`, "D30") — a whole
/// schema's DDL being small, it bites only on one enormous statement and the
/// spans after it in its run (`KD31`).
pub const SPAN_STORED_TEXT_MAX_BYTES: usize = 64 * 1024;

/// The payload behind [`SpanBody::Data`] — see that variant's docs for why
/// one `Data` span kind covers all three producers despite them not sharing a
/// shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DataBlock {
    Copy(CopyBlock),
    InsertRun(InsertRun),
    LargeObjects(LargeObjectRegion),
}

/// A run of `pg_dump --inserts`/`--column-inserts` output for one table —
/// `INSERT INTO <table> ...;` statements, one per row, merged into a single
/// `Data` span instead of one `Unparsed` span per statement. No inner
/// offsets, nothing reading rows out of this yet
/// (`docs/design/decisions.md`, "D33").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertRun {
    /// Same convention as [`CopyBlock::database`].
    pub database: Option<String>,
    /// The table every statement in this run targets, as its own first
    /// `INSERT INTO` line named it — schema-qualified when the line was,
    /// matching [`CopyHeader::qualified_name`](pgdt_core::copy::CopyHeader::qualified_name)'s
    /// convention for the same object.
    pub table: String,
    /// Number of `INSERT` statements folded into this span — free to
    /// compute, finding the run's end meaning recognizing each completion.
    pub row_count: u64,
}

/// The large-object data region (I12) — every `BEGIN;`/`COMMIT;`-wrapped
/// `lo_open`/`lowrite`/`lo_close` run in the file, merged into a single `Data`
/// span regardless of how many archive entries `pg_dump` split it across (one
/// on v13-16, one per object on v17+ — see [`pgdt_dump::map::Builder::on_large_object_start`]).
/// No identity at all: the region's only identity is the OID in each
/// `lo_create('<oid>')` opener, and recovering it costs a walk of the whole
/// region (`docs/design/decisions.md`, "D33").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LargeObjectRegion {
    /// Same convention as [`CopyBlock::database`].
    pub database: Option<String>,
}

/// What a span is, at the granularity the statement-driven pass (no
/// TOC enrichment) can tell — see the module docs.
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
    /// [`pgdt_dump::preamble::dump_metadata_from_spans`] needs the database name
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
    /// A `--binary-upgrade` dump's `ALTER TYPE <name> ADD VALUE '<label>'
    /// ...;` (I6) — recognized so
    /// [`pgdt_dump::preamble::dump_metadata_from_spans`] can fold the label into
    /// the [`TypeDef`](SpanBody::TypeDef) span it targets.
    AlterTypeAddValue {
        type_name: String,
        label: String,
    },
    /// A `--binary-upgrade` dump's `ALTER TYPE <name> DROP ATTRIBUTE
    /// <attr>;`, dropping a composite's placeholder for an attribute the type
    /// has dropped — recognized so
    /// [`pgdt_dump::preamble::dump_metadata_from_spans`] can remove the field from
    /// the [`TypeDef`](SpanBody::TypeDef) span it targets.
    AlterTypeDropAttribute {
        type_name: String,
        attribute: String,
    },
    /// An `ALTER TYPE` that could change an enum's labels and that
    /// [`pgdt_dump::preamble`] does not read — `RENAME VALUE`, `ADD VALUE IF NOT
    /// EXISTS`, a label no plain `'…'` literal spells — naming the type it alters, or `None`
    /// where it names none this grammar reads. No `pg_dump` writes one; it is
    /// recognized so [`pgdt_dump::preamble::dump_metadata_from_spans`] can mark
    /// the labels it may have changed inexact.
    EnumLabelsUnread {
        type_name: Option<String>,
    },
    /// An `ALTER TABLE` adding to a table what
    /// [`pgdt_dump::preamble::parse_alter_table_reference`] reads — a
    /// `--binary-upgrade` dump's `INHERIT <parent>`, `OF <type>` or `ADD
    /// CONSTRAINT … CHECK`, a pre-v18 `SET NOT NULL`, an `ATTACH PARTITION`,
    /// whose `table` is the partition it attaches — recognized so
    /// [`pgdt_dump::preamble::dump_metadata_from_spans`] can fold the reference
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
    /// into its owning entry's span (the module docs' "no grouping").
    Unparsed,
    /// Bytes no scan has walked yet — always a single trailing span
    /// (`docs/design/decisions.md`, "D30"). Never produced by `pgdt_query::stream::build_map`,
    /// which always scans to EOF; an incremental scan is what emits one.
    Unscanned,
}

/// What [`pgdt_dump::map::check_tiling`] found wrong, if anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum TilingIssue {
    /// `spans[i]` and `spans[i + 1]` overlap or leave a gap between them.
    Discontinuity { after_index: usize, span_end: u64, next_start: u64 },
    /// The spans aren't sorted by `start` — every other check assumes they
    /// are, so this is reported instead of a misleading discontinuity.
    OutOfOrder { index: usize },
    /// The first span doesn't start at byte 0.
    DoesNotStartAtZero { first_start: u64 },
    /// The last span's `end` doesn't reach `expected_end` (the scanned
    /// prefix's length, or the file size for a full scan).
    DoesNotReachEnd { last_end: u64, expected_end: u64 },
    /// A span's own `start >= end`.
    Empty { index: usize },
}

/// One located COPY block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyBlock {
    pub header: CopyHeader,
    /// The database this block belongs to — the name from the `\connect`
    /// governing it. `None` means the file had no `\connect` at all (a plain
    /// dump), which falls back to the single unnamed [`crate::DatabaseMetadata`].
    /// Not an ordinal into `metadata.databases`: an incremental scan's
    /// metadata can hold one entry however many databases the file contains.
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
    /// `pgdt_query::stream::table_stream` to decide whether a cold query may stop
    /// once the queried table's block closes or must run to EOF, those blocks
    /// not being adjacent (`docs/design/decisions.md`, "D49").
    pub partition_root: Option<String>,
    /// This block's per-row-group column statistics, where the mapping pass
    /// that recorded it was asked to gather them (`pgdt_query::statistics`).
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
    /// **The fields its type's `*_in` refuses that a parse told to ignore them
    /// went past in this block** — per column, its first in full and a count
    /// — and `None` where no pass gathering it met one so. Recorded by the
    /// mapping pass and merged with a back-fill's re-read
    /// ([`IgnoredRefusals::merge`]); a parse refusing such fields over this
    /// map fails with it where it tracks such a column
    /// ([`pgdt_core::Error::FieldRefusedRecorded`]).
    #[serde(default)]
    pub ignored_refusals: Option<Box<IgnoredRefusals>>,
    /// **Whether a strict parse checked every field of this block**
    /// ([`crate::PostgresInvalidValues::Strict`]), mapping or re-reading it,
    /// and found none refused — a fact about the dump's bytes, which the
    /// cache's identity binds it to. A strict parse re-reads every block
    /// holding `false`, so a clean one has checked every field whichever runs
    /// built the map; nothing else reads it.
    #[serde(default)]
    pub checked_in_full: bool,
    /// This block's array-shape census, one [`ArrayShape`] per column in
    /// `header.columns` order, and `None` for a block mapped at the
    /// [`pgdt_query::StatisticsLevel::Metadata`] level, which records nothing drawn
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
    /// a `DumpIndex` built by hand rather than by `pgdt_query::stream::build_index`. A derived
    /// view over `spans` (`pgdt_dump::preamble::dump_metadata_from_spans`),
    /// computed once (`docs/design/decisions.md`, "D34").
    pub metadata: Option<DumpMetadata>,
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
    /// wherever an index is mapped or loaded. A preamble-only run maps
    /// only the preamble, so it returns a loaded cache's figures as they were, or none
    /// but its source's. See [`crate::Diagnostic`].
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
