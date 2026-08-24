//! The full file map: an ordered, tiling set of [`Span`]s covering every
//! byte of a `pg_dump` plain-format file
//! (`docs/design/roadmap-phase3-object-inventory.md`, "What this phase is
//! for" and "The map is the structure, not a description of it").
//!
//! [`build_map`] classifies DDL statements directly; a TOC comment block is
//! recognized both as a **boundary** — see "Span boundaries" below — and, as
//! of Phase 3.3, as an **enrichment layer** read off the same lines: owner,
//! kind label, the `Tablespace:` field, and a per-file TOC-coverage count
//! (`roadmap-phase3-object-inventory.md`, "Scanning: one statement-driven
//! pass, TOC comments as an enrichment layer"). See "TOC enrichment" below.
//!
//! ## Span boundaries: TOC-block-anchored, with a statement-grammar fallback
//!
//! A span opens at the `--` of a TOC comment block (recognized lexically —
//! any run of `--`-prefixed lines containing one that matches `-- Name: ...;
//! Type: ...`) or, absent one, at the first byte of a recognized statement.
//! It runs greedily to the byte before the next span opens
//! (`roadmap-phase3-object-inventory.md`, "Span boundaries: object-anchored
//! and greedy") — this module never computes a span's `end` directly; it
//! only ever decides where the *next* span starts, and a caller-side pass
//! derives `end` as `next.start` (or the scan's end, for the last span).
//!
//! **Why TOC-block boundaries, not pure statement-completion tracking.** A
//! `CREATE FUNCTION`/`PROCEDURE` body is dollar-quoted, and
//! `crate::scan::CopyScanner` never emits an [`crate::scan::Event::Line`]
//! for a dollar-quoted line — not even the one that closes the tag with the
//! statement's own terminating `;` on it (`docs/status/history/2026-08-23.md`,
//! "Dollar-quoted lines never reach `Event::Line`"). So a tracker watching
//! only the visible line stream cannot observe that such a statement closed.
//! Every real `pg_dump` entry — `CREATE FUNCTION` included — carries its own
//! `-- Name: ...; Type: ...` TOC header, and that header can *never* appear
//! inside a dollar-quoted body (the scanner already filters those lines out
//! before any event reaches this module, fake-looking `-- Name:` text
//! included), so recognizing the next entry's header is a sound, general
//! "the previous span has ended" signal that sidesteps the problem entirely.
//!
//! For input carrying **no** TOC headers there is a second signal, added in
//! Phase 3.2.3: [`crate::scan::Event::DollarQuoteEnd`], a position-only event
//! marking where a dollar-quoted region closed, which
//! [`Builder::on_dollar_quote_end`] treats as completing whatever statement
//! is in flight. Without it the first dollar-quoted body in a header-less
//! file absorbs every statement after it into one span. The lines themselves
//! stay unsurfaced either way.
//! The statement-grammar fallback (used for content with no TOC header —
//! `\connect`/`\restrict`/framing lines, and any statement, like a trailing
//! `ALTER ... OWNER TO`, that isn't its own TOC entry) is exactly
//! [`crate::preamble::statement_complete`], hardened in this same slice for
//! the double-quote/`--`-comment cases a five-keyword-triggered accumulator
//! never used to reach.
//!
//! **Consequence: no "grouping".** `docs/design/roadmap-phase3-object-inventory.md`
//! lists grouping (folding an object's trailing `ALTER ... OWNER TO` into
//! the same TOC entry) as something the TOC layer *contributes* — "Without
//! it: They tile as two adjacent spans instead of one." That remains this
//! module's behavior: a definition and its ungrouped trailing statement tile
//! as two adjacent spans, per the design doc's own "graceful degradation"
//! framing. Grouping (via the TOC's `Dependencies:` field) is not in either
//! 3.3's or 3.4's specified scope — it has no assigned slice yet. **Not
//! grouping is not the same as not attributing** — see "TOC inheritance"
//! below: the two spans stay separate, but as of Phase 3.3.1 both carry the
//! same [`Span::toc`].
//!
//! ## What has landed, slice by slice, and what's still outside
//! - **The `Data`-span fast path for `INSERT` runs and the large-object
//!   region — landed in Phase 3.6.** [`DataBlock`] is what [`SpanBody::Data`]
//!   holds now: [`DataBlock::Copy`] for a `COPY` block (unchanged),
//!   [`DataBlock::InsertRun`] for a run of `INSERT INTO <table> ...;`
//!   statements (`Mode::InsertRun`, entered from `Mode::Statement`'s first
//!   line), and [`DataBlock::LargeObjects`] for the whole
//!   `BEGIN;`/`COMMIT;`-wrapped large-object region (I12), merged across
//!   however many archive entries `pg_dump` split it into
//!   ([`Builder::on_large_object_start`]). `crate::scan` recognizes a bare
//!   `BEGIN;`/`COMMIT;` pair at the scanner level, the same way it recognizes
//!   a `COPY` header/`\.` pair, and skips everything in between unread — see
//!   [`crate::scan::Event::LargeObjectStart`]/[`crate::scan::Event::LargeObjectEnd`].
//!   `INSERT` runs stay a `feed_line`-level concern instead (no new scanner
//!   state): the win there is not re-parsing/re-storing text per row, which
//!   reusing [`statement_complete`]'s already-hardened quote/paren tracking
//!   already gets, and the phase's own gating measurements don't cover
//!   `INSERT`-run throughput specifically (see "Verification" in
//!   `roadmap-phase3-object-inventory.md`). Neither kind carries the inner
//!   offsets `DataBlock::Copy` does — nothing reads their rows yet
//!   (Phase 8, unscheduled). Notes: `docs/design/roadmap-phase3.6-bulk-region-fast-path-notes.md`.
//! - **Wiring into `DumpIndex`/`crate::cache`/`crate::stream` — landed in
//!   Phase 3.2.1 through 3.2.1.2.1.** [`Builder`] (this module's
//!   boundary/classification state machine) is driven directly by
//!   [`crate::index::build_index`], [`crate::index::scan_preamble`] and
//!   `crate::stream`'s mapping pass, fed the same [`crate::scan::Event`]
//!   stream those already walk — [`build_map`] itself is a thin wrapper over
//!   the same [`Builder`], kept for callers (and this module's own tests)
//!   that just want a span list. `DumpIndex::spans` is the primary,
//!   persisted structure; `DumpIndex::blocks()`/`blocks_for` are filters over
//!   it. [`SpanBody::Unscanned`] covers whatever a partial scan has not
//!   reached, so **every** `DumpIndex` tiles its file — one built by a query
//!   included, with no exemption for a resumed stream. `DumpMetadata` is
//!   [`crate::preamble::dump_metadata_from_spans`] — a derived view over
//!   `spans`, computed once, the way the design's "The span is the
//!   container" section calls for; it's what [`SpanBody::Connect`],
//!   [`SpanBody::VersionHeader`] and [`SpanBody::AlterTypeAddValue`] exist
//!   for, rather than folding into generic [`SpanBody::Framing`]/[`SpanBody::Unparsed`].
//!
//!   A [`Builder`] that starts partway through a file opens its first span at
//!   its own first recognized content, **not** at the byte it began reading:
//!   closing that seam against whatever already covers the bytes before it is
//!   the caller's job, and `crate::stream::splice` does it by extending the
//!   preceding span — the same rule [`Builder::push_span`] applies to every
//!   other boundary, and what keeps a map assembled across several scans
//!   identical to one built in a single pass.
//! - **Span text and diagnostics — landed in Phase 3.2.2.** [`attach_text`]
//!   fills [`Span::text`] by slicing the file at each span's own offsets,
//!   capped at [`TEXT_CAP`] with a `truncated` marker; `Data` and `Unscanned`
//!   spans store none. [`check_tiling`] now has production callers
//!   ([`crate::index::build_index`] and `crate::stream`'s mapping pass), which
//!   report a failure as a [`crate::diagnostic::DiagnosticKind::TilingBroken`]
//!   on the index and return the map anyway — a hole is a bug in this module,
//!   never a reason to refuse the file.
//! - **TOC enrichment — landed in Phase 3.3.** [`Span::toc`] is filled by
//!   [`parse_toc_header_line`] whenever a comment block's TOC-Name line
//!   parses: `-- Name: ...` or its `-- Data for Name: ...` sibling (I3),
//!   through `; Type: ...; Schema: ...; Owner: ...` and the optional trailing
//!   `; Tablespace: ...` (I16). A `-` or empty field parses to `None`, matching
//!   `_printTocEntry()`'s two ways of writing "no value" (`sanitize_line`'s
//!   `want_hyphen` argument differs by field and by caller). The verbose
//!   `-- TOC entry N (class C OID O)` / `-- Dependencies: ...` lines that can
//!   precede the Name line (`--verbose`) are ordinary comment lines to this
//!   parser — recognized as such, contributing nothing, exactly like any
//!   other line that fails to parse as a header. [`crate::index::DumpIndex`]'s
//!   per-file TOC-coverage figure (`crate::index::toc_coverage_diagnostic`) is
//!   just `spans.iter().filter(|s| s.toc.is_some()).count()` against
//!   `spans.len()` — no separate counter, since `Span::toc` already carries
//!   the fact.
//! - **TOC inheritance for follow-on statements — landed in Phase 3.3.1.**
//!   3.3 shipped a coverage figure that read ~50% on a healthy, fully-TOC'd
//!   dump: a TOC entry is not one statement, and every follow-on
//!   (`ALTER ... OWNER TO`, `ALTER TEXT SEARCH CONFIGURATION ... ADD MAPPING
//!   FOR`, ...) carried `toc: None`, uncovered. `Builder`'s `governing_toc`
//!   field fixes this — it's the entry a comment-less statement inherits, updated
//!   by every [`Builder::push_span`] call: set to that span's own `toc` for a
//!   plain statement or `Data` span, cleared to `None` for `Framing`/
//!   `Connect`/`VersionHeader`. [`Span::toc_owned`] is the "separate record
//!   of whether it carried the header text itself" the design calls for —
//!   `true` only for the span whose own comment `toc` came from, `false` for
//!   every span that inherited it. Coverage now counts attribution
//!   (`toc.is_some()`, unchanged code — inheritance alone makes the numerator
//!   right); an object *census* (`pgdq info`'s `object kinds:`) counts
//!   `toc_owned` spans instead, one per archive entry, since counting every
//!   attributed span would double it. No grouping either way — see
//!   "Consequence: no 'grouping'" above.
//! - **The cross-reference set — landed in Phase 3.4.** [`Builder`] grows
//!   `roles`/`tablespaces` (both [`crate::index::DumpIndex`] fields of the
//!   same name), accumulated as spans close: [`Builder::push_span`] reads
//!   `toc.owner`/`toc.tablespace`, and [`Builder::push_statement_span`] scans
//!   the closing statement's own text via
//!   [`crate::preamble::extract_statement_cross_refs`] for `OWNER TO`,
//!   `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, and `SET
//!   default_tablespace` — the two sources `Span::toc` alone can't cover
//!   (`roadmap-phase3-object-inventory.md`, "What a span carries").

use std::collections::BTreeSet;
use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::index::CopyBlock;
use crate::io::ByteRangeSource;
use crate::preamble::{
    StatementShape, classify_statement, in_open_quote, push_stmt_line, statement_complete,
};
use crate::preamble::{TypeDef, parse_connect};
use crate::scan::{Event, ScanOptions, scan};

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
    /// cache so `pgdq info` answers without re-reading the dump
    /// (`docs/design/roadmap-phase3-object-inventory.md`, "Span text comes
    /// from the file, not from the parser"). `None` until [`attach_text`] has
    /// run, and permanently `None` for [`SpanBody::Data`] and
    /// [`SpanBody::Unscanned`] spans, whose bytes are unbounded and carry
    /// nothing a reader wants.
    pub text: Option<SpanText>,
    /// The TOC entry this span **belongs to** — not necessarily the comment
    /// this span's own bytes start with. A follow-on statement with no TOC
    /// comment of its own (`ALTER ... OWNER TO`, `ALTER TEXT SEARCH
    /// CONFIGURATION ... ADD MAPPING FOR`, ...) **inherits** the governing
    /// entry's header rather than carrying `None`
    /// (`docs/design/roadmap-phase3-object-inventory.md`, "Span boundaries:
    /// statement-anchored, object-attributed, greedy") — [`toc_owned`](Self::toc_owned)
    /// is what distinguishes the two. `None` for a span with no governing
    /// entry at all: the header-less-input fallback, or one of the kinds
    /// inheritance never crosses (`Framing`, `Connect`, `VersionHeader`).
    /// Independent of, and not a substitute for, [`SpanBody`]'s own per-kind
    /// fields: the TOC comment and the statement grammar are two
    /// separately-sourced observations of the same object
    /// (`docs/design/roadmap-phase3-object-inventory.md`, "Scanning: one
    /// statement-driven pass, TOC comments as an enrichment layer").
    pub toc: Option<TocHeader>,
    /// Whether *this span's own* preceding comment carried the TOC header
    /// text (`true`), as opposed to `toc` being inherited from an earlier
    /// entry's span (`false`) — always `false` when `toc` is `None`. This is
    /// the "separate record of whether it carried the header text itself"
    /// `roadmap-phase3-object-inventory.md`'s "Span boundaries" section calls
    /// for: an object census (`pgdq info`'s `object kinds:`) counts
    /// `toc_owned` spans, one per archive entry, while TOC-coverage counts
    /// every attributed span (`toc.is_some()`), inherited ones included —
    /// counting `toc.is_some()` for both would double-count every object that
    /// has a follow-on statement.
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
    /// `te->desc` — one of the closed ~63-value vocabulary I16 documents
    /// (`TABLE`, `FK CONSTRAINT`, `POLICY`, `TABLE DATA`, ...). Never `None`:
    /// `_printTocEntry()` always writes a `Type:` field.
    pub kind: String,
    /// `None` for `Schema: -` (no namespace — a database-level or
    /// namespace-less object).
    pub schema: Option<String>,
    /// `None` for `Owner: -` (`--no-owner`, or an owner-less kind like
    /// `COMMENT`) or `Owner: ` (empty — some entry kinds pass an empty string
    /// rather than `NULL` through `sanitize_line`, observed on `COMMENT`
    /// entries). Both mean "no owner recorded here" to a reader.
    pub owner: Option<String>,
    /// The `; Tablespace: <name>` suffix, present only when the entry has a
    /// non-default tablespace and `--no-tablespaces` was not given.
    pub tablespace: Option<String>,
}

/// Parse pg_dump's TOC header line into a [`TocHeader`] — `None` for any
/// comment line that doesn't match, which is the graceful-degradation case
/// the design already expects for header-less input: a line this doesn't
/// recognize just leaves [`Span::toc`] `None`, exactly as if there were no
/// TOC comment at all.
///
/// `trimmed` is expected to already have leading/trailing whitespace removed,
/// matching every other caller in this module.
///
/// **Handles both `-- Name: ...` and `-- Data for Name: ...` with one code
/// path** — `TOC_PREFIX_DATA` ("Data for ") is optional, and whether or not
/// it's present, what follows must still be `Name: `. The third prefix
/// `_printTocEntry()` can write, `TOC_PREFIX_STATS` ("Statistics for ", a
/// `pg_dump` 18+ `--statistics` component — real flag name; the register
/// entry that first named it said `--with-statistics`, which does not exist,
/// see I18), is left unrecognized: fixture evidence now exists
/// (`fixtures/18/objects/stats.sql`, slice 3.1.1), but recognizing it is a
/// deliberate deferral, not a gap — an entry using it just degrades
/// gracefully (span still tiles, this comment isn't read as a header) the
/// same way any other unhandled shape does.
///
/// **Splits on the field markers in the order `_printTocEntry()` writes
/// them**, not on a fully general grammar — `sanitize_line` only strips
/// newlines, never escapes a literal `; Type: ` (etc.) that might occur
/// inside an object's own name, so a pathological name could in principle
/// mis-split. I3's own caveat already treats the TOC comment as a hint, never
/// a correctness guarantee, and this parser only ever feeds enrichment
/// (`Span::toc`), never a span boundary — see "Span boundaries" above, whose
/// boundary detection (`looks_like_toc_name_line`) doesn't call this at all.
fn parse_toc_header_line(trimmed: &str) -> Option<TocHeader> {
    let rest = trimmed.strip_prefix("-- ")?;
    let rest = rest.strip_prefix("Data for ").unwrap_or(rest);
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

/// A span's stored bytes. Capped at [`TEXT_CAP`]: one pathological function
/// body must not make the cache unbounded, and the offsets are kept
/// regardless, so a caller that needs the rest can always read the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanText {
    /// Lossy UTF-8 of the span's bytes — truncated to the cap, if it hit one.
    pub text: String,
    /// Whether bytes were dropped to fit the cap.
    pub truncated: bool,
}

/// Per-span cap on stored text (`roadmap-phase3-object-inventory.md`, "Cache:
/// the dump file's identity is checked, not assumed"). A whole schema's DDL
/// is small — koji's entire surface is 154KB — so this only ever bites on a
/// single enormous statement.
pub const TEXT_CAP: usize = 64 * 1024;

/// Whether a span of this kind stores its text at all. `Data` spans are
/// excluded by the design ("`Data` spans never store text"); `Unscanned`
/// covers bytes by definition unread, so there is nothing to slice.
fn stores_text(body: &SpanBody) -> bool {
    !matches!(body, SpanBody::Data(_) | SpanBody::Unscanned)
}

/// Fill in [`Span::text`] for every span that stores it, reading the bytes
/// back from `source` by offset.
///
/// **Sliced from the file, never accumulated from [`crate::scan::Event::Line`]**
/// — `crate::scan` emits no line at all for anything inside, entering or
/// leaving a dollar-quoted string, so accumulated text would be missing every
/// function body in the file. Slicing makes text a pure function of a span's
/// boundaries, which is the same property the tiling invariant wants.
///
/// Runs as a pass over finished spans rather than at the moment each span
/// closes, because only the caller owns the source. It costs far less than
/// one read per span: spans tile, and the spans that store text are exactly
/// the ones between `Data` blocks, so **contiguous runs are coalesced into a
/// single `read_range`** — in a real dump that is one read per gap between
/// data blocks, over schema-sized regions the scan just walked.
pub async fn attach_text<S: ByteRangeSource>(source: &S, spans: &mut [Span]) -> Result<()> {
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
        // A run capped per span still reads only what it can store, so a
        // multi-gigabyte `Unparsed` region is never pulled into memory whole.
        let want = (run_end - run_start).min(((j - i + 1) * TEXT_CAP) as u64) as usize;
        let bytes = source.read_range(run_start, want).await?;
        for span in &mut spans[i..=j] {
            let from = (span.start - run_start) as usize;
            let to = ((span.end - run_start) as usize).min(bytes.len());
            let slice = if from < to { &bytes[from..to] } else { &[][..] };
            let truncated = slice.len() < (span.end - span.start) as usize
                || (span.end - span.start) as usize > TEXT_CAP;
            let slice = &slice[..slice.len().min(TEXT_CAP)];
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
/// `Data` span instead of one `Unparsed` span per statement
/// (`roadmap-phase3-object-inventory.md`, "Bulk regions": "a koji-scale
/// `--inserts` dump is ~1TB of `INSERT INTO` lines"). No inner offsets: unlike
/// a `CopyBlock`, nothing reads rows out of this yet — Phase 8 Track A adds
/// that reader, using the same quote-tracking [`Builder`] already does to
/// find the run's own boundaries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InsertRun {
    /// Same convention as [`CopyBlock::database`].
    pub database: Option<String>,
    /// The table every statement in this run targets, as its own first
    /// `INSERT INTO` line named it — schema-qualified when the line was,
    /// matching [`CopyHeader::qualified_name`](crate::copy::CopyHeader::qualified_name)'s
    /// convention for the same object.
    pub table: String,
    /// Number of `INSERT` statements folded into this span. Free to compute:
    /// finding the run's end already means recognizing each statement's own
    /// completion.
    pub row_count: u64,
}

/// The large-object data region (I12) — every `BEGIN;`/`COMMIT;`-wrapped
/// `lo_open`/`lowrite`/`lo_close` run in the file, merged into a single `Data`
/// span regardless of how many archive entries `pg_dump` split it across (one
/// on v13-16, one per object on v17+ — see [`Builder::on_large_object_start`]).
/// No identity at all: "the only identity the region carries is the OID in
/// each `lo_create('<oid>')` opener, and recovering it costs a walk of the
/// whole region, which is exactly what one span avoids paying"
/// (`roadmap-phase3-object-inventory.md`, "Bulk regions").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LargeObjectRegion {
    /// Same convention as [`CopyBlock::database`].
    pub database: Option<String>,
}

/// What a span is, at the granularity Phase 3.2's statement-driven pass (no
/// TOC enrichment) can tell — see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpanBody {
    Table {
        name: String,
        columns: Vec<(String, String)>,
    },
    TypeDef {
        name: String,
        kind: crate::preamble::TypeKind,
    },
    Extension {
        name: String,
        schema: Option<String>,
    },
    /// A bulk region — a `COPY` block, an `INSERT` run, or the large-object
    /// data region; see `docs/design/roadmap-phase3-object-inventory.md`,
    /// "Bulk regions". One span kind for all three, per that section's
    /// "Treating all three as one kind" — [`DataBlock`] is where they stop
    /// sharing a shape: only [`DataBlock::Copy`] carries the inner offsets a
    /// row reader seeks by, since it is the only one of the three a reader
    /// exists for yet (Phase 8, unscheduled, adds one for `INSERT` runs).
    Data(DataBlock),
    /// A `\connect <name>` meta-command — kept distinct from [`Framing`](SpanBody::Framing)
    /// because [`crate::preamble::dump_metadata_from_spans`] (Phase 3.2.1.1)
    /// needs the database name itself, not just "this was framing", to
    /// reconstruct `DumpMetadata`'s per-database segmenting.
    Connect {
        database: String,
    },
    /// The dump's (or a `\connect`ed database's) own two-line version-header
    /// comment block (I9): `-- Dumped from database version ...` / `-- Dumped
    /// by pg_dump version ...`. Distinct from [`Framing`](SpanBody::Framing)
    /// for the same reason as [`Connect`](SpanBody::Connect) — the derived
    /// view needs the actual version strings, not just "this was framing".
    VersionHeader {
        server_version: Option<String>,
        pg_dump_version: Option<String>,
    },
    /// A `--binary-upgrade` dump's `ALTER TYPE <name> ADD VALUE '<label>'
    /// ...;` (I6) — recognized so [`crate::preamble::dump_metadata_from_spans`]
    /// can fold the label back into the [`TypeDef`](SpanBody::TypeDef) span
    /// it targets.
    AlterTypeAddValue {
        type_name: String,
        label: String,
    },
    /// File prologue/epilogue framing (the `PostgreSQL database dump`
    /// banner, `\restrict`/`\unrestrict`, the `SET`/`set_config` preamble
    /// block, ...) — never a real database object, and not one of this
    /// module's other, more specific framing-adjacent kinds.
    Framing,
    /// A recognized statement (has a trailing `;`, parens/quotes balanced)
    /// that isn't one of this slice's three classified shapes — including
    /// every object kind TOC enrichment would otherwise label, and any
    /// statement (e.g. a trailing `ALTER ... OWNER TO`) not grouped into
    /// its owning entry's span, per the module docs' "no grouping" note.
    Unparsed,
    /// Bytes no scan has walked yet — always a single trailing span, since
    /// `build_map` always scans to its target's end
    /// (`docs/design/roadmap-phase3-object-inventory.md`, "Scan coverage is
    /// a prefix, expressed as a span"). Not produced by [`build_map`] today
    /// (which always scans to EOF); Phase 3.2.1 wires the incremental scan
    /// that emits one.
    Unscanned,
}

/// What [`check_tiling`] found wrong, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
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
/// no gaps or overlaps, starting at 0 and ending at `expected_end`
/// (`docs/design/roadmap-phase3-object-inventory.md`, "Tiling is verified at
/// runtime and reported as a diagnostic"). Returns every issue found, not
/// just the first — a caller still gets a usable (if incomplete) map either
/// way; per the design, a tiling failure is evidence of a bug in this
/// module, never a reason to refuse the file.
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
    /// accumulate the two-line version-header block's fields (I9) as they're
    /// seen, so the block can close as a [`SpanBody::VersionHeader`] instead
    /// of generic [`SpanBody::Framing`] when it's neither a TOC entry nor
    /// ordinary framing prose. `toc` is set the moment a line parses via
    /// [`parse_toc_header_line`] — independent of `saw_name`, since a
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
    /// [`Builder::governing_toc`], see "Span boundaries: statement-anchored,
    /// object-attributed, greedy" in `roadmap-phase3-object-inventory.md`) or
    /// right after a TOC comment block closes with `saw_name` true (`toc` is
    /// that comment's own header, `toc_owned` true) — either way `start` is
    /// the *span's* start, which for the TOC case is the comment block's
    /// start, not this statement's own first line.
    Statement { start: u64, buf: String, toc: Option<TocHeader>, toc_owned: bool },
    /// Accumulating a run of `INSERT INTO <table> ...;` statements for one
    /// table (`roadmap-phase3-object-inventory.md`, "Bulk regions") — entered
    /// from `Mode::Statement`'s first line instead of staying there, so the
    /// whole run becomes one `Data` span rather than one `Unparsed` span per
    /// statement. `start`/`toc`/`toc_owned` are the span's own, same
    /// convention as `Statement`. `table` is fixed at the run's first line; a
    /// later statement targeting a different table ends the run (real
    /// `pg_dump` output never does this — a TOC comment always separates two
    /// tables' data — but a header-less input isn't guaranteed to). `buf`
    /// accumulates the *current*, not-yet-complete statement only, empty
    /// between statements — that's the signal a fresh line either continues
    /// the run or ends it. `row_count` is complete statements folded in so
    /// far.
    InsertRun {
        start: u64,
        table: String,
        database: Option<String>,
        buf: String,
        row_count: u64,
        toc: Option<TocHeader>,
        toc_owned: bool,
    },
}

/// The statement-driven boundary/classification pass, shared by
/// [`build_map`] (a standalone, always-to-EOF scan) and
/// [`crate::index::build_index`]/[`crate::index::scan_preamble`], which drive
/// it directly so that producing spans costs no second pass over bytes
/// [`crate::scan::scan`] already walked
/// (`docs/design/roadmap-phase3-object-inventory.md`, "The map is the
/// structure, not a description of it").
pub(crate) struct Builder {
    mode: Mode,
    database: Option<String>,
    spans: Vec<Span>,
    /// The open `COPY` block, tracked separately from `mode` (which is
    /// always `Idle` while a `CopyStart`/`CopyEnd` pair is in flight —
    /// `crate::scan::CopyScanner` never interleaves the two). `.0` is the
    /// span's own start offset, which for a TOC-commented block precedes
    /// `.1`'s `header_offset` — see [`Builder::on_copy_start`]. `.2` is the
    /// partition-root marker this block's header carried, consumed at
    /// `CopyStart` so a later block cannot inherit it. `.3` is the TOC header
    /// the preceding `-- Data for Name: ...` comment (if any) parsed to.
    pending_data: Option<(u64, crate::scan::CopyStart, Option<String>, Option<TocHeader>)>,
    /// The `-- load via partition root <name>` marker (I2) seen since the
    /// last TOC entry began, waiting for the `COPY` header it belongs to.
    /// Cleared by the header that consumes it, and by the next TOC `Name:`
    /// line — an entry that turned out not to be table data at all leaves
    /// nothing behind for the following one to pick up.
    pending_partition_root: Option<String>,
    /// The in-progress merged large-object `Data` span, if a `BEGIN;` has
    /// been seen with no flush since — see [`on_large_object_start`](Self::on_large_object_start).
    /// `.0` is the span's own start (the first region's preceding TOC
    /// comment, if it had one, else its own `BEGIN;` line); `.1` is the
    /// offset just past the most recently closed `COMMIT;`, which becomes the
    /// span's `end` once nothing extends it further; `.2` is the first
    /// region's own TOC header, kept as the merged span's single
    /// representative `toc` (`Span::toc` holds one header; a v17+ run can
    /// carry several — see [`on_large_object_start`](Self::on_large_object_start)).
    pending_large_objects: Option<(u64, u64, Option<TocHeader>)>,
    /// Roles/tablespaces referenced anywhere fed to this builder so far —
    /// accumulated as spans close, per
    /// `docs/design/roadmap-phase3-object-inventory.md`'s "What a span
    /// carries" ("a modelled cross-reference set, accumulated during the
    /// scan"). See [`Builder::push_span`] (TOC `Owner:`/`Tablespace:`) and
    /// [`Builder::push_statement_span`] (`OWNER TO`/`GRANT`/`REVOKE`/`ALTER
    /// DEFAULT PRIVILEGES FOR ROLE`/`SET default_tablespace`).
    roles: BTreeSet<String>,
    tablespaces: BTreeSet<String>,
    /// The TOC entry a follow-on statement with no comment of its own would
    /// inherit — `roadmap-phase3-object-inventory.md`'s "Span boundaries:
    /// statement-anchored, object-attributed, greedy". Updated by every
    /// [`push_span`](Self::push_span) call: set to that span's own `toc` for
    /// a plain statement or `Data` span (whether freshly parsed or itself
    /// inherited — either way it's what the *next* follow-on should carry),
    /// cleared to `None` for `Framing`/`Connect`/`VersionHeader`, which the
    /// design says inheritance never crosses. Reset fresh by every new
    /// `Builder` (including a live segment's — `crate::stream`'s mapping pass
    /// always resumes exactly at a `Data` span's own boundary in practice, so
    /// nothing real depends on this carrying across builder instances).
    governing_toc: Option<TocHeader>,
}

/// Whether `-- Name: ...` (pg_dump's `_printTocEntry()` header, I3) is
/// present in `line` — the lexical, field-blind boundary signal the module
/// docs describe. Deliberately not full TOC parsing: it never reads
/// `Type:`/`Schema:`/`Owner:`, only confirms this comment block is a real
/// TOC entry rather than framing prose.
fn looks_like_toc_name_line(line: &str) -> bool {
    line.starts_with("-- Name: ") && line.contains("; Type: ")
}

/// The root table named by a `-- load via partition root <name>` marker
/// line (I2), if `line` is one. `pg_dump` writes it into the `TABLE DATA`
/// entry's `defn` whenever that entry's `COPY` header names the partition's
/// **root** rather than the partition itself — which is the only shape in
/// which one header name owns several blocks in one dump.
fn partition_root_marker(line: &str) -> Option<String> {
    let rest = line.strip_prefix("-- load via partition root ")?;
    let rest = rest.trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// The table an `INSERT INTO <table> ...` line targets, if `line` is one —
/// `pg_dump`'s `dumpTableData_insert()` always emits exactly this fixed
/// casing (`--inserts`/`--column-inserts`, with or without a column list
/// after the table name; both parse the same, since only the identifier
/// right after `INSERT INTO ` is read). `None` for anything else, including a
/// line that merely starts with this text as multi-line *string content* —
/// guarded the same way [`looks_like_toc_name_line`]/[`partition_root_marker`]
/// are, by the caller only ever checking this on a fresh statement's first
/// line (`Mode::Statement`'s `buf.is_empty()` gate, `Mode::InsertRun`'s own).
fn parse_insert_target(line: &str) -> Option<String> {
    let rest = line.strip_prefix("INSERT INTO ")?;
    crate::preamble::parse_qualified_name(rest).map(|(name, _consumed)| name)
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
    /// [`crate::stream::table_stream`]'s mapping pass needs, since it starts
    /// at the map's frontier rather than at byte 0 and may be well inside an
    /// already-`\connect`ed database's territory. Feeding it the file's
    /// earlier `\connect` lines is not an option: it never reads those bytes.
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
        }
    }

    /// Push a newly-completed span, and — since the tiling invariant makes a
    /// span's true end exactly the next span's start — fix up the
    /// previously-pushed span's placeholder `end` at the same time. Only the
    /// span still open when this call returns (`self.spans.last()`) carries
    /// a not-yet-real `end`; [`finish`](Self::finish)/[`snapshot`](Self::snapshot)
    /// are what close that one out, since nothing later has opened yet to
    /// fix it up.
    ///
    /// A builder that starts partway through a file therefore opens its first
    /// span at its first recognized content, not at the byte it began
    /// reading; closing that seam is the caller's job, and
    /// `crate::stream::splice` does it by extending the span before it — the
    /// same rule this method applies to every other boundary.
    ///
    /// `toc_owned` is `true` iff *this span's own* preceding comment carried
    /// the header text `toc` came from — `false` for a follow-on statement
    /// inheriting [`governing_toc`](Self::governing_toc). Every call site
    /// but [`push_statement_span`](Self::push_statement_span)'s inherited
    /// path passes `toc.is_some()`, since nothing else in this module ever
    /// carries a `toc` it didn't just parse from its own comment.
    ///
    /// Also where [`governing_toc`](Self::governing_toc) itself updates —
    /// centralized here, alongside the cross-reference bookkeeping below,
    /// because every span this module ever produces passes through this one
    /// method.
    fn push_span(&mut self, start: u64, body: SpanBody, toc: Option<TocHeader>, toc_owned: bool) {
        // Any new span — this one included, unless it *is* the flush itself
        // (which `flush_large_objects` already took `pending_large_objects`
        // out of `self` before calling back in here) — means the pending
        // large-object region isn't being extended, so it closes now.
        self.flush_large_objects();
        if let Some(t) = &toc {
            // The TOC comment's own `Owner:`/`Tablespace:` fields — one of
            // this span's two cross-reference sources regardless of its
            // kind, since `_printTocEntry()` writes them ahead of every
            // entry, not just the ones this module classifies
            // (`roadmap-phase3-object-inventory.md`, "What a span carries").
            if let Some(owner) = &t.owner {
                crate::preamble::insert_role(&mut self.roles, owner.clone());
            }
            if let Some(tablespace) = &t.tablespace {
                crate::preamble::insert_tablespace(&mut self.tablespaces, tablespace.clone());
            }
        }
        // `Framing`/`Connect`/`VersionHeader` are the three kinds inheritance
        // never crosses (`roadmap-phase3-object-inventory.md`, "Span
        // boundaries"); everything else becomes the entry a following
        // comment-less statement would inherit, whether this span's own
        // `toc` was freshly parsed or itself inherited.
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

    /// Classify a complete statement into a span, the way every
    /// `classify(buf)` call site below does — but first scan `buf` itself
    /// for the cross-references [`Span::toc`] can't cover: `OWNER TO`,
    /// `GRANT`/`REVOKE`/`ALTER DEFAULT PRIVILEGES FOR ROLE`, and `SET
    /// default_tablespace` (`crate::preamble::extract_statement_cross_refs`).
    /// Centralized here — rather than at each call site — so every statement
    /// this module ever classifies is scanned exactly once, the same way
    /// [`push_span`](Self::push_span) centralizes the TOC-sourced half.
    ///
    /// **A statement that classifies as `Framing`** (a mid-file `SET
    /// default_tablespace = ...;`/`SET ...;` — [`looks_like_framing_statement`])
    /// **never inherits**, even though `toc`/`toc_owned` may have arrived here
    /// carrying an inherited value: `classify` is what decides the span's
    /// final kind, and that decision has to happen before inheritance can be
    /// vetoed, which is why the override lives here rather than at the
    /// `Mode::Idle`→`Statement` transition that seeds it.
    fn push_statement_span(
        &mut self,
        start: u64,
        buf: &str,
        toc: Option<TocHeader>,
        toc_owned: bool,
    ) {
        crate::preamble::extract_statement_cross_refs(buf, &mut self.roles, &mut self.tablespaces);
        let body = classify(buf);
        let (toc, toc_owned) =
            if matches!(body, SpanBody::Framing) { (None, false) } else { (toc, toc_owned) };
        self.push_span(start, body, toc, toc_owned);
    }

    /// The roles/tablespaces referenced so far — see the [`roles`](Self::roles)
    /// field's docs. Read before [`finish`](Self::finish) consumes the
    /// builder (or any time, for [`snapshot`](Self::snapshot)'s non-consuming
    /// caller).
    pub(crate) fn roles(&self) -> &BTreeSet<String> {
        &self.roles
    }

    pub(crate) fn tablespaces(&self) -> &BTreeSet<String> {
        &self.tablespaces
    }

    /// Close whatever's pending at end of scan. `on_copy_start` handles the
    /// analogous mid-scan case (a `CopyStart` interrupting something in
    /// flight) itself, since it needs the interrupted span's start offset
    /// to seed the `Data` span that follows.
    ///
    /// `end` is the stop point [`finish`](Self::finish) is closing out at —
    /// needed here (not just applied afterward) because
    /// [`crate::index::scan_preamble`] can retreat that stop point to a
    /// pending comment's own `start` (via [`pending_comment_start`](Self::pending_comment_start))
    /// rather than let it guess the comment's classification. A comment with
    /// `start >= end` is exactly that case — nothing about it was decided —
    /// so it is dropped rather than pushed: pushing it would create a
    /// zero-length span (`start == end`) and, being generic
    /// `Framing`/`Unparsed`, would be the *wrong* guess besides, since the
    /// whole reason to retreat is that a later, unfed-truncated scan is the
    /// one that can classify it correctly (e.g. into a `Data` span).
    fn flush_pending(&mut self, end: u64) {
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => {}
            Mode::Comment { start, saw_name, server_version, pg_dump_version, toc } => {
                if start < end {
                    // Only reachable for a comment block that runs to EOF
                    // (or is interrupted by a `CopyStart` with nothing
                    // pending-comment-aware about the stop) with no closing
                    // non-`--` line — never observed in a well-formed
                    // `pg_dump` file (every real TOC comment, and the
                    // version-header block, is followed by something else),
                    // but classifies the same way `step`'s own
                    // comment-close arm would have, had a closing line ever
                    // arrived.
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
        let text = String::from_utf8_lossy(raw).into_owned();
        // Tracked here rather than inside `step`'s mode machine because the
        // marker is separated from the `COPY` header it describes by a blank
        // line, which closes whatever comment block held it — so by the time
        // `on_copy_start` runs, no mode carries it any more. Recognized
        // before dispatch so it is seen wherever the line lands.
        let trimmed = text.trim();
        if looks_like_toc_name_line(trimmed) {
            self.pending_partition_root = None;
        } else if let Some(root) = partition_root_marker(trimmed) {
            self.pending_partition_root = Some(root);
        }
        let mut current = Some((offset, text));
        while let Some((offset, line)) = current.take() {
            current = self.step(offset, &line);
        }
    }

    /// Process one line under the current mode. Returns `Some` when the
    /// same line must be reprocessed under a mode this call just switched
    /// into.
    fn step(&mut self, offset: u64, line: &str) -> Option<(u64, String)> {
        let trimmed = line.trim();
        match &mut self.mode {
            Mode::Idle => {
                if trimmed.is_empty() {
                    return None;
                }
                if let Some(name) = parse_connect(line) {
                    self.database = Some(name.clone());
                    self.push_span(offset, SpanBody::Connect { database: name }, None, false);
                    return None;
                }
                if trimmed.starts_with('\\') {
                    // Any other psql meta-command (`\restrict`,
                    // `\unrestrict`, ...): a single complete line, never
                    // continued, never real SQL.
                    self.push_span(offset, SpanBody::Framing, None, false);
                    return None;
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
                    return None;
                }
                // No comment precedes this statement: it inherits whatever
                // entry is currently governing (`None` if none is), per
                // `roadmap-phase3-object-inventory.md`'s "Span boundaries" —
                // `push_statement_span` still vetoes this if the statement
                // turns out to classify as `Framing`.
                self.mode = Mode::Statement {
                    start: offset,
                    buf: String::new(),
                    toc: self.governing_toc.clone(),
                    toc_owned: false,
                };
                Some((offset, line.to_string()))
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
                    return None;
                }
                if trimmed.is_empty() {
                    // A blank line right after a comment block's closing
                    // `--` is genuinely ambiguous — `_printTocEntry()` always
                    // writes `--\n\n` (I3), whether a DDL statement or a
                    // `COPY` header follows. Absorbed without deciding either
                    // way: staying in `Mode::Comment` is what lets
                    // `on_copy_start`'s `Mode::Comment` arm still see this
                    // block (and its `toc`) when the very next thing is a
                    // `COPY` header — `crate::scan::CopyScanner` intercepts
                    // that line as `Event::CopyStart` and never routes it
                    // through `feed_line` at all, so this arm never even runs
                    // for the block-closing case; only a genuinely
                    // non-blank, non-`--` line (a DDL statement) ever reaches
                    // the close/transition logic below.
                    return None;
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
                } else {
                    let body = close_comment(false, server_version.take(), pg_dump_version.take());
                    let owned = toc.is_some();
                    self.push_span(start, body, toc, owned);
                    self.mode = Mode::Idle;
                }
                Some((offset, line.to_string()))
            }
            Mode::Statement { start, buf, toc, toc_owned } => {
                // A `--`-prefixed line reasserts a fresh boundary even
                // though `buf` never reached `statement_complete` — the
                // case a dollar-quoted body's invisible closing line
                // creates (see the module docs' "Why TOC-block boundaries"
                // section): nothing ever supplies the swallowed `;`, so
                // without this, every following object would be absorbed
                // into the same dangling statement forever. Guarded by
                // `in_open_quote` so a `--`-looking continuation line that's
                // really multi-line *string content* (a value spanning
                // physical lines) is never mistaken for one — `pg_dump`
                // never emits an inline comment inside this module's own
                // recognized statement shapes, so this is unambiguous for
                // every other case.
                if trimmed.starts_with("--") && !in_open_quote(buf) {
                    let start = *start;
                    let buf = std::mem::take(buf);
                    let toc = toc.take();
                    let toc_owned = *toc_owned;
                    self.mode = Mode::Idle;
                    self.push_statement_span(start, &buf, toc, toc_owned);
                    return Some((offset, line.to_string()));
                }
                // The run's first line, recognized before it ever becomes a
                // one-statement `Unparsed` span — `roadmap-phase3-object-inventory.md`,
                // "Bulk regions": grouping the whole run into one `Data` span
                // is what keeps a koji-scale `--inserts` dump from allocating
                // (and, pre-3.6, text-storing) one span per row.
                if buf.is_empty()
                    && let Some(table) = parse_insert_target(line)
                {
                    let start = *start;
                    let toc = toc.take();
                    let toc_owned = *toc_owned;
                    self.mode = Mode::InsertRun {
                        start,
                        table,
                        database: self.database.clone(),
                        buf: String::new(),
                        row_count: 0,
                        toc,
                        toc_owned,
                    };
                    return Some((offset, line.to_string()));
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
                None
            }
            Mode::InsertRun { start, table, database, buf, row_count, toc, toc_owned } => {
                // Closes the run in place — takes owned copies of everything
                // first (mirroring `Mode::Statement`'s dangling-close arm
                // above) so `self.mode = Mode::Idle` and the `self.push_span`
                // call inside `push_insert_run` don't overlap this arm's
                // borrow of `self.mode`.
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
                        return Some((offset, line.to_string()));
                    }};
                }
                if buf.is_empty() {
                    if trimmed.is_empty() {
                        // Absorbed the same way `Mode::Comment` absorbs a
                        // blank line between two entries — waiting to see
                        // whether the run continues or the next TOC comment
                        // (or EOF) closes it.
                        return None;
                    }
                    let continues = parse_insert_target(line).as_deref() == Some(table.as_str());
                    if !continues {
                        close_and_reprocess!();
                    }
                    // Falls through to accumulate this line as the run's next
                    // statement.
                }
                // Defensive dangling-close, mirroring `Mode::Statement`'s —
                // not expected in real `pg_dump` output (a `--` line always
                // arrives with `buf` empty, handled above), kept for the same
                // graceful-degradation reason.
                if trimmed.starts_with("--") && !in_open_quote(buf) {
                    close_and_reprocess!();
                }
                push_stmt_line(buf, line);
                if statement_complete(buf) {
                    *row_count += 1;
                    buf.clear();
                }
                None
            }
        }
    }

    /// A dollar-quoted region closed at `offset`
    /// ([`crate::scan::Event::DollarQuoteEnd`]). Whatever statement is in
    /// flight ends with it: `pg_dump` writes the statement's own terminating
    /// `;` on the closing line (`AS $$ … $$;`), and that line never reaches
    /// [`feed_line`](Self::feed_line), so nothing else will ever complete the
    /// statement.
    ///
    /// Without this, the first dollar-quoted body in a file with no TOC
    /// comments absorbs every statement after it into one span — measured, in
    /// `docs/status/history/2026-08-23.md`. Real `pg_dump` output is
    /// unaffected either way, because the next entry's `--` header already
    /// reasserts a boundary; this is what makes the "graceful degradation"
    /// claim true for a `pg_dump`-compatible dump from elsewhere.
    ///
    /// A producer that puts the `;` on a *later* line instead leaves that
    /// line as its own small span. Coarser, still tiling — the same trade the
    /// rest of the fallback makes.
    pub(crate) fn on_dollar_quote_end(&mut self, _offset: u64) {
        if let Mode::Statement { start, buf, toc, toc_owned } =
            std::mem::replace(&mut self.mode, Mode::Idle)
        {
            self.push_statement_span(start, &buf, toc, toc_owned);
        }
    }

    pub(crate) fn on_copy_start(&mut self, event: crate::scan::CopyStart) {
        // I12 puts the large-object region after every `COPY` block, so a
        // pending one here would mean malformed/non-`pg_dump` input — flush
        // it rather than silently absorbing whatever follows into it.
        self.flush_large_objects();
        let (start, toc) = match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => (event.header_offset, None),
            // A TOC comment (`-- Data for Name: ...; Type: TABLE DATA`, or,
            // rarely, none at all) directly precedes the header: absorb it
            // into the `Data` span's outer boundary per
            // `roadmap-phase3-object-inventory.md`'s "COPY blocks are the
            // one exception" — `span.start <= header_offset`.
            Mode::Comment { start, toc, .. } => (start, toc),
            // Never observed in a well-formed dump (a statement never
            // precedes a `COPY` header with no separating blank line/TOC
            // comment of its own), but every byte must land somewhere.
            Mode::Statement { start, buf, toc, toc_owned } => {
                self.push_statement_span(start, &buf, toc, toc_owned);
                (event.header_offset, None)
            }
            // Same reasoning: a `COPY` header never follows an `INSERT` run
            // in real `pg_dump` output (data format is dump-wide, not
            // per-table), but every byte must land somewhere.
            Mode::InsertRun { .. } => {
                self.close_insert_run();
                (event.header_offset, None)
            }
        };
        self.pending_data = Some((start, event, self.pending_partition_root.take(), toc));
    }

    pub(crate) fn on_copy_end(&mut self, end: crate::scan::CopyEnd) {
        // `on_copy_start` always runs first for a matching block
        // (`crate::scan::CopyScanner` never emits `CopyEnd` without a prior
        // `CopyStart`), so this is always `Some`.
        let Some((start, copy_start, partition_root, toc)) = self.pending_data.take() else {
            return;
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
            sparse_index: None,
            column_stats: None,
        };
        let owned = toc.is_some();
        self.push_span(start, SpanBody::Data(DataBlock::Copy(block)), toc, owned);
    }

    /// A `BEGIN;` line opened a large-object data region (I12) — see
    /// [`crate::scan::Event::LargeObjectStart`].
    ///
    /// **This is where v13-16's single archive entry and v17+'s
    /// one-per-object entries end up producing the same map.** Neither this
    /// method nor [`on_large_object_end`](Self::on_large_object_end) push a
    /// span directly: the region stays *pending* until
    /// [`flush_large_objects`](Self::flush_large_objects) closes it, which
    /// only happens when something else is about to open — a new statement,
    /// a `COPY` header, or end of scan. So as long as nothing but more
    /// `BEGIN;`/`COMMIT;` pairs (each with, at most, its own TOC comment)
    /// arrives in between, a v17+ file's several consecutive `BLOBS` entries
    /// merge into the exact same single span a v13-16 file's one entry
    /// already produces. I12 is what makes this sound without checking each
    /// entry's own TOC `Type:` field: the large-object data region is its own
    /// contiguous priority band, so nothing else — not a `COPY` block, not
    /// ordinary DDL — can appear between two of its entries in real `pg_dump`
    /// output; whatever *does* arrive in between (this module never assumes
    /// I12 holds) closes the region via `flush_large_objects` the normal way.
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
        // The TOC `Owner:`/`Tablespace:` fields feed the cross-reference set
        // regardless of whether this entry becomes its own span or merges
        // into an already-open one — the same rule `push_span` applies to
        // every other span's `toc`.
        if let Some(t) = &toc {
            if let Some(owner) = &t.owner {
                crate::preamble::insert_role(&mut self.roles, owner.clone());
            }
            if let Some(tablespace) = &t.tablespace {
                crate::preamble::insert_tablespace(&mut self.tablespaces, tablespace.clone());
            }
        }
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
    /// any *other* span implies this one isn't being extended further) and at
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
    /// however its caller found that out — [`step`](Self::step) itself
    /// (the run ends the ordinary way), [`flush_pending`](Self::flush_pending)
    /// (already holds the destructured `Mode::InsertRun` fields from its own
    /// `end`-of-scan match), or [`close_insert_run`](Self::close_insert_run)
    /// (a non-`push_span` entry point interrupts a still-open run).
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

    /// Close whatever [`Mode::InsertRun`] has accumulated so far into a real
    /// span — called from the non-`push_span` entry points that can
    /// interrupt a run ([`on_copy_start`](Self::on_copy_start),
    /// [`on_large_object_start`](Self::on_large_object_start)). Takes
    /// `self.mode` unconditionally — every caller has already matched it as
    /// `Mode::InsertRun`; a no-op otherwise.
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
        // A separate field from `mode` (see its docs), so `flush_pending`
        // doesn't already cover it — a file ending right after the
        // large-object region's last `COMMIT;` still needs this to close it.
        self.flush_large_objects();
        if let Some(last) = self.spans.last_mut() {
            last.end = end;
        }
        self.spans
    }

    /// The spans recognized so far, without consuming `self` — unlike
    /// [`finish`](Self::finish), which a caller can only call once, at true
    /// end of scan. `end` closes out the still-open last span, the same way
    /// `finish`'s `end` does; the caller supplies it because this module
    /// only ever decides where the *next* span starts, never a span's own
    /// end (see the module docs).
    ///
    /// Only sound to call at a boundary where nothing is mid-classification
    /// — i.e. `self.mode` is [`Mode::Idle`] **and** no large-object region is
    /// pending — since otherwise the last-pushed span in `self.spans` is not
    /// actually the span open at `end`, it's the one before it, and stamping
    /// its `end` there would be wrong. Right after [`on_copy_end`](Self::on_copy_end)
    /// is exactly such a boundary (`on_copy_start` always leaves `mode`
    /// `Idle` for the block's duration, and I12 puts the large-object region
    /// strictly after every `COPY` block, so nothing pends one yet either),
    /// which is the caller this exists for — `crate::stream`'s mapping pass
    /// persists its progress after every completed block, not just once at
    /// the true end of its scan.
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
    /// `None` unless `self.mode` is [`Mode::Comment`]. For a caller that must
    /// stop scanning *before* the decision a `--`-prefixed run is waiting on
    /// (a statement, or a `COPY` header this builder is never fed —
    /// [`crate::index::scan_preamble`], the only such caller today), asking
    /// [`finish`](Self::finish)/[`snapshot`](Self::snapshot) to close out at
    /// the stopping point would swallow the still-open comment as a guessed
    /// [`SpanBody::Framing`]/[`SpanBody::Unparsed`] span — permanently
    /// wrong for a `-- Data for Name: ...` block, whose bytes belong to the
    /// `Data` span a later, unfed-truncated scan is the one that can still
    /// produce correctly. Retreating the stop point to this offset instead
    /// leaves the comment's bytes for that later scan to absorb from
    /// scratch.
    pub(crate) fn pending_comment_start(&self) -> Option<u64> {
        match &self.mode {
            Mode::Comment { start, .. } => Some(*start),
            _ => None,
        }
    }
}

/// A bare `SET ...;` or `SELECT pg_catalog.set_config(...);` — the two
/// statement shapes `_doSetFixedOutputState()` writes ahead of the archive
/// proper (`docs/design/roadmap-phase3-object-inventory.md`, "Framing
/// spans") and `_selectTablespace()` writes ahead of a definition — read for
/// its tablespace reference by `push_statement_span`'s
/// `crate::preamble::extract_statement_cross_refs` call regardless of how
/// this function classifies it. Neither is one of this module's three
/// classified shapes, and treating both uniformly as
/// framing (rather than `Unparsed`) matches the design doc regardless of
/// which of the two producers wrote a given occurrence — this slice does no
/// grouping, so a tablespace-setting `SET` ahead of an object still tiles
/// as its own adjacent span either way.
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
    if let Some(rest) = crate::preamble::strip_kw(stmt.trim_start(), "ALTER TYPE")
        && let Some((type_name, label)) = crate::preamble::parse_alter_type_add_value_body(rest)
    {
        return SpanBody::AlterTypeAddValue { type_name, label };
    }
    match classify_statement(stmt) {
        Some(StatementShape::Table { name, columns }) => SpanBody::Table { name, columns },
        Some(StatementShape::Type(TypeDef { name, kind })) => SpanBody::TypeDef { name, kind },
        Some(StatementShape::Extension(crate::preamble::Extension { name, schema })) => {
            SpanBody::Extension { name, schema }
        }
        None => SpanBody::Unparsed,
    }
}

/// Scan `source` end to end and build its full file map — see the module
/// docs for what this slice does and doesn't classify. Always scans to EOF;
/// there is no partial/incremental form until Phase 3.2.1, so
/// [`SpanBody::Unscanned`] never appears in the result.
pub async fn build_map<S: ByteRangeSource>(source: &S, options: &ScanOptions) -> Result<Vec<Span>> {
    let mut builder = Builder::new();

    scan(source, options, |event| {
        match event {
            Event::CopyStart(start) => builder.on_copy_start(start),
            Event::Row(_) => {}
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

    /// Feed `lines` (each with a synthetic offset — `\n`-joined, matching
    /// how a real scan would number them) through a fresh [`Builder`] and
    /// return the finished spans. Exercises the pure boundary/classification
    /// logic directly, without a real file or the async scanner.
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

    /// `-- Name: EXTENSION postgres_fdw; Type: COMMENT; Schema: -; Owner: `
    /// (a real fixture line, `fixtures/16/objects/verbose.sql`) — `Schema:
    /// -` and a trailing empty `Owner: ` both mean "none", per I16 and
    /// `_printTocEntry()`'s two ways of writing it.
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
    /// `on_copy_end` — the one span kind [`Builder::push_span`] isn't called
    /// for from inside [`Builder::step`] at all.
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
        builder.on_copy_start(crate::scan::CopyStart {
            header: crate::copy::CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        });
        let terminator_offset = header_offset + 48;
        let end_offset = terminator_offset + 3;
        builder.on_copy_end(crate::scan::CopyEnd { terminator_offset, end_offset, row_count: 1 });

        let spans = builder.finish(end_offset);
        assert_eq!(spans.len(), 1);
        assert!(matches!(spans[0].body, SpanBody::Data(_)));
        assert_eq!(spans[0].toc.as_ref().map(|t| t.kind.as_str()), Some("TABLE DATA"));
    }

    /// A statement with no preceding TOC comment at all — the header-less
    /// fallback — carries no `toc` and `toc_owned: false`, which is the
    /// graceful-degradation case the design expects rather than an error:
    /// there is no governing entry yet for it to inherit.
    #[test]
    fn a_statement_with_no_toc_comment_carries_no_toc_header() {
        let spans = spans_of(&["CREATE EXTENSION pgcrypto;"]);
        assert_eq!(spans[0].toc, None);
        assert!(!spans[0].toc_owned);
    }

    /// Slice 3.3.1's core behavior: a follow-on statement with no TOC comment
    /// of its own (`ALTER SCHEMA ... OWNER TO ...;`, mirroring
    /// `fixtures/*/objects/default.sql`) inherits the governing entry's
    /// header instead of carrying `None` — `roadmap-phase3-object-inventory.md`,
    /// "Span boundaries: statement-anchored, object-attributed, greedy". The
    /// two spans carry the *same* `toc` value, but only the first has
    /// `toc_owned: true`.
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
    /// `objects.simple_config` (several consecutive `ALTER TEXT SEARCH
    /// CONFIGURATION ... ADD MAPPING FOR ...;` statements after one TOC
    /// comment).
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
    /// (`looks_like_framing_statement`) and — per "Span boundaries" — never
    /// inherits, even when it directly follows a governed entry; and it
    /// clears the governing header for whatever comes after it, since
    /// `Framing` is one of the three kinds inheritance never crosses.
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

    /// A `COPY` block with no TOC comment of its own (the header-less-input
    /// fallback that `on_copy_start`'s `Mode::Idle` arm handles) resets
    /// inheritance the same way `Framing`/`Connect` do: the entry governing
    /// *before* the block does not leak past it, even though the `Data` span
    /// itself ends up with `toc: None` too.
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
        builder.on_copy_start(crate::scan::CopyStart {
            header: crate::copy::CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        });
        let terminator_offset = header_offset + 48;
        let end_offset = terminator_offset + 3;
        builder.on_copy_end(crate::scan::CopyEnd { terminator_offset, end_offset, row_count: 1 });
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

    /// The case the module docs' "Why TOC-block boundaries" section exists
    /// for: a dollar-quoted function body swallows its own closing `;`
    /// entirely (never reaches `Event::Line`), so only the next TOC
    /// comment's boundary — not statement completion — can close the span.
    /// `feed_line` here is only ever given the lines a real scan would still
    /// emit as `Event::Line` (the dollar-quoted body itself is never among
    /// them), matching `crate::scan::CopyScanner`'s actual behavior.
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

    /// I9's two-line version-header block (`docs/design/roadmap-phase3.2.1-span-wiring-notes.md`,
    /// Phase 3.2.1.1) gets its own span kind rather than generic `Framing`,
    /// so `crate::preamble::dump_metadata_from_spans` can recover the
    /// strings without re-reading the file.
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

    /// The case [`Builder::snapshot`] exists for: a caller (`stream.rs`'s
    /// live segment, in the follow-up slice that wires this in) needs a
    /// tiling span list *before* the scan reaches its true end, right after
    /// each `CopyEnd` — the one point `on_copy_end` guarantees `mode` is
    /// back to `Idle`. Drives the same `Builder` a real live segment would:
    /// two DDL statements, a `COPY` block, then a trailing DDL statement,
    /// checking `check_tiling` at every such boundary rather than only at
    /// the very end.
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
        builder.on_copy_start(crate::scan::CopyStart {
            header: crate::copy::CopyHeader {
                schema: Some("public".to_string()),
                table: "t".to_string(),
                columns: vec!["id".to_string()],
            },
            header_offset,
            data_offset: header_offset + 32,
        });
        let terminator_offset = header_offset + 48;
        let end_offset = terminator_offset + 3;
        builder.on_copy_end(crate::scan::CopyEnd { terminator_offset, end_offset, row_count: 1 });
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
    /// more will ever be fed, must agree — `finish` just also takes
    /// ownership, which a caller building an intermediate checkpoint (rather
    /// than actually finishing the scan) can't afford to do.
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
