//! The full file map: an ordered, tiling set of [`Span`]s covering every byte
//! of a `pg_dump` plain-format file (`docs/design/decisions.md`, "D30").
//!
//! [`build_map`] classifies DDL statements directly. A TOC comment block is
//! recognized both as a **boundary** — see below — and as an **enrichment
//! layer** read off the same lines: owner, kind label, the `Tablespace:`
//! field, and a per-file TOC-coverage count.
//!
//! ## Span boundaries: TOC-block-anchored, with a statement-grammar fallback
//!
//! A span opens at the `--` of a TOC comment block (recognized lexically —
//! any run of `--`-prefixed lines containing one that matches `-- Name: ...;
//! Type: ...`) or, absent one, at the first byte of a recognized statement.
//! It runs greedily to the byte before the next span opens. **A span's `end`
//! is where the next span starts**: pushing a span closes the one before it
//! there, and [`Builder::finish`]/[`Builder::snapshot`] close the last at the
//! scan's end.
//!
//! The anchor is the TOC block rather than statement completion because
//! [`crate::scan::CopyScanner`] emits no [`crate::scan::Event::Line`] for a
//! dollar-quoted line — not even the one carrying a `CREATE FUNCTION`'s own
//! terminating `;` — and it filters TOC-shaped lines inside such a body out
//! before any event reaches this module (I3).
//!
//! Two fallbacks cover input carrying **no** TOC headers:
//! [`crate::scan::Event::DollarQuoteEnd`], which
//! [`Builder::on_dollar_quote_end`] treats as completing whatever statement
//! is in flight, and [`crate::preamble::statement_complete`] for any
//! statement that is not its own TOC entry. The lines themselves stay
//! unsurfaced either way.
//!
//! **Consequence: no "grouping".** A definition and its ungrouped trailing
//! statement tile as two adjacent spans rather than one, both carrying the
//! same [`Span::toc`]; grouping via the TOC's `Dependencies:` field is
//! unimplemented and unassigned.
//!
//! ## What this module owns
//!
//! - **Boundaries and classification.** [`Builder`] is the state machine,
//!   driven directly by [`crate::index::build_index`],
//!   [`crate::index::scan_preamble`] and `crate::stream`'s mapping pass, fed
//!   the same [`crate::scan::Event`] stream those already walk. [`build_map`]
//!   is a thin wrapper for callers that just want a span list.
//!
//!   A [`Builder`] that starts partway through a file opens its first span at
//!   its own first recognized content, **not** at the byte it began reading;
//!   closing that seam is the caller's job (`crate::stream::splice`).
//!
//! - **Bulk regions.** [`SpanBody::Data`] holds a [`DataBlock`] — a `COPY`
//!   block, an `INSERT` run, or the whole `BEGIN;`/`COMMIT;`-wrapped
//!   large-object region (I12), merged across however many archive entries
//!   `pg_dump` split it into. `crate::scan` skips a large-object region
//!   **unread**; `INSERT` runs stay a `feed_line`-level concern with no
//!   scanner state, driving a [`StatementScan`] over raw line bytes. Only
//!   [`DataBlock::Copy`] carries inner offsets
//!   (`docs/design/decisions.md`, "D33").
//!
//! - **TOC enrichment.** [`Span::toc`] is filled by [`parse_toc_header_line`]
//!   whenever a comment block's TOC-Name line parses (I3, I16). The verbose
//!   `-- TOC entry N (class C OID O)` / `-- Dependencies: ...` lines that can
//!   precede the Name line are ordinary comment lines to this parser.
//!
//! - **TOC inheritance for follow-on statements.** A TOC entry is not one
//!   statement: `Builder`'s `governing_toc` is the entry a comment-less
//!   follow-on inherits, and [`Span::toc_owned`] records whether a span
//!   carried the header text itself (`docs/design/decisions.md`, "D31").
//!
//! - **The cross-reference set.** [`Builder`] accumulates `roles`/`tablespaces`
//!   (both [`crate::index::DumpIndex`] fields of the same name) as spans
//!   close: [`Builder::push_span`] reads `toc.owner`/`toc.tablespace`, and
//!   [`Builder::push_statement_span`] scans the closing statement's own text
//!   via [`extract_statement_cross_refs`] — the sources `Span::toc` alone
//!   cannot cover.
//!
//! - **Span text and the tiling check.** [`attach_text`] fills [`Span::text`]
//!   by slicing the file at each span's own offsets, capped at [`SPAN_STORED_TEXT_MAX_BYTES`];
//!   `Data` and `Unscanned` spans store none. [`check_tiling`]'s production
//!   callers report a failure as a
//!   [`crate::diagnostic::DiagnosticKind::TilingBroken`] on the index and
//!   return the map anyway (`docs/design/decisions.md`, "D30").
//!
//! [`SpanBody::Unscanned`] covers whatever a partial scan has not reached, so
//! **every** `DumpIndex` tiles its file, one built by a query included.
//! `DumpMetadata` is [`crate::preamble::dump_metadata_from_spans`], a derived
//! view over `spans` — which is what [`SpanBody::Connect`],
//! [`SpanBody::VersionHeader`] and [`SpanBody::AlterTypeAddValue`] exist for
//! rather than generic [`SpanBody::Framing`]/[`SpanBody::Unparsed`].

use std::collections::BTreeSet;
use std::ops::ControlFlow;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::copy::split_fields;
use crate::index::{ArrayShape, CopyBlock};
use crate::instrument::StatisticsScope;
use crate::io::ByteRangeSource;
use crate::preamble::{
    CollationDef, ColumnDef, Extension, StatementScan, StatementShape, TypeDef, TypeKind,
    classify_statement, extract_statement_cross_refs, in_open_quote, insert_role,
    insert_tablespace, parse_alter_type_add_value_body, parse_connect, parse_qualified_name,
    push_stmt_line, statement_complete, strip_kw,
};
use crate::scan::{CopyEnd, CopyStart, Event, ScanOptions, scan};
use crate::statistics::{BlockGathered, BlockObserver};

/// One tile of the full file map. `start`/`end` are absolute file offsets;
/// `[start, end)` never overlaps another span's range, and every span
/// together sums to the scanned portion of the file — see [`check_tiling`].
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
    /// (`docs/design/decisions.md`, "D30"). `None` until [`attach_text`] has
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
    /// `VersionHeader`) with no TOC comment of its own. Not a substitute for
    /// [`SpanBody`]'s own per-kind
    /// fields: the two are separately-sourced observations of one object
    /// (`docs/design/decisions.md`, "D30").
    pub toc: Option<TocHeader>,
    /// Whether *this span's own* preceding comment carried the TOC header
    /// text (`true`), as opposed to `toc` being inherited from an earlier
    /// entry's span (`false`) — `false` when `toc` is `None`, but for a comment
    /// block shaped like a TOC entry whose header did not parse. An object
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
/// [`parse_toc_header_line`]; see the module docs' "TOC enrichment" bullet.
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

/// Parse pg_dump's TOC header line into a [`TocHeader`] — `None` for any
/// comment line that doesn't match, which leaves [`Span::toc`] `None` exactly
/// as if there were no TOC comment at all. `trimmed` is expected to have
/// leading/trailing whitespace already removed.
///
/// All three of `_printTocEntry()`'s prefixes go through one code path:
/// `TOC_PREFIX_DATA` ("Data for ") and `TOC_PREFIX_STATS` ("Statistics for ",
/// a `pg_dump` 18+ `--statistics` component, I18) are each optional, and what
/// follows one must still be `Name: `. Whether a prefix is also a *boundary*
/// signal is [`looks_like_toc_name_line`]'s question
/// (`docs/design/decisions.md`, "D31").
///
/// Splits on the field markers in the order `_printTocEntry()` writes them,
/// not on a general grammar: `sanitize_line` never escapes a literal
/// `; Type: ` inside an object's own name, so a pathological name can
/// mis-split. I3 treats the TOC comment as a hint, and this parser only ever
/// feeds enrichment, never a boundary.
fn parse_toc_header_line(trimmed: &str) -> Option<TocHeader> {
    let rest = trimmed.strip_prefix("-- ")?;
    let rest = rest
        .strip_prefix("Data for ")
        .or_else(|| rest.strip_prefix("Statistics for "))
        .unwrap_or(rest);
    let rest = rest.strip_prefix("Name: ")?;
    let (name, rest) = rest.split_once("; Type: ")?;
    let (kind, rest) = rest.split_once("; Schema: ")?;
    let (schema, rest) = rest.split_once("; Owner: ")?;
    let (owner, tablespace) = match rest.split_once("; Tablespace: ") {
        Some((owner, tablespace)) => (owner, Some(tablespace.to_string())),
        None => (rest, None),
    };
    let none_if_placeholder = |s: &str| (!s.is_empty() && s != "-").then(|| s.to_string());
    Some(TocHeader {
        name: name.to_string(),
        kind: kind.to_string(),
        schema: none_if_placeholder(schema),
        owner: none_if_placeholder(owner),
        tablespace,
    })
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
/// schema's DDL being small, it only bites on one enormous statement.
pub const SPAN_STORED_TEXT_MAX_BYTES: usize = 64 * 1024;

/// Whether a span of this kind stores its text at all. `Data` spans never do
/// (`docs/design/decisions.md`, "D30"); `Unscanned` covers bytes by
/// definition unread, so there is nothing to slice.
fn stores_text(body: &SpanBody) -> bool {
    !matches!(body, SpanBody::Data(_) | SpanBody::Unscanned)
}

/// Fill in [`Span::text`] for every span that stores it, reading the bytes
/// back from `source` by offset — sliced from the file, never accumulated
/// from [`crate::scan::Event::Line`], which emits nothing for a dollar-quoted
/// string and would miss every function body in the file.
///
/// Runs as a pass over finished spans rather than as each span closes,
/// because only the caller owns the source. Contiguous runs of text-storing
/// spans are coalesced into one `read_range`, so this is one read per gap
/// between data blocks rather than one per span.
///
/// Deficiency register: `deficiency: KD31` — that read is capped at
/// `SPAN_STORED_TEXT_MAX_BYTES` times the run's span count from the run's start, not per span,
/// so a span following one longer than the cap can fall past what was read and
/// be stored empty and `truncated` however short it is. **(c) unowned**;
/// promoted by a `--map` listing seen to lose a statement's text, the fix
/// being a read per span past the cap.
pub async fn attach_text(source: &dyn ByteRangeSource, spans: &mut [Span]) -> Result<()> {
    let mut i = 0;
    while i < spans.len() {
        if !stores_text(&spans[i].body) {
            spans[i].text = None;
            i += 1;
            continue;
        }
        // Extend over the whole contiguous run of text-storing spans, so one
        // read serves all of them.
        let mut j = i;
        while j + 1 < spans.len()
            && stores_text(&spans[j + 1].body)
            && spans[j + 1].start == spans[j].end
        {
            j += 1;
        }
        let (run_start, run_end) = (spans[i].start, spans[j].end);
        // Capped at what the run's spans can store between them, so a
        // multi-gigabyte `Unparsed` region is never pulled into memory whole
        // (`KD31`).
        let want =
            (run_end - run_start).min(((j - i + 1) * SPAN_STORED_TEXT_MAX_BYTES) as u64) as usize;
        let bytes = source.read_range(run_start, want).await?;
        for span in &mut spans[i..=j] {
            let from = (span.start - run_start) as usize;
            let to = ((span.end - run_start) as usize).min(bytes.len());
            let slice = if from < to { &bytes[from..to] } else { &[][..] };
            let truncated = slice.len() < (span.end - span.start) as usize
                || (span.end - span.start) as usize > SPAN_STORED_TEXT_MAX_BYTES;
            let slice = &slice[..slice.len().min(SPAN_STORED_TEXT_MAX_BYTES)];
            span.text =
                Some(SpanText { text: String::from_utf8_lossy(slice).into_owned(), truncated });
        }
        i = j + 1;
    }
    Ok(())
}

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
    /// matching [`CopyHeader::qualified_name`](crate::copy::CopyHeader::qualified_name)'s
    /// convention for the same object.
    pub table: String,
    /// Number of `INSERT` statements folded into this span — free to
    /// compute, finding the run's end meaning recognizing each completion.
    pub row_count: u64,
}

/// The large-object data region (I12) — every `BEGIN;`/`COMMIT;`-wrapped
/// `lo_open`/`lowrite`/`lo_close` run in the file, merged into a single `Data`
/// span regardless of how many archive entries `pg_dump` split it across (one
/// on v13-16, one per object on v17+ — see [`Builder::on_large_object_start`]).
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
        columns: Vec<ColumnDef>,
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
    /// [`crate::preamble::dump_metadata_from_spans`] needs the database name
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
    /// [`crate::preamble::dump_metadata_from_spans`] can fold the label into
    /// the [`TypeDef`](SpanBody::TypeDef) span it targets.
    AlterTypeAddValue {
        type_name: String,
        label: String,
    },
    /// File prologue/epilogue framing (the `PostgreSQL database dump`
    /// banner, `\restrict`/`\unrestrict`, the `SET`/`set_config` preamble
    /// block, ...) — never a real database object, and none of the more
    /// specific framing-adjacent kinds above.
    Framing,
    /// A recognized statement (trailing `;`, parens/quotes balanced) that is
    /// none of this module's classified shapes — including every object kind
    /// TOC enrichment would otherwise label, and any statement not grouped
    /// into its owning entry's span (the module docs' "no grouping").
    Unparsed,
    /// Bytes no scan has walked yet — always a single trailing span
    /// (`docs/design/decisions.md`, "D30"). Never produced by [`build_map`],
    /// which always scans to EOF; an incremental scan is what emits one.
    Unscanned,
}

/// What [`check_tiling`] found wrong, if anything.
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

/// Verify that `spans` tiles `[0, expected_end)` exactly: sorted, contiguous,
/// no gaps or overlaps, starting at 0 and ending at `expected_end`. Returns
/// every issue found, not just the first; a tiling failure is a bug in this
/// module, never a reason to refuse the file
/// (`docs/design/decisions.md`, "D30").
pub fn check_tiling(spans: &[Span], expected_end: u64) -> Vec<TilingIssue> {
    let mut issues = Vec::new();
    if spans.is_empty() {
        if expected_end != 0 {
            issues.push(TilingIssue::DoesNotReachEnd { last_end: 0, expected_end });
        }
        return issues;
    }

    for (i, s) in spans.iter().enumerate() {
        if s.start >= s.end {
            issues.push(TilingIssue::Empty { index: i });
        }
    }
    for i in 0..spans.len() - 1 {
        let (a, b) = (&spans[i], &spans[i + 1]);
        if b.start < a.start {
            issues.push(TilingIssue::OutOfOrder { index: i + 1 });
        } else if a.end != b.start {
            issues.push(TilingIssue::Discontinuity {
                after_index: i,
                span_end: a.end,
                next_start: b.start,
            });
        }
    }
    if spans[0].start != 0 {
        issues.push(TilingIssue::DoesNotStartAtZero { first_start: spans[0].start });
    }
    let last_end = spans.last().unwrap().end;
    if last_end != expected_end {
        issues.push(TilingIssue::DoesNotReachEnd { last_end, expected_end });
    }
    issues
}

/// Accumulator state between one span boundary and the next.
enum Mode {
    /// Nothing pending; the next non-blank line (or `CopyStart`) opens a new
    /// span.
    Idle,
    /// Absorbing a run of `--`-prefixed lines. `saw_name` is set the moment
    /// one matches `-- Name: ...; Type: ...`; `server_version`/`pg_dump_version`
    /// accumulate the two-line version-header block's fields (I9), so the
    /// block can close as a [`SpanBody::VersionHeader`] rather than generic
    /// [`SpanBody::Framing`]. `toc` is set the moment a line parses via
    /// [`parse_toc_header_line`], independent of `saw_name`: a
    /// `-- Data for Name: ...` line parses without matching
    /// `looks_like_toc_name_line`'s stricter boundary check.
    Comment {
        start: u64,
        saw_name: bool,
        server_version: Option<String>,
        pg_dump_version: Option<String>,
        toc: Option<TocHeader>,
    },
    /// Absorbing a statement's lines via [`statement_complete`]. Started
    /// either directly (no TOC comment — `toc`/`toc_owned` seeded from
    /// [`Builder::governing_toc`]) or right after a TOC comment block closes
    /// with `saw_name` true (`toc` that comment's own header, `toc_owned`
    /// true). Either way `start` is the *span's* start, which for the TOC
    /// case is the comment block's, not this statement's first line.
    Statement { start: u64, buf: String, toc: Option<TocHeader>, toc_owned: bool },
    /// Accumulating a run of `INSERT INTO <table> ...;` statements for one
    /// table, so the whole run becomes one `Data` span
    /// (`docs/design/decisions.md`, "D33"). `start`/`toc`/`toc_owned` are the
    /// span's own, same convention as `Statement`. `table` is fixed at the
    /// run's first line; a later statement targeting a different table ends
    /// the run (header-less input may do this; real `pg_dump` output never
    /// does). `scan` tracks the *current*, not-yet-complete statement only,
    /// empty between statements — the signal a fresh line either continues
    /// the run or ends it. `row_count` is complete statements folded in.
    ///
    /// This holds no statement *text*, nothing reading a run's bytes, so the
    /// accumulation is a [`StatementScan`] over raw bytes. `prefix` is the
    /// `INSERT INTO <table>` byte prefix the run's first line spelled, kept so
    /// [`Builder::insert_run_line`] recognizes a continuing statement without
    /// re-parsing the identifier.
    InsertRun {
        start: u64,
        table: String,
        database: Option<String>,
        scan: StatementScan,
        prefix: Box<[u8]>,
        row_count: u64,
        toc: Option<TocHeader>,
        toc_owned: bool,
    },
}

/// The statement-driven boundary/classification pass, shared by
/// [`build_map`] (a standalone, always-to-EOF scan) and
/// [`crate::index::build_index`]/[`crate::index::scan_preamble`], which drive
/// it directly off the events [`crate::scan::scan`] already walks
/// (`docs/design/decisions.md`, "D34").
pub(crate) struct Builder {
    mode: Mode,
    database: Option<String>,
    spans: Vec<Span>,
    /// The open `COPY` block, tracked separately from `mode`, which is always
    /// `Idle` while a `CopyStart`/`CopyEnd` pair is in flight. `.0` is the
    /// span's own start offset, which for a TOC-commented block precedes
    /// `.1`'s `header_offset`. `.2` is the partition-root marker this block's
    /// header carried, consumed at `CopyStart` so a later block cannot inherit
    /// it. `.3` is the TOC header the preceding `-- Data for Name: ...`
    /// comment (if any) parsed to.
    pending_data: Option<(u64, CopyStart, Option<String>, Option<TocHeader>)>,
    /// The `-- load via partition root <name>` marker (I2) seen since the
    /// last TOC entry began, waiting for the `COPY` header it belongs to.
    /// Cleared by the header that consumes it and by the next TOC `Name:`
    /// line, so an entry that was not table data leaves nothing behind.
    pending_partition_root: Option<String>,
    /// The in-progress merged large-object `Data` span, if a `BEGIN;` has
    /// been seen with no flush since — see [`on_large_object_start`](Self::on_large_object_start).
    /// `.0` is the span's own start (the first region's preceding TOC
    /// comment, if it had one, else its own `BEGIN;` line); `.1` is the
    /// offset just past the most recently closed `COMMIT;`, recorded and not
    /// read — the span's `end` is the next span's start, as every span's is;
    /// `.2` is the first
    /// region's own TOC header, kept as the merged span's single
    /// representative `toc` (`Span::toc` holds one header; a v17+ run can
    /// carry several — see [`on_large_object_start`](Self::on_large_object_start)).
    pending_large_objects: Option<(u64, u64, Option<TocHeader>)>,
    /// Roles/tablespaces referenced anywhere fed to this builder so far —
    /// accumulated as spans close. See [`Builder::push_span`] (TOC
    /// `Owner:`/`Tablespace:`) and [`Builder::push_statement_span`]
    /// (`OWNER TO`/`GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR
    /// ROLE`/`SET default_tablespace`).
    roles: BTreeSet<String>,
    tablespaces: BTreeSet<String>,
    /// The TOC entry a follow-on statement with no comment of its own would
    /// inherit (`docs/design/decisions.md`, "D31"). Updated by every
    /// [`push_span`](Self::push_span) call: that span's own `toc` for a plain
    /// statement or `Data` span, whether freshly parsed or itself inherited,
    /// and `None` for `Framing`/`Connect`/`VersionHeader`. Reset fresh by
    /// every new `Builder`.
    governing_toc: Option<TocHeader>,
    /// The census accumulating for the open `COPY` block (`ArrayShape`).
    /// Sized at `CopyStart` from the header's column list, and grown by any
    /// row that turns out to have more fields (a header-less block, whose
    /// column count only the rows know). Every mapping pass censuses, a block
    /// reaching the map only once walked end to end
    /// (`docs/design/decisions.md`, "D35").
    pending_census: Vec<ArrayShape>,
    /// The statistics observer for the open `COPY` block, when the mapping
    /// pass asked for one ([`Builder::observe_block`]). Handed every row and
    /// finished into [`CopyBlock::statistics`] at its `CopyEnd`.
    pending_observer: Option<Box<dyn BlockObserver>>,
}

/// Fold one data row of a `COPY` block into `census`, one [`ArrayShape`] per
/// column, growing it for a row that turns out to have more fields than the
/// header named (a header-less block, whose column count only the rows know).
///
/// The row is rejected wholesale before it is split: an array literal always
/// contains a `{`, the only other thing that can start one is an `[lb:ub]=`
/// prefix, so a row holding neither byte costs one `memchr2` pass.
///
/// **Two measurements are regenerated by patching this function**, and for
/// every caller, which is why the patch point is here rather than in
/// [`Builder::on_row`]: `census-brace-free` and `census-arrays` take their
/// census-off column from this body preceded by a bare `return;`. Renaming
/// it, splitting it, or moving the pre-filter out breaks that recipe.
///
/// A free function because its two callers hold their census in different
/// places: the serial mapping pass on the [`Builder`], an interior worker
/// (`crate::leader`) its own.
pub(crate) fn census_row(census: &mut Vec<ArrayShape>, raw: &[u8]) {
    if memchr::memchr2(b'{', b'[', raw).is_none() {
        return;
    }
    for (i, field) in split_fields(raw).enumerate() {
        if i >= census.len() {
            census.resize(i + 1, Default::default());
        }
        census[i].observe(field);
    }
}

/// Whether `-- Name: ...` (pg_dump's `_printTocEntry()` header, I3) is
/// present in `line` — the lexical, field-blind boundary signal the module
/// docs describe. Deliberately not full TOC parsing: it never reads
/// `Type:`/`Schema:`/`Owner:`, only confirms this comment block is a real
/// TOC entry rather than framing prose.
///
/// `TOC_PREFIX_STATS` ("Statistics for ") counts, what follows a statistics
/// entry being an ordinary statement the span must run into;
/// `TOC_PREFIX_DATA` ("Data for ") does not
/// (`docs/design/decisions.md`, "D31"). The refusal decides nothing on a
/// default dump, and earns its keep on a `--disable-triggers` one (I31),
/// where a statement *does* intervene between the entry and its data:
/// absorbing there would run the entry into that statement's span, where
/// [`Builder::push_statement_span`]'s `Framing` veto discards the header
/// outright. Pinned by
/// `a_data_entry_keeps_its_own_span_when_disable_triggers_intervenes`.
fn looks_like_toc_name_line(line: &str) -> bool {
    let named = line.starts_with("-- Name: ") || line.starts_with("-- Statistics for Name: ");
    named && line.contains("; Type: ")
}

/// The root table named by a `-- load via partition root <name>` marker
/// line (I2), if `line` is one. `pg_dump` writes it whenever the entry's
/// `COPY` header names the partition's **root** rather than the partition —
/// the only shape in which one header name owns several blocks.
fn partition_root_marker(line: &str) -> Option<String> {
    let rest = line.strip_prefix("-- load via partition root ")?;
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// The table an `INSERT INTO <table> ...` line targets, if `line` is one —
/// `pg_dump`'s `dumpTableData_insert()` always emits exactly this fixed
/// casing, with or without a column list, only the identifier right after
/// `INSERT INTO ` being read. `None` for anything else, including a line that
/// merely starts with this text as multi-line *string content*: the caller
/// only ever checks this on a fresh statement's first line.
fn parse_insert_target(line: &str) -> Option<String> {
    parse_insert_target_span(line).map(|(name, _prefix_len)| name)
}

const INSERT_INTO: &str = "INSERT INTO ";

/// [`parse_insert_target`], plus the byte length of the `INSERT INTO
/// <table>` opening it read — the exact prefix a *continuing* statement of
/// the same run restates.
///
/// [`Builder::insert_run_line`] keeps that prefix and compares raw bytes
/// against it, so a run's ordinary lines are classified without parsing an
/// identifier per row. The comparison is conservative rather than equivalent:
/// a line matching the prefix provably parses to the same name, and one that
/// does not is handed to [`Builder::step`], which parses it properly.
fn parse_insert_target_span(line: &str) -> Option<(String, usize)> {
    let rest = line.strip_prefix(INSERT_INTO)?;
    parse_qualified_name(rest).map(|(name, consumed)| (name, INSERT_INTO.len() + consumed))
}

/// Whether `line` is one of the version-header block's two lines (I9), and
/// if so, which field it fills (`true` for server version, `false` for
/// `pg_dump` version).
fn version_header_field(line: &str) -> Option<(bool, String)> {
    if let Some(rest) = line.strip_prefix("-- Dumped from database version ") {
        return Some((true, rest.trim().to_string()));
    }
    line.strip_prefix("-- Dumped by pg_dump version ").map(|rest| (false, rest.trim().to_string()))
}

/// What a closing comment block becomes: a TOC entry stays [`SpanBody::Unparsed`]
/// (its span continues into the statement it precedes — see [`Builder::step`]'s
/// `Mode::Comment` arm), the version-header block becomes [`SpanBody::VersionHeader`],
/// and everything else is generic [`SpanBody::Framing`].
fn close_comment(
    saw_name: bool,
    server_version: Option<String>,
    pg_dump_version: Option<String>,
) -> SpanBody {
    if saw_name {
        SpanBody::Unparsed
    } else if server_version.is_some() || pg_dump_version.is_some() {
        SpanBody::VersionHeader { server_version, pg_dump_version }
    } else {
        SpanBody::Framing
    }
}

impl Builder {
    /// A builder with no database in scope until a `\connect` establishes
    /// one — right for a scan that starts at byte 0.
    pub(crate) fn new() -> Self {
        Self::with_database(None)
    }

    /// A builder with `database` already in scope — what
    /// [`crate::stream::table_stream`]'s mapping pass needs, starting at the
    /// map's frontier rather than at byte 0 and possibly well inside an
    /// already-`\connect`ed database's territory whose `\connect` line it
    /// never reads.
    pub(crate) fn with_database(database: Option<String>) -> Self {
        Self {
            mode: Mode::Idle,
            database,
            spans: Vec::new(),
            pending_data: None,
            pending_partition_root: None,
            pending_large_objects: None,
            roles: BTreeSet::new(),
            tablespaces: BTreeSet::new(),
            governing_toc: None,
            pending_census: Vec::new(),
            pending_observer: None,
        }
    }

    /// The database in scope right now — whatever the last `\\connect` this
    /// builder has seen named, or whatever [`with_database`](Self::with_database)
    /// seeded it with, and the value [`on_copy_end`](Self::on_copy_end) stamps
    /// onto a block. Exposed for `crate::stream`'s mapping pass, which has to
    /// notice a `COPY` header belonging to a database whose DDL it has not
    /// stated.
    pub(crate) fn database(&self) -> Option<&str> {
        self.database.as_deref()
    }

    /// A TOC comment's own `Owner:`/`Tablespace:` fields, added to the
    /// cross-reference sets — one of every span's two sources regardless of
    /// its kind, `_printTocEntry()` writing those fields ahead of *every*
    /// entry (`docs/design/decisions.md`, "D31"). The other is the statement
    /// text, which `extract_statement_cross_refs` reads.
    fn harvest_toc_cross_refs(&mut self, toc: &Option<TocHeader>) {
        let Some(t) = toc else { return };
        if let Some(owner) = &t.owner {
            insert_role(&mut self.roles, owner.clone());
        }
        if let Some(tablespace) = &t.tablespace {
            insert_tablespace(&mut self.tablespaces, tablespace.clone());
        }
    }

    /// Push a newly-completed span, and — since the tiling invariant makes a
    /// span's true end exactly the next span's start — fix up the
    /// previously-pushed span's placeholder `end` at the same time. Only the
    /// span still open when this call returns (`self.spans.last()`) carries a
    /// not-yet-real `end`; [`finish`](Self::finish)/[`snapshot`](Self::snapshot)
    /// close that one out. The partway-through case is the module docs'.
    ///
    /// `toc_owned` is `true` iff *this span's own* preceding comment carried
    /// the header text `toc` came from — `false` for a follow-on statement
    /// inheriting [`governing_toc`](Self::governing_toc), which every call
    /// site but the inherited paths of
    /// [`push_statement_span`](Self::push_statement_span) and
    /// [`push_insert_run`](Self::push_insert_run) passes as `toc.is_some()`. This is also where
    /// [`governing_toc`](Self::governing_toc) updates, every span this module
    /// produces passing through here.
    fn push_span(&mut self, start: u64, body: SpanBody, toc: Option<TocHeader>, toc_owned: bool) {
        // Any new span means the pending large-object region isn't being
        // extended, so it closes now — the flush itself excepted, having
        // already taken `pending_large_objects` out of `self`.
        self.flush_large_objects();
        self.harvest_toc_cross_refs(&toc);
        // `Framing`/`Connect`/`VersionHeader` are the three kinds inheritance
        // never crosses (`docs/design/decisions.md`, "D31"); everything else
        // becomes the entry a following comment-less statement inherits.
        self.governing_toc = match &body {
            SpanBody::Framing | SpanBody::Connect { .. } | SpanBody::VersionHeader { .. } => None,
            _ => toc.clone(),
        };
        if let Some(last) = self.spans.last_mut() {
            last.end = start;
        }
        self.spans.push(Span {
            start,
            end: start,
            database: self.database.clone(),
            text: None,
            toc,
            toc_owned,
            body,
        });
    }

    /// Classify a complete statement into a span — but first scan `buf`
    /// itself for the cross-references [`Span::toc`] can't cover: `OWNER TO`,
    /// `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, and `SET
    /// default_tablespace` (`extract_statement_cross_refs`), so every
    /// statement this module classifies is scanned exactly once.
    ///
    /// A statement that classifies as `Framing` never inherits, even when
    /// `toc`/`toc_owned` arrived carrying an inherited value: `classify`
    /// decides the span's final kind, so the veto has to follow that decision
    /// rather than sit at the `Mode::Idle`→`Statement` transition that seeds
    /// it (`docs/design/decisions.md`, "D31").
    fn push_statement_span(
        &mut self,
        start: u64,
        buf: &str,
        toc: Option<TocHeader>,
        toc_owned: bool,
    ) {
        extract_statement_cross_refs(buf, &mut self.roles, &mut self.tablespaces);
        let body = classify(buf);
        let (toc, toc_owned) =
            if matches!(body, SpanBody::Framing) { (None, false) } else { (toc, toc_owned) };
        self.push_span(start, body, toc, toc_owned);
    }

    /// The roles/tablespaces referenced so far — see the [`roles`](Self::roles)
    /// field's docs. Read before [`finish`](Self::finish) consumes the
    /// builder.
    pub(crate) fn roles(&self) -> &BTreeSet<String> {
        &self.roles
    }

    pub(crate) fn tablespaces(&self) -> &BTreeSet<String> {
        &self.tablespaces
    }

    /// Close whatever's pending at end of scan. `on_copy_start` handles the
    /// analogous mid-scan case itself, needing the interrupted span's start
    /// offset to seed the `Data` span that follows.
    ///
    /// `end` is the stop point [`finish`](Self::finish) is closing out at,
    /// needed here because [`crate::index::scan_preamble`] can retreat it to
    /// a pending comment's own `start` rather than guess that comment's kind
    /// (`docs/design/decisions.md`, "D32"). A comment with `start >= end` is
    /// exactly that case, so it is dropped rather than pushed as a
    /// zero-length, wrongly-guessed span.
    fn flush_pending(&mut self, end: u64) {
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => {}
            Mode::Comment { start, saw_name, server_version, pg_dump_version, toc } => {
                if start < end {
                    // Only reachable for a comment block with no closing
                    // non-`--` line — never observed in a well-formed
                    // `pg_dump` file, but classified the same way `step`'s own
                    // comment-close arm would have.
                    let owned = toc.is_some();
                    self.push_span(
                        start,
                        close_comment(saw_name, server_version, pg_dump_version),
                        toc,
                        owned,
                    );
                }
            }
            Mode::Statement { start, buf, toc, toc_owned } => {
                self.push_statement_span(start, &buf, toc, toc_owned)
            }
            Mode::InsertRun { start, table, database, row_count, toc, toc_owned, .. } => {
                self.push_insert_run(start, table, database, row_count, toc, toc_owned)
            }
        }
    }

    /// Feed one outside-block line. Loops internally to let a
    /// comment-block-close or statement-complete transition re-dispatch the
    /// *same* line under the new mode without the caller needing to know.
    pub(crate) fn feed_line(&mut self, offset: u64, raw: &[u8]) {
        if self.insert_run_line(raw) {
            return;
        }
        let text = String::from_utf8_lossy(raw);
        // Tracked here rather than inside `step`'s mode machine: `Mode` has
        // no field to carry the marker in. The block itself does survive to
        // the header — a blank line is absorbed without closing a pending
        // comment (I3), which is what lets `on_copy_start` still read its
        // `toc` — but the marker line is only one `--` line inside it.
        let trimmed = text.trim();
        if looks_like_toc_name_line(trimmed) {
            self.pending_partition_root = None;
        } else if let Some(root) = partition_root_marker(trimmed) {
            self.pending_partition_root = Some(root);
        }
        while self.step(offset, &text) {}
    }

    /// The `INSERT`-run fast path: one line of a run in flight, decided on
    /// raw bytes with neither a UTF-8 conversion nor a statement buffer
    /// behind it. `true` when this line is fully accounted for; `false`
    /// leaves it to [`feed_line`](Self::feed_line)'s ordinary path.
    ///
    /// `KD9`'s discharge: `feed_line`'s per-line validate-and-allocate plus
    /// `statement_complete` re-walking an accumulated `String` is most of an
    /// `INSERT` scan's cost (`docs/design/decisions.md`, "D33").
    ///
    /// Anything the ordinary path might classify differently is handed back:
    /// a line whose first non-ASCII-whitespace byte is not ASCII (`feed_line`'s
    /// Unicode-trimmed prologue may still have something to say about it), a
    /// line starting `-` (every boundary signal begins `--`), a blank line,
    /// and, at a statement boundary, any line that does not restate this run's
    /// own `INSERT INTO <table>` prefix byte for byte.
    ///
    /// Raw bytes where `step` would feed a lossy conversion is no difference:
    /// every byte [`StatementScan`] acts on is ASCII.
    fn insert_run_line(&mut self, raw: &[u8]) -> bool {
        let Mode::InsertRun { scan, prefix, row_count, .. } = &mut self.mode else {
            return false;
        };
        if !matches!(raw.trim_ascii_start().first(), Some(b) if b.is_ascii() && *b != b'-') {
            return false;
        }
        if scan.is_empty()
            && !(raw.starts_with(prefix)
                // The byte after the prefix must end the identifier, or
                // `INSERT INTO t` would match a line targeting `t2`.
                && raw.get(prefix.len()).is_some_and(|b| b.is_ascii_whitespace() || *b == b'('))
        {
            return false;
        }
        scan.feed_line(raw);
        if scan.complete() {
            *row_count += 1;
            scan.reset();
        }
        true
    }

    /// Process one line under the current mode. Returns `true` when the
    /// same line must be reprocessed under a mode this call just switched
    /// into — always the same `offset` and the same `line`, which is why the
    /// caller re-supplies both rather than this returning them.
    fn step(&mut self, offset: u64, line: &str) -> bool {
        let trimmed = line.trim();
        match &mut self.mode {
            Mode::Idle => {
                if trimmed.is_empty() {
                    return false;
                }
                if let Some(name) = parse_connect(line) {
                    self.database = Some(name.clone());
                    self.push_span(offset, SpanBody::Connect { database: name }, None, false);
                    return false;
                }
                if trimmed.starts_with('\\') {
                    // Any other psql meta-command (`\restrict`,
                    // `\unrestrict`, ...): a single complete line, never
                    // continued, never real SQL.
                    self.push_span(offset, SpanBody::Framing, None, false);
                    return false;
                }
                if trimmed.starts_with("--") {
                    let (server_version, pg_dump_version) = match version_header_field(trimmed) {
                        Some((true, v)) => (Some(v), None),
                        Some((false, v)) => (None, Some(v)),
                        None => (None, None),
                    };
                    self.mode = Mode::Comment {
                        start: offset,
                        saw_name: looks_like_toc_name_line(trimmed),
                        server_version,
                        pg_dump_version,
                        toc: parse_toc_header_line(trimmed),
                    };
                    return false;
                }
                // No comment precedes this statement: it inherits whatever
                // entry is currently governing, `None` if none is, and
                // `push_statement_span` vetoes even that if the statement
                // classifies as `Framing` (`docs/design/decisions.md`, "D31").
                self.mode = Mode::Statement {
                    start: offset,
                    buf: String::new(),
                    toc: self.governing_toc.clone(),
                    toc_owned: false,
                };
                true
            }
            Mode::Comment { start, saw_name, server_version, pg_dump_version, toc } => {
                if trimmed.starts_with("--") {
                    *saw_name |= looks_like_toc_name_line(trimmed);
                    match version_header_field(trimmed) {
                        Some((true, v)) => *server_version = Some(v),
                        Some((false, v)) => *pg_dump_version = Some(v),
                        None => {}
                    }
                    if let Some(header) = parse_toc_header_line(trimmed) {
                        *toc = Some(header);
                    }
                    return false;
                }
                if trimmed.is_empty() {
                    // A blank line does not close a pending comment
                    // (`_printTocEntry()` writes `--\n\n` whether a DDL
                    // statement or a `COPY` header follows, I3), so it is
                    // absorbed without deciding either way
                    // (`docs/design/decisions.md`, "D32"). Staying in
                    // `Mode::Comment` is what lets `on_copy_start`'s own arm
                    // still see this block and its `toc`; only a non-blank,
                    // non-`--` line reaches the close logic below.
                    return false;
                }
                let start = *start;
                let saw_name = *saw_name;
                let toc = toc.take();
                if saw_name {
                    // The comment block was a real TOC entry: the span
                    // continues into the statement it precedes, starting at
                    // the comment's own offset. `toc_owned: true` — this
                    // comment is where `toc` came from.
                    self.mode = Mode::Statement { start, buf: String::new(), toc, toc_owned: true };
                } else if let Some((table, prefix_len)) = parse_insert_target_span(line) {
                    // An `INSERT` run follows, so this comment block is a
                    // `-- Data for Name: ...` entry — the one prefix
                    // [`looks_like_toc_name_line`] refuses, which routes it
                    // here rather than into `Mode::Statement`. The comment is
                    // absorbed into the run's outer boundary and its header
                    // carried along, as `on_copy_start` does for a `COPY`
                    // block; otherwise the run would start at its first
                    // `INSERT` line with `toc: None`. Unconditional: a comment
                    // block carrying no header is absorbed the same way.
                    let owned = toc.is_some();
                    self.mode = Mode::InsertRun {
                        start,
                        table,
                        database: self.database.clone(),
                        scan: StatementScan::new(),
                        prefix: line.as_bytes()[..prefix_len].into(),
                        row_count: 0,
                        toc,
                        toc_owned: owned,
                    };
                } else {
                    let body = close_comment(false, server_version.take(), pg_dump_version.take());
                    let owned = toc.is_some();
                    self.push_span(start, body, toc, owned);
                    self.mode = Mode::Idle;
                }
                true
            }
            Mode::Statement { start, buf, toc, toc_owned } => {
                // A `--` line outside a quote closes a statement even
                // though `buf` never reached `statement_complete`
                // (`docs/design/decisions.md`, "D32") — the case a
                // dollar-quoted body's invisible closing line creates, where
                // nothing ever supplies the swallowed `;`. The `in_open_quote`
                // guard is what keeps a `--`-looking continuation line that
                // is really multi-line string content from counting.
                if trimmed.starts_with("--") && !in_open_quote(buf) {
                    let start = *start;
                    let buf = std::mem::take(buf);
                    let toc = toc.take();
                    let toc_owned = *toc_owned;
                    self.mode = Mode::Idle;
                    self.push_statement_span(start, &buf, toc, toc_owned);
                    return true;
                }
                // The run's first line, recognized before it ever becomes a
                // one-statement `Unparsed` span: the whole run is one `Data`
                // span rather than one span per row
                // (`docs/design/decisions.md`, "D33").
                if buf.is_empty()
                    && let Some((table, prefix_len)) = parse_insert_target_span(line)
                {
                    let start = *start;
                    let toc = toc.take();
                    let toc_owned = *toc_owned;
                    self.mode = Mode::InsertRun {
                        start,
                        table,
                        database: self.database.clone(),
                        scan: StatementScan::new(),
                        prefix: line.as_bytes()[..prefix_len].into(),
                        row_count: 0,
                        toc,
                        toc_owned,
                    };
                    return true;
                }
                push_stmt_line(buf, line);
                if statement_complete(buf) {
                    let start = *start;
                    let buf = std::mem::take(buf);
                    let toc = toc.take();
                    let toc_owned = *toc_owned;
                    self.mode = Mode::Idle;
                    self.push_statement_span(start, &buf, toc, toc_owned);
                }
                false
            }
            // Only the lines [`insert_run_line`](Self::insert_run_line)
            // declines reach this arm — a boundary signal, a blank line, or a
            // fresh statement that may not continue the run. It feeds the
            // *same* `scan`, so a run's classification does not depend on
            // which of the two saw a given line.
            Mode::InsertRun { start, table, database, scan, row_count, toc, toc_owned, .. } => {
                // Closes the run in place, taking owned copies first so
                // `self.mode = Mode::Idle` and `push_insert_run`'s
                // `push_span` do not overlap this arm's borrow of
                // `self.mode`.
                macro_rules! close_and_reprocess {
                    () => {{
                        let (start, table, database, row_count, toc, toc_owned) = (
                            *start,
                            table.clone(),
                            database.clone(),
                            *row_count,
                            toc.take(),
                            *toc_owned,
                        );
                        self.mode = Mode::Idle;
                        self.push_insert_run(start, table, database, row_count, toc, toc_owned);
                        return true;
                    }};
                }
                if scan.is_empty() {
                    if trimmed.is_empty() {
                        // Absorbed the way `Mode::Comment` absorbs a blank
                        // line, waiting to see whether the run continues or
                        // the next TOC comment (or EOF) closes it.
                        return false;
                    }
                    let continues = parse_insert_target(line).as_deref() == Some(table.as_str());
                    if !continues {
                        close_and_reprocess!();
                    }
                    // Falls through to accumulate this line as the run's next
                    // statement.
                }
                // Defensive dangling-close, mirroring `Mode::Statement`'s;
                // not expected in real `pg_dump` output.
                if trimmed.starts_with("--") && !scan.in_quote() {
                    close_and_reprocess!();
                }
                scan.feed_line(line.as_bytes());
                if scan.complete() {
                    *row_count += 1;
                    scan.reset();
                }
                false
            }
        }
    }

    /// A dollar-quoted region closed at `offset`
    /// ([`crate::scan::Event::DollarQuoteEnd`]). Whatever statement is in
    /// flight ends with it: `pg_dump` writes the statement's own terminating
    /// `;` on the closing line (`AS $$ … $$;`), and that line never reaches
    /// [`feed_line`](Self::feed_line), so nothing else will ever complete the
    /// statement (`docs/design/decisions.md`, "D32").
    ///
    /// This is the header-less fallback; real `pg_dump` output is unaffected.
    /// A producer that puts the `;` on a *later* line leaves that line as its
    /// own small span — coarser, still tiling.
    pub(crate) fn on_dollar_quote_end(&mut self, _offset: u64) {
        if let Mode::Statement { start, buf, toc, toc_owned } =
            std::mem::replace(&mut self.mode, Mode::Idle)
        {
            self.push_statement_span(start, &buf, toc, toc_owned);
        }
    }

    pub(crate) fn on_copy_start(&mut self, event: CopyStart) {
        // I12 puts the large-object region after every `COPY` block, so a
        // pending one here means non-`pg_dump` input — flushed rather than
        // silently absorbing whatever follows into it.
        self.flush_large_objects();
        let (start, toc) = match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => (event.header_offset, None),
            // A TOC comment directly precedes the header: absorbed into the
            // `Data` span's outer boundary, so
            // `span.start <= header_offset` (`docs/design/decisions.md`,
            // "D33").
            Mode::Comment { start, toc, .. } => (start, toc),
            // Never observed in a well-formed dump, but every byte must
            // land somewhere.
            Mode::Statement { start, buf, toc, toc_owned } => {
                self.push_statement_span(start, &buf, toc, toc_owned);
                (event.header_offset, None)
            }
            // Same: a `COPY` header never follows an `INSERT` run in real
            // `pg_dump` output, the data format being dump-wide.
            Mode::InsertRun { .. } => {
                self.close_insert_run();
                (event.header_offset, None)
            }
        };
        self.pending_census = vec![Default::default(); event.header.columns.len()];
        self.pending_observer = None;
        self.pending_data = Some((start, event, self.pending_partition_root.take(), toc));
    }

    /// Fold one data row of the open `COPY` block into its array-shape
    /// census — see [`census_row`], shared with the interior workers a split
    /// `COPY` block is scanned by (`crate::leader`) — and hand it to the
    /// block's statistics observer, if it has one. `offset` is the row's
    /// absolute file offset.
    pub(crate) fn on_row(&mut self, offset: u64, raw: &[u8]) {
        census_row(&mut self.pending_census, raw);
        if let (Some(observer), Some((_, start, ..))) =
            (self.pending_observer.as_mut(), self.pending_data.as_ref())
        {
            observer.observe_row(offset - start.data_offset, raw);
        }
    }

    /// Gather statistics for the `COPY` block [`on_copy_start`](Self::on_copy_start)
    /// just opened: every row [`on_row`](Self::on_row) folds is handed to
    /// `observer`, and its answer is the block's [`CopyBlock::statistics`].
    pub(crate) fn observe_block(&mut self, observer: Box<dyn BlockObserver>) {
        self.pending_observer = Some(observer);
    }

    /// The open `COPY` block's statistics observer, for the interior workers
    /// that observe its rows in pieces instead of [`on_row`](Self::on_row)
    /// (`crate::leader::scan_region`).
    pub(crate) fn block_observer(&mut self) -> Option<&mut (dyn BlockObserver + 'static)> {
        self.pending_observer.as_deref_mut()
    }

    /// Union an already-folded census into the open `COPY` block's own — what
    /// a block whose rows were counted by interior workers states instead of
    /// the [`on_row`](Self::on_row) calls it never made
    /// (`crate::leader::scan_region`). Takes `&[ArrayShape]` rather than the
    /// leader's `Interior`: this module is L1 and the leader L4, so the shape
    /// vector is the L1 value they share (`docs/design/decisions.md`, "D68").
    ///
    /// Length-tolerant because a header-less block states no width, so the
    /// workers' union can be wider than what
    /// [`on_copy_start`](Self::on_copy_start) sized.
    pub(crate) fn absorb_census(&mut self, census: &[ArrayShape]) {
        if census.len() > self.pending_census.len() {
            self.pending_census.resize(census.len(), ArrayShape::default());
        }
        for (slot, shape) in self.pending_census.iter_mut().zip(census) {
            slot.merge(shape);
        }
    }

    pub(crate) fn on_copy_end(&mut self, end: CopyEnd) {
        // `crate::scan::CopyScanner` never emits `CopyEnd` without a prior
        // `CopyStart`, so this is always `Some`.
        let Some((start, copy_start, partition_root, toc)) = self.pending_data.take() else {
            return;
        };
        // Shared (`docs/design/decisions.md`, "D34"); a block that declined
        // records the allowance instead (`docs/design/decisions.md`, "D85").
        let gathered = self.pending_observer.take().map(|observer| {
            let _attributed = StatisticsScope::enter();
            observer.finish(end.terminator_offset - copy_start.data_offset)
        });
        let (statistics, statistics_declined) = match gathered {
            Some(BlockGathered::Gathered(statistics)) => (Some(Arc::new(statistics)), None),
            Some(BlockGathered::Declined { allowance }) => (None, Some(allowance)),
            None => (None, None),
        };
        let block = CopyBlock {
            header: copy_start.header,
            database: self.database.clone(),
            header_offset: copy_start.header_offset,
            data_offset: copy_start.data_offset,
            terminator_offset: end.terminator_offset,
            end_offset: end.end_offset,
            row_count: end.row_count,
            partition_root,
            statistics,
            statistics_declined,
            array_shapes: std::mem::take(&mut self.pending_census),
        };
        let owned = toc.is_some();
        self.push_span(start, SpanBody::Data(DataBlock::Copy(block)), toc, owned);
    }

    /// A `BEGIN;` line opened a large-object data region (I12) — see
    /// [`crate::scan::Event::LargeObjectStart`].
    ///
    /// Where v13-16's single archive entry and v17+'s one-per-object entries
    /// end up producing the same map. Neither this method nor
    /// [`on_large_object_end`](Self::on_large_object_end) pushes a span: the
    /// region stays *pending* until
    /// [`flush_large_objects`](Self::flush_large_objects) closes it, which
    /// happens only when something else is about to open, so consecutive
    /// `BEGIN;`/`COMMIT;` pairs merge into one span. I12 is what makes that
    /// sound without reading each entry's TOC `Type:` field; whatever *does*
    /// arrive in between closes the region the normal way.
    pub(crate) fn on_large_object_start(&mut self, offset: u64) {
        let (open_start, toc) = match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => (offset, None),
            Mode::Comment { start, toc, .. } => (start, toc),
            // Never observed in a well-formed dump (I12: nothing but a TOC
            // comment ever precedes a large-object entry's `BEGIN;`), but
            // every byte must land somewhere.
            Mode::Statement { start, buf, toc, toc_owned } => {
                self.push_statement_span(start, &buf, toc, toc_owned);
                (offset, None)
            }
            Mode::InsertRun { .. } => {
                self.close_insert_run();
                (offset, None)
            }
        };
        // Harvested whether or not this entry becomes its own span: one
        // that merges into an already-open region still named an owner.
        self.harvest_toc_cross_refs(&toc);
        match &mut self.pending_large_objects {
            // Continuing an already-open region: keep its own start/toc, not
            // this entry's.
            Some(_) => {}
            None => self.pending_large_objects = Some((open_start, offset, toc)),
        }
    }

    /// A `COMMIT;` line closed the large-object entry [`on_large_object_start`](Self::on_large_object_start)
    /// opened — see [`crate::scan::Event::LargeObjectEnd`]. Extends the
    /// pending region's running end; does not push a span (see that method's
    /// docs for why).
    pub(crate) fn on_large_object_end(&mut self, offset: u64) {
        if let Some(region) = &mut self.pending_large_objects {
            region.1 = offset;
        }
    }

    /// Close the pending large-object region, if one is open, into a real
    /// span. Called at the top of every [`push_span`](Self::push_span) (so
    /// any *other* span implies this one isn't being extended further), at
    /// [`on_copy_start`](Self::on_copy_start), and at
    /// [`finish`](Self::finish) (so a file ending right after the region's
    /// last `COMMIT;` still closes it).
    fn flush_large_objects(&mut self) {
        if let Some((start, _end, toc)) = self.pending_large_objects.take() {
            let owned = toc.is_some();
            self.push_span(
                start,
                SpanBody::Data(DataBlock::LargeObjects(LargeObjectRegion {
                    database: self.database.clone(),
                })),
                toc,
                owned,
            );
        }
    }

    /// Push the finished span for a [`Mode::InsertRun`] that has ended,
    /// however its caller found that out — [`step`](Self::step),
    /// [`flush_pending`](Self::flush_pending) at end of scan, or
    /// [`close_insert_run`](Self::close_insert_run) when a non-`push_span`
    /// entry point interrupts a still-open run.
    fn push_insert_run(
        &mut self,
        start: u64,
        table: String,
        database: Option<String>,
        row_count: u64,
        toc: Option<TocHeader>,
        toc_owned: bool,
    ) {
        self.push_span(
            start,
            SpanBody::Data(DataBlock::InsertRun(InsertRun { database, table, row_count })),
            toc,
            toc_owned,
        );
    }

    /// Close whatever [`Mode::InsertRun`] has accumulated into a real span —
    /// called from the non-`push_span` entry points that can interrupt a run
    /// ([`on_copy_start`](Self::on_copy_start),
    /// [`on_large_object_start`](Self::on_large_object_start)). A no-op if
    /// `self.mode` is anything else.
    fn close_insert_run(&mut self) {
        if let Mode::InsertRun { start, table, database, row_count, toc, toc_owned, .. } =
            std::mem::replace(&mut self.mode, Mode::Idle)
        {
            self.push_insert_run(start, table, database, row_count, toc, toc_owned);
        }
    }

    /// Finish the scan: whatever's still pending is closed out using `end`
    /// (the scan's own end offset) as the trigger that would otherwise have
    /// opened the next span.
    pub(crate) fn finish(mut self, end: u64) -> Vec<Span> {
        self.flush_pending(end);
        // A separate field from `mode`, so `flush_pending` does not cover
        // it: a file ending right after the region's last `COMMIT;` needs
        // this to close it.
        self.flush_large_objects();
        if let Some(last) = self.spans.last_mut() {
            last.end = end;
        }
        self.spans
    }

    /// The spans recognized so far, without consuming `self` — unlike
    /// [`finish`](Self::finish), callable only once, at true end of scan.
    /// `end` closes out the still-open last span, the caller supplying it
    /// because this module only ever decides where the *next* span starts.
    ///
    /// Only sound to call where nothing is mid-classification — `self.mode`
    /// is [`Mode::Idle`] **and** no large-object region is pending — since
    /// otherwise the last-pushed span is the one *before* the span open at
    /// `end`, and stamping its `end` there would be wrong.
    ///
    /// A `COPY` block's two edges are both such boundaries, and
    /// `crate::stream`'s mapping pass calls this at each of them: right after
    /// [`on_copy_end`](Self::on_copy_end), to bank progress at completed
    /// blocks (`mode` is `Idle` for the block's duration and I12 puts the
    /// large-object region strictly after it, so nothing pends one either);
    /// and right after [`on_copy_start`](Self::on_copy_start), at the *start*
    /// offset of the still-pending `Data` span, where `self.spans.last()` is
    /// the DDL span before it and I1 makes this the boundary
    /// `crate::preamble::dump_metadata_from_spans` may be called at.
    pub(crate) fn snapshot(&self, end: u64) -> Vec<Span> {
        debug_assert!(matches!(self.mode, Mode::Idle));
        debug_assert!(self.pending_large_objects.is_none());
        let mut spans = self.spans.clone();
        if let Some(last) = spans.last_mut() {
            last.end = end;
        }
        spans
    }

    /// The start offset of a comment block currently being absorbed —
    /// `None` unless `self.mode` is [`Mode::Comment`]. A caller that must stop
    /// scanning *before* the decision a `--`-prefixed run is waiting on
    /// ([`crate::index::scan_preamble`]) retreats its stop point here rather
    /// than guess the comment's kind (`docs/design/decisions.md`, "D32"):
    /// closing out at the stopping point would swallow it as a guessed
    /// [`SpanBody::Framing`]/[`SpanBody::Unparsed`] span, permanently wrong
    /// for a `-- Data for Name: ...` block.
    pub(crate) fn pending_comment_start(&self) -> Option<u64> {
        match &self.mode {
            Mode::Comment { start, .. } => Some(*start),
            _ => None,
        }
    }
}

/// A bare `SET ...;` or `SELECT pg_catalog.set_config(...);` — the two
/// statement shapes `_doSetFixedOutputState()` writes ahead of the archive
/// proper and `_selectTablespace()` writes ahead of a definition
/// (`docs/design/decisions.md`, "D31"). Both classify as framing rather than
/// `Unparsed` whichever producer wrote them; either way
/// `push_statement_span`'s `extract_statement_cross_refs` call still reads
/// the tablespace reference out of the text.
fn looks_like_framing_statement(stmt: &str) -> bool {
    let trimmed = stmt.trim_start();
    let upper_prefix = |kw: &str| {
        trimmed.len() >= kw.len()
            && trimmed.as_bytes()[..kw.len()].eq_ignore_ascii_case(kw.as_bytes())
    };
    upper_prefix("SET ") || upper_prefix("SELECT pg_catalog.set_config(")
}

fn classify(stmt: &str) -> SpanBody {
    if looks_like_framing_statement(stmt) {
        return SpanBody::Framing;
    }
    if let Some(rest) = strip_kw(stmt.trim_start(), "ALTER TYPE")
        && let Some((type_name, label)) = parse_alter_type_add_value_body(rest)
    {
        return SpanBody::AlterTypeAddValue { type_name, label };
    }
    match classify_statement(stmt) {
        Some(StatementShape::Table { name, columns }) => SpanBody::Table { name, columns },
        Some(StatementShape::Type(TypeDef { name, kind })) => SpanBody::TypeDef { name, kind },
        Some(StatementShape::Extension(Extension { name, schema })) => {
            SpanBody::Extension { name, schema }
        }
        Some(StatementShape::Collation(collation)) => SpanBody::Collation { collation },
        None => SpanBody::Unparsed,
    }
}

/// Scan `source` end to end and build its full file map — see the module
/// docs for what is and is not classified. Always scans to EOF, so
/// [`SpanBody::Unscanned`] never appears in the result.
pub async fn build_map(source: &dyn ByteRangeSource, options: &ScanOptions) -> Result<Vec<Span>> {
    let mut builder = Builder::new();

    scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => builder.on_copy_start(start),
            Event::Row(row) => builder.on_row(row.offset, row.raw),
            Event::CopyEnd(end) => builder.on_copy_end(end),
            Event::Line(line) => builder.feed_line(line.offset, line.raw),
            Event::DollarQuoteEnd(end) => builder.on_dollar_quote_end(end.offset),
            Event::LargeObjectStart(start) => builder.on_large_object_start(start.start_offset),
            Event::LargeObjectEnd(end) => builder.on_large_object_end(end.end_offset),
        }
        ControlFlow::Continue(())
    })
    .await?;

    let end = source.size().await?;
    let mut spans = builder.finish(end);
    attach_text(source, &mut spans).await?;
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copy::CopyHeader;

    /// Feed `lines` (each with a synthetic offset — `\n`-joined, matching
    /// how a real scan would number them) through a fresh [`Builder`] and
    /// return the finished spans.
    fn spans_of(lines: &[&str]) -> Vec<Span> {
        let mut builder = Builder::new();
        let mut offset = 0u64;
        for line in lines {
            builder.feed_line(offset, line.as_bytes());
            offset += line.len() as u64 + 1;
        }
        builder.finish(offset)
    }

    #[test]
    fn looks_like_toc_name_line_matches_the_real_grammar() {
        assert!(looks_like_toc_name_line(
            "-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres"
        ));
        assert!(looks_like_toc_name_line(
            "-- Name: EXTENSION postgres_fdw; Type: COMMENT; Schema: -; Owner: "
        ));
        assert!(!looks_like_toc_name_line("-- PostgreSQL database dump"));
        assert!(!looks_like_toc_name_line("-- Dumped from database version 18.6"));
        assert!(!looks_like_toc_name_line("--"));
    }

    #[test]
    fn looks_like_framing_statement_matches_set_and_set_config_only() {
        assert!(looks_like_framing_statement("SET statement_timeout = 0;"));
        assert!(looks_like_framing_statement(
            "SELECT pg_catalog.set_config('search_path', '', false);"
        ));
        assert!(looks_like_framing_statement("set client_encoding = 'UTF8';"));
        assert!(!looks_like_framing_statement("CREATE TABLE t (id integer);"));
        assert!(!looks_like_framing_statement("SELECT 1;"));
    }

    #[test]
    fn a_toc_commented_table_is_one_span_from_the_comment_through_the_statement() {
        let spans = spans_of(&[
            "--",
            "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
            "--",
            "",
            "CREATE TABLE public.t (",
            "    id integer",
            ");",
        ]);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start, 0);
        assert!(matches!(&spans[0].body, SpanBody::Table { name, .. } if name == "public.t"));
        assert_eq!(
            spans[0].toc,
            Some(TocHeader {
                name: "t".to_string(),
                kind: "TABLE".to_string(),
                schema: Some("public".to_string()),
                owner: Some("postgres".to_string()),
                tablespace: None,
            })
        );
    }

    #[test]
    fn parse_toc_header_line_handles_data_for_name_and_the_tablespace_suffix() {
        assert_eq!(
            parse_toc_header_line(
                "-- Data for Name: t; Type: TABLE DATA; Schema: public; Owner: postgres; \
                 Tablespace: fastspace"
            ),
            Some(TocHeader {
                name: "t".to_string(),
                kind: "TABLE DATA".to_string(),
                schema: Some("public".to_string()),
                owner: Some("postgres".to_string()),
                tablespace: Some("fastspace".to_string()),
            })
        );
    }

    /// `TOC_PREFIX_STATS`, the third prefix `_printTocEntry()` writes
    /// (`fixtures/18/objects/stats.sql`). Parses like the other two, and
    /// unlike `Data for ` it *is* a boundary signal.
    #[test]
    fn a_statistics_entry_parses_and_opens_a_span() {
        let line =
            "-- Statistics for Name: widgets; Type: STATISTICS DATA; Schema: objects; Owner: -";
        assert_eq!(
            parse_toc_header_line(line),
            Some(TocHeader {
                name: "widgets".to_string(),
                kind: "STATISTICS DATA".to_string(),
                schema: Some("objects".to_string()),
                owner: None,
                tablespace: None,
            })
        );
        assert!(looks_like_toc_name_line(line));
    }

    /// The one input on which `looks_like_toc_name_line`'s refusal of
    /// `"Data for "` changes an outcome, and therefore the only thing that
    /// can pin it: I31's `--disable-triggers` shape, where `SET SESSION
    /// AUTHORIZATION DEFAULT;` stands between the `-- Data for Name:` block
    /// and the `COPY` header it heads. Accepting the prefix would run the
    /// entry into that statement's span, where `push_statement_span`'s
    /// `Framing` veto discards the header outright; refusing keeps it on a
    /// `Framing` span of its own. The data span is unattributed either way,
    /// which is not what this test is about.
    ///
    /// Deficiency register: `deficiency: KD1` — under I31's shape every data
    /// span in the file loses its TOC attribution, `COPY` and `INSERT` alike,
    /// so `Span::toc` is `None` where a plain dump names the table and the
    /// coverage diagnostic under-reports by that much. **(c) unowned**, the
    /// shape being opt-in. Closing it takes three changes that land together —
    /// accept the prefix, veto on framing differently, re-tile the seam — and
    /// this test is the only thing that can observe the outcome.
    #[test]
    fn a_data_entry_keeps_its_own_span_when_disable_triggers_intervenes() {
        let mut builder = Builder::new();
        let mut offset = 0u64;
        for line in [
            "--",
            "-- Data for Name: t; Type: TABLE DATA; Schema: public; Owner: postgres",
            "--",
            "",
            "SET SESSION AUTHORIZATION DEFAULT;",
            "",
            "ALTER TABLE public.t DISABLE TRIGGER ALL;",
            "",
        ] {
            builder.feed_line(offset, line.as_bytes());
            offset += line.len() as u64 + 1;
        }
        let header_offset = offset;
        builder.on_copy_start(CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        });
        let terminator_offset = header_offset + 48;
        let end_offset = terminator_offset + 3;
        builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 });
        let spans = builder.finish(end_offset);

        assert_eq!(spans[0].start, 0);
        assert!(matches!(spans[0].body, SpanBody::Framing));
        assert!(spans[0].toc_owned, "the entry survives on the comment block's own span");
        assert_eq!(spans[0].toc.as_ref().map(|t| t.name.as_str()), Some("t"));

        let data = spans.last().expect("the COPY block is the last span");
        assert!(matches!(data.body, SpanBody::Data(_)));
        assert_eq!(data.toc, None, "I31: the intervening statements cost the data its entry");
    }

    /// The whole statistics entry — comment block and the
    /// `pg_restore_relation_stats()` call it heads — is **one** span owning
    /// its TOC entry.
    #[test]
    fn a_statistics_entry_is_one_attributed_span() {
        let spans = spans_of(&[
            "--",
            "-- Statistics for Name: widgets; Type: STATISTICS DATA; Schema: objects; Owner: -",
            "--",
            "",
            "SELECT * FROM pg_catalog.pg_restore_relation_stats(",
            "    'version', '180000'::integer,",
            "    'relation', 'objects.widgets'::regclass,",
            "    'relpages', '1'::integer",
            ");",
        ]);
        assert_eq!(spans.len(), 1);
        assert!(spans[0].toc_owned);
        assert_eq!(spans[0].toc.as_ref().map(|t| t.kind.as_str()), Some("STATISTICS DATA"));
    }

    /// A `-- Data for Name: ...` entry heading an `INSERT` run is **one**
    /// span owning its TOC entry, the same shape `on_copy_start` produces for
    /// a `COPY` block: the absorption happens in `Mode::Comment`'s close arm,
    /// so the run starts at the comment's offset rather than at its first
    /// `INSERT` line.
    #[test]
    fn an_insert_run_absorbs_its_data_entry_and_owns_it() {
        let spans = spans_of(&[
            "--",
            "-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres",
            "--",
            "",
            "INSERT INTO public.widgets VALUES (1, 'alpha');",
            "INSERT INTO public.widgets VALUES (2, 'beta');",
        ]);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].start, 0, "the span opens at the comment, not at the first INSERT");
        assert!(spans[0].toc_owned);
        assert_eq!(spans[0].toc.as_ref().map(|t| t.kind.as_str()), Some("TABLE DATA"));
        match &spans[0].body {
            SpanBody::Data(DataBlock::InsertRun(run)) => {
                assert_eq!(run.table, "public.widgets");
                assert_eq!(run.row_count, 2);
            }
            other => panic!("expected an INSERT run, got {other:?}"),
        }
    }

    /// Every line-shape [`Builder::insert_run_line`] declines, in one run,
    /// counted correctly: the two paths share a [`StatementScan`] and each
    /// line reaches exactly one of them, so a run whose lines alternate
    /// between them folds the same way a run of plain statements does.
    ///
    /// The shapes, in order: a value carrying a raw newline; a continuation
    /// that *looks* like a comment and is really string content (the
    /// `in_quote` guard); a blank line; and a statement spelled with an extra
    /// space, which the byte-prefix check refuses and
    /// `parse_insert_target` then accepts.
    #[test]
    fn an_insert_run_folds_the_same_way_when_its_lines_take_the_slow_path() {
        let spans = spans_of(&[
            "--",
            "-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres",
            "--",
            "",
            "INSERT INTO public.widgets VALUES (1, 'alpha');",
            "INSERT INTO public.widgets VALUES (2, 'two",
            "-- lines, and this is not a comment');",
            "",
            "INSERT INTO  public.widgets VALUES (3, 'gamma');",
            "INSERT INTO public.widgets VALUES (4, 'it''s ok');",
        ]);
        assert_eq!(spans.len(), 1);
        match &spans[0].body {
            SpanBody::Data(DataBlock::InsertRun(run)) => {
                assert_eq!(run.table, "public.widgets");
                assert_eq!(run.row_count, 4);
            }
            other => panic!("expected an INSERT run, got {other:?}"),
        }
    }

    /// The byte-prefix check must not let `public.widgets2` continue
    /// `public.widgets`'s run: it is a prefix of the longer name, so the
    /// byte after it has to end the identifier.
    #[test]
    fn a_table_whose_name_extends_the_runs_own_starts_a_new_run() {
        let spans = spans_of(&[
            "INSERT INTO public.widgets VALUES (1);",
            "INSERT INTO public.widgets2 VALUES (2);",
        ]);
        let runs: Vec<_> = spans
            .iter()
            .filter_map(|s| match &s.body {
                SpanBody::Data(DataBlock::InsertRun(run)) => {
                    Some((run.table.as_str(), run.row_count))
                }
                _ => None,
            })
            .collect();
        assert_eq!(runs, vec![("public.widgets", 1), ("public.widgets2", 1)]);
    }

    /// `fixtures/16/objects/verbose.sql`'s `Schema: -` and trailing empty
    /// `Owner: ` both mean "none", per I16 and `_printTocEntry()`'s two ways
    /// of writing it.
    #[test]
    fn a_hyphen_schema_and_an_empty_owner_both_parse_to_none() {
        let header = parse_toc_header_line(
            "-- Name: EXTENSION postgres_fdw; Type: COMMENT; Schema: -; Owner: ",
        )
        .unwrap();
        assert_eq!(header.schema, None);
        assert_eq!(header.owner, None);
    }

    /// `--no-owner` writes `Owner: -`, the other of `_printTocEntry()`'s two
    /// "no owner" shapes (`fixtures/*/edge_cases/no-owner.sql`).
    #[test]
    fn a_hyphen_owner_parses_to_none() {
        let header =
            parse_toc_header_line("-- Name: sample_fn(); Type: FUNCTION; Schema: public; Owner: -")
                .unwrap();
        assert_eq!(header.owner, None);
    }

    #[test]
    fn a_non_toc_comment_line_does_not_parse_as_a_toc_header() {
        assert_eq!(parse_toc_header_line("-- PostgreSQL database dump"), None);
        assert_eq!(parse_toc_header_line("-- TOC entry 7 (class 2615 OID 16386)"), None);
        assert_eq!(parse_toc_header_line("-- Dependencies: 2"), None);
    }

    /// `--verbose` inserts `-- TOC entry ...`/`-- Dependencies: ...` lines
    /// ahead of the Name line (`fixtures/*/objects/verbose.sql`) — they don't
    /// parse as headers themselves, and don't stop the Name line after them
    /// from being recognized.
    #[test]
    fn verbose_lines_ahead_of_the_name_line_do_not_prevent_it_parsing() {
        let spans = spans_of(&[
            "--",
            "-- TOC entry 7 (class 2615 OID 16386)",
            "-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres",
            "--",
            "",
            "CREATE SCHEMA objects;",
        ]);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].toc.as_ref().map(|t| t.name.as_str()), Some("objects"));
    }

    /// A `Data` span's `toc` comes from its own `-- Data for Name: ...`
    /// comment, threaded through `pending_data` from `on_copy_start` to
    /// `on_copy_end`.
    #[test]
    fn a_data_span_carries_the_toc_header_its_data_for_name_comment_parsed_to() {
        let mut builder = Builder::new();
        let mut offset = 0u64;
        for line in [
            "--",
            "-- Data for Name: t; Type: TABLE DATA; Schema: public; Owner: postgres",
            "--",
            "",
        ] {
            builder.feed_line(offset, line.as_bytes());
            offset += line.len() as u64 + 1;
        }
        let header_offset = offset;
        builder.on_copy_start(CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        });
        let terminator_offset = header_offset + 48;
        let end_offset = terminator_offset + 3;
        builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 });

        let spans = builder.finish(end_offset);
        assert_eq!(spans.len(), 1);
        assert!(matches!(spans[0].body, SpanBody::Data(_)));
        assert_eq!(spans[0].toc.as_ref().map(|t| t.kind.as_str()), Some("TABLE DATA"));
    }

    /// A statement with no preceding TOC comment at all — the header-less
    /// fallback — carries no `toc` and `toc_owned: false`, there being no
    /// governing entry for it to inherit.
    #[test]
    fn a_statement_with_no_toc_comment_carries_no_toc_header() {
        let spans = spans_of(&["CREATE EXTENSION pgcrypto;"]);
        assert_eq!(spans[0].toc, None);
        assert!(!spans[0].toc_owned);
    }

    /// TOC inheritance: a follow-on statement with no TOC comment of its own
    /// (`ALTER SCHEMA ... OWNER TO ...;`, mirroring
    /// `fixtures/*/objects/default.sql`) inherits the governing entry's header
    /// instead of carrying `None` (`docs/design/decisions.md`, "D31"). Both
    /// spans carry the *same* `toc`; only the first has `toc_owned: true`.
    #[test]
    fn a_follow_on_statement_inherits_the_governing_toc_header() {
        let spans = spans_of(&[
            "--",
            "-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres",
            "--",
            "",
            "CREATE SCHEMA objects;",
            "",
            "ALTER SCHEMA objects OWNER TO postgres;",
        ]);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].body, SpanBody::Unparsed);
        assert_eq!(spans[0].toc, spans[1].toc, "both spans belong to the same TOC entry");
        assert!(spans[0].toc.is_some());
        assert!(spans[0].toc_owned, "the first span's own comment carried the header");
        assert!(!spans[1].toc_owned, "the second span inherited it");
    }

    /// Inheritance keeps propagating across more than one follow-on in a
    /// row — every span until the next boundary carries the *same* governing
    /// header, mirroring `fixtures/16/objects/default.sql`'s
    /// `objects.simple_config`.
    #[test]
    fn several_consecutive_follow_ons_all_inherit_the_same_header() {
        let spans = spans_of(&[
            "--",
            "-- Name: t; Type: TEXT SEARCH CONFIGURATION; Schema: public; Owner: postgres",
            "--",
            "",
            "CREATE TEXT SEARCH CONFIGURATION public.t (COPY = simple);",
            "",
            "ALTER TEXT SEARCH CONFIGURATION public.t ALTER MAPPING FOR asciiword WITH simple;",
            "",
            "ALTER TEXT SEARCH CONFIGURATION public.t ALTER MAPPING FOR word WITH simple;",
        ]);
        assert_eq!(spans.len(), 3);
        assert!(spans[0].toc_owned);
        assert!(!spans[1].toc_owned);
        assert!(!spans[2].toc_owned);
        assert!(spans[0].toc.is_some());
        assert_eq!(spans[0].toc, spans[1].toc);
        assert_eq!(spans[1].toc, spans[2].toc);
    }

    /// A mid-file `SET default_tablespace = ...;` classifies as `Framing`
    /// (`looks_like_framing_statement`), never inherits even when it directly
    /// follows a governed entry, and clears the governing header for whatever
    /// comes after it (`docs/design/decisions.md`, "D31").
    #[test]
    fn a_mid_file_framing_statement_never_inherits_and_clears_the_governing_header() {
        let spans = spans_of(&[
            "--",
            "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
            "--",
            "",
            "CREATE TABLE public.t (id integer);",
            "",
            "SET default_tablespace = '';",
            "",
            "CREATE EXTENSION pgcrypto;",
        ]);
        assert_eq!(spans.len(), 3);
        assert!(spans[0].toc.is_some());
        assert_eq!(spans[1].body, SpanBody::Framing);
        assert_eq!(spans[1].toc, None, "a Framing span never carries an inherited header");
        assert!(!spans[1].toc_owned);
        assert_eq!(
            spans[2].toc, None,
            "the Framing statement must clear the governing header for what follows it"
        );
    }

    /// `\connect` clears the governing header the same way `Framing` does —
    /// a statement in the newly-connected database has no TOC entry of its
    /// own to inherit from the previous database's last one.
    #[test]
    fn connect_clears_the_governing_toc_header() {
        let spans = spans_of(&[
            "--",
            "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
            "--",
            "",
            "CREATE TABLE public.t (id integer);",
            "\\connect two",
            "CREATE EXTENSION pgcrypto;",
        ]);
        let last = spans.last().unwrap();
        assert!(matches!(&last.body, SpanBody::Extension { name, .. } if name == "pgcrypto"));
        assert_eq!(last.toc, None);
    }

    /// A `COPY` block with no TOC comment of its own resets inheritance the
    /// same way `Framing`/`Connect` do: the entry governing *before* the block
    /// does not leak past it, the `Data` span itself carrying `toc: None`.
    #[test]
    fn a_copy_block_with_no_comment_of_its_own_resets_inheritance() {
        let mut builder = Builder::new();
        let mut offset = 0u64;
        for line in [
            "--",
            "-- Name: t; Type: TABLE; Schema: public; Owner: postgres",
            "--",
            "",
            "CREATE TABLE public.t (id integer);",
            "",
        ] {
            builder.feed_line(offset, line.as_bytes());
            offset += line.len() as u64 + 1;
        }
        let header_offset = offset;
        builder.on_copy_start(CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        });
        let terminator_offset = header_offset + 48;
        let end_offset = terminator_offset + 3;
        builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 });
        offset = end_offset;
        builder.feed_line(offset, b"CREATE EXTENSION pgcrypto;");

        let spans = builder.finish(offset + 27);
        assert_eq!(spans.len(), 3, "table, COPY block, extension");
        assert!(matches!(spans[1].body, SpanBody::Data(_)));
        assert_eq!(spans[1].toc, None, "no comment preceded this COPY header");
        assert_eq!(
            spans[2].toc, None,
            "the table's header must not leak across the uncommented COPY block"
        );
    }

    #[test]
    fn a_non_toc_comment_and_a_bare_statement_are_two_adjacent_spans() {
        let spans =
            spans_of(&["-- just a note, not a TOC entry", "", "CREATE EXTENSION pgcrypto;"]);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].body, SpanBody::Framing);
        assert!(matches!(&spans[1].body, SpanBody::Extension { name, .. } if name == "pgcrypto"));
        assert_eq!(spans[0].end, spans[1].start, "no gap between adjacent spans");
    }

    #[test]
    fn a_toc_comment_block_with_no_trailing_statement_closes_at_scan_end() {
        let spans = spans_of(&["--", "-- Name: x; Type: SCHEMA; Schema: -; Owner: postgres", "--"]);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].body, SpanBody::Unparsed);
    }

    #[test]
    fn connect_opens_its_own_connect_span_and_switches_the_database() {
        let spans = spans_of(&["\\connect one", "CREATE EXTENSION pgcrypto;"]);
        assert_eq!(spans[0].body, SpanBody::Connect { database: "one".to_string() });
        assert_eq!(
            spans[0].database.as_deref(),
            Some("one"),
            "the \\connect line's own span is attributed to the database it establishes"
        );
        assert_eq!(spans[1].database.as_deref(), Some("one"));
    }

    /// The case the module docs' "Span boundaries" section exists for: a
    /// dollar-quoted function body swallows its own closing `;` entirely
    /// (never reaches `Event::Line`), so only the next TOC comment's boundary
    /// can close the span. `feed_line` here is given only the lines a real
    /// scan would still emit.
    #[test]
    fn a_dollar_quoted_function_with_no_trailing_statement_closes_at_the_next_toc_comment() {
        let spans = spans_of(&[
            "--",
            "-- Name: f; Type: FUNCTION; Schema: public; Owner: postgres",
            "--",
            "",
            "CREATE FUNCTION public.f() RETURNS void",
            "    LANGUAGE plpgsql",
            // The `AS $$ ... $$;` body is entirely invisible to `Event::Line`
            // (`crate::scan`'s dollar-quote handling) — omitted here, exactly
            // matching what a real scan would (not) emit.
            "",
            "--",
            "-- Name: g; Type: SCHEMA; Schema: -; Owner: postgres",
            "--",
            "",
            "CREATE SCHEMA g;",
        ]);
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].body, SpanBody::Unparsed, "CREATE FUNCTION isn't a classified shape");
        assert_eq!(spans[0].end, spans[1].start);
        assert_eq!(spans[1].body, SpanBody::Unparsed, "CREATE SCHEMA isn't a classified shape");
    }

    #[test]
    fn a_backslash_meta_command_is_its_own_single_line_framing_span() {
        let spans = spans_of(&["\\restrict aToken", "CREATE EXTENSION pgcrypto;"]);
        assert_eq!(spans[0].body, SpanBody::Framing);
        assert_eq!(spans[0].end, spans[1].start);
    }

    /// I9's two-line version-header block gets its own span kind rather than
    /// generic `Framing`, so `crate::preamble::dump_metadata_from_spans` can
    /// recover the strings without re-reading the file.
    #[test]
    fn version_header_lines_become_their_own_span() {
        let spans = spans_of(&[
            "-- Dumped from database version 16.15",
            "-- Dumped by pg_dump version 16.15",
            "",
            "CREATE EXTENSION pgcrypto;",
        ]);
        assert_eq!(spans.len(), 2);
        assert_eq!(
            spans[0].body,
            SpanBody::VersionHeader {
                server_version: Some("16.15".to_string()),
                pg_dump_version: Some("16.15".to_string()),
            }
        );
    }

    /// A `--binary-upgrade` dump's `ALTER TYPE ... ADD VALUE ...;` (I6) gets
    /// its own span kind rather than falling into the generic `Unparsed`
    /// bucket, so `crate::preamble::dump_metadata_from_spans` can fold the
    /// label into the `TypeDef` span it targets.
    #[test]
    fn binary_upgrade_add_value_becomes_its_own_span() {
        let spans = spans_of(&["ALTER TYPE public.mood ADD VALUE 'sad';"]);
        assert_eq!(
            spans[0].body,
            SpanBody::AlterTypeAddValue {
                type_name: "public.mood".to_string(),
                label: "sad".to_string(),
            }
        );
    }

    /// The case [`Builder::snapshot`] exists for: `stream.rs`'s live segment
    /// needs a tiling span list *before* the scan reaches its true end, right
    /// after each `CopyEnd` — the one point `on_copy_end` guarantees `mode`
    /// is back to `Idle`. `check_tiling` is checked at every such boundary
    /// rather than only at the very end.
    #[test]
    fn snapshot_tiles_the_prefix_seen_so_far_at_every_copy_end() {
        fn feed(builder: &mut Builder, offset: &mut u64, line: &str) {
            builder.feed_line(*offset, line.as_bytes());
            *offset += line.len() as u64 + 1;
        }

        let mut builder = Builder::new();
        let mut offset = 0u64;

        feed(&mut builder, &mut offset, "CREATE EXTENSION pgcrypto;");
        assert!(check_tiling(&builder.snapshot(offset), offset).is_empty());

        feed(&mut builder, &mut offset, "CREATE SCHEMA g;");
        assert!(check_tiling(&builder.snapshot(offset), offset).is_empty());

        let header_offset = offset;
        builder.on_copy_start(CopyStart {
            header: CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        });
        let terminator_offset = header_offset + 48;
        let end_offset = terminator_offset + 3;
        builder.on_copy_end(CopyEnd { terminator_offset, end_offset, row_count: 1 });
        offset = end_offset;

        let snapshot = builder.snapshot(end_offset);
        let issues = check_tiling(&snapshot, end_offset);
        assert!(issues.is_empty(), "{issues:?}");
        assert!(matches!(snapshot.last().unwrap().body, SpanBody::Data(_)));
        assert_eq!(snapshot.last().unwrap().end, end_offset);

        feed(&mut builder, &mut offset, "CREATE SCHEMA h;");
        let finished = builder.finish(offset);
        let issues = check_tiling(&finished, offset);
        assert!(issues.is_empty(), "{issues:?}");
        assert_eq!(finished.len(), 4, "extension, schema g, the COPY block, schema h");
    }

    /// `snapshot` and `finish`, called at the same watermark once nothing
    /// more will ever be fed, must agree; `finish` just also takes
    /// ownership.
    #[test]
    fn snapshot_agrees_with_finish_at_the_same_watermark() {
        let lines = ["CREATE EXTENSION pgcrypto;", "", "CREATE SCHEMA g;"];
        let drive = || {
            let mut builder = Builder::new();
            let mut offset = 0u64;
            for line in lines {
                builder.feed_line(offset, line.as_bytes());
                offset += line.len() as u64 + 1;
            }
            (builder, offset)
        };

        let (builder, offset) = drive();
        let via_snapshot = builder.snapshot(offset);
        let (builder, offset) = drive();
        let via_finish = builder.finish(offset);

        assert_eq!(via_snapshot, via_finish);
    }
}
