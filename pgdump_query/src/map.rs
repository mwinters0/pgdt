//! The full file map: an ordered, tiling set of [`Span`]s covering every
//! byte of a `pg_dump` plain-format file
//! (`docs/design/roadmap-phase3-object-inventory.md`, "What this phase is
//! for" and "The map is the structure, not a description of it").
//!
//! [`build_map`] is Phase 3.2's statement-driven pass: it classifies DDL
//! statements directly, with **no TOC-comment enrichment** (owner, kind
//! label, `Tablespace:`, TOC-coverage — that's Phase 3.3). A TOC comment
//! block is still recognized here, but only as a **boundary** — see "Span
//! boundaries" below — never for the fields it carries.
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
//! "Dollar-quoted lines never reach `Event::Line`"). So a statement-only
//! completion tracker can never observe that such a statement closed at
//! all: nothing in the visible line stream says so. Every real `pg_dump`
//! entry — `CREATE FUNCTION` included — carries its own `-- Name: ...; Type:
//! ...` TOC header, and that header can *never* appear inside a
//! dollar-quoted body (the scanner already filters those lines out before
//! any event reaches this module, fake-looking `-- Name:` text included), so
//! recognizing the next entry's header is a sound, general "the previous
//! span has ended" signal that sidesteps the dollar-quote problem entirely.
//! The statement-grammar fallback (used for content with no TOC header —
//! `\connect`/`\restrict`/framing lines, and any statement, like a trailing
//! `ALTER ... OWNER TO`, that isn't its own TOC entry) is exactly
//! [`crate::preamble::statement_complete`], hardened in this same slice for
//! the double-quote/`--`-comment cases a five-keyword-triggered accumulator
//! never used to reach.
//!
//! **Consequence: no "grouping" in this slice.** `docs/design/roadmap-phase3-object-inventory.md`
//! lists grouping (folding an object's trailing `ALTER ... OWNER TO` into
//! the same TOC entry) as something the TOC layer *contributes* — "Without
//! it: They tile as two adjacent spans instead of one." That is exactly
//! this slice's behavior: a definition and its ungrouped trailing statement
//! become two spans, both correctly tiled, per the design doc's own
//! "graceful degradation" framing. Grouping (via the TOC's `Dependencies:`
//! field) is Phase 3.3/3.4 work.
//!
//! ## What is outside this slice, and which slice has it
//!
//! - **TOC enrichment** (owner, kind label, `Tablespace:`, TOC-coverage,
//!   grouping) — Phase 3.3/3.4.
//! - **A dedicated `Data`-span fast path for `INSERT` runs and the
//!   large-object (`BLOBS`/`BLOB METADATA`) region.** The design's "Bulk
//!   regions" section frames grouping either into one span as a
//!   *performance* optimization (avoiding a statement per row on a
//!   koji-scale `--inserts` dump or a multi-GB large object), not a
//!   correctness requirement. This slice's generic statement-grammar
//!   fallback already tiles both shapes correctly today — including the
//!   embedded-raw-newline `INSERT` case
//!   (`fixtures/*/edge_cases/inserts.sql`'s `escapes` row 10), since
//!   [`crate::preamble::statement_complete`] re-scans its whole buffer
//!   (parens/quotes included) on every appended line regardless of how many
//!   physical lines a value spans — it just does it one statement (or one
//!   `lowrite` call) per span rather than one span per whole run. Phase 3.6;
//!   see `docs/design/roadmap-phase3.2-span-model-notes.md` for the
//!   verification this rests on.
//! - **Wiring into `DumpIndex`/`crate::cache`/`crate::stream` — landed in
//!   Phase 3.2.1, with one gap left for a follow-up.** [`Builder`] (this
//!   module's boundary/classification state machine) is now driven directly
//!   by [`crate::index::build_index`] and [`crate::index::scan_preamble`],
//!   fed the same [`crate::scan::Event`] stream those functions already walk
//!   — [`build_map`] itself is now a thin wrapper over the same [`Builder`],
//!   kept for callers (and this module's own tests) that just want a span
//!   list. `DumpIndex::spans` is the primary, persisted structure;
//!   `DumpIndex::blocks()`/`blocks_for` are filters over it, and
//!   `crate::stream::table_stream`'s `Recorder` appends each live-discovered
//!   block as its own [`SpanBody::Data`]. [`SpanBody::Unscanned`] is produced
//!   for real by [`crate::index::preamble_only`], the one genuinely partial
//!   scan today (`build_index` always reaches EOF).
//!
//!   **Left for a follow-up** (`docs/design/roadmap-phase3.2.1-span-wiring-notes.md`):
//!   `DumpMetadata` is still populated by its own [`crate::preamble::PreambleBuilder`]
//!   pass run alongside [`Builder`], not yet a derived view over `spans` the
//!   way the design's "The span is the container" section calls for; and
//!   `crate::stream::table_stream`'s live segment records `Data` spans
//!   without classifying the DDL between them, so an index built by a query
//!   (as opposed to `build_index`'s full scan) does not tile the file the way
//!   `check_tiling` expects — an accepted gap since nothing yet reads
//!   non-`Data` spans back out of a query-built `DumpIndex`.
//! - **Span text storage** (`docs/design/roadmap-phase3-object-inventory.md`,
//!   "Span text comes from the file, not from the parser"), the cache's
//!   64KB-per-span cap, and the file-level `Diagnostic` channel
//!   [`check_tiling`] is meant to report through — Phase 3.2.2. [`Span`]
//!   carries offsets only, and no production path calls [`check_tiling`].

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
    pub body: SpanBody,
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
    /// A `COPY` data block — the one span kind with inner offsets; see
    /// `docs/design/roadmap-phase3-object-inventory.md`, "Bulk regions".
    Data(CopyBlock),
    /// File prologue/epilogue framing (the `PostgreSQL database dump`
    /// banner, `\restrict`/`\unrestrict`, version-header comments, the
    /// `SET`/`set_config` preamble block, ...) and psql meta-commands
    /// (`\connect`) — never a real database object.
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
    /// one matches `-- Name: ...; Type: ...`.
    Comment { start: u64, saw_name: bool },
    /// Absorbing a statement's lines via [`statement_complete`]. Started
    /// either directly (no TOC comment) or right after a TOC comment block
    /// closes with `saw_name` true — either way `start` is the *span's*
    /// start, which for the TOC case is the comment block's start, not this
    /// statement's own first line.
    Statement { start: u64, buf: String },
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
    /// `.1`'s `header_offset` — see [`Builder::on_copy_start`].
    pending_data: Option<(u64, crate::scan::CopyStart)>,
}

/// Whether `-- Name: ...` (pg_dump's `_printTocEntry()` header, I3) is
/// present in `line` — the lexical, field-blind boundary signal the module
/// docs describe. Deliberately not full TOC parsing: it never reads
/// `Type:`/`Schema:`/`Owner:`, only confirms this comment block is a real
/// TOC entry rather than framing prose.
fn looks_like_toc_name_line(line: &str) -> bool {
    line.starts_with("-- Name: ") && line.contains("; Type: ")
}

impl Builder {
    pub(crate) fn new() -> Self {
        Self { mode: Mode::Idle, database: None, spans: Vec::new(), pending_data: None }
    }

    fn push_span(&mut self, start: u64, body: SpanBody) {
        self.spans.push(Span { start, end: start, database: self.database.clone(), body });
    }

    /// Close whatever's pending at end of scan. `on_copy_start` handles the
    /// analogous mid-scan case (a `CopyStart` interrupting something in
    /// flight) itself, since it needs the interrupted span's start offset
    /// to seed the `Data` span that follows.
    fn flush_pending(&mut self) {
        match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => {}
            // Only reachable for a comment block that runs to EOF (or is
            // interrupted by a `CopyStart`) with no closing non-`--` line —
            // never observed in a well-formed `pg_dump` file (every real
            // TOC comment is followed by its object's definition), but a
            // `saw_name` one still classifies as `Unparsed` rather than
            // `Framing`, matching what `step`'s own comment-close arm would
            // have done had a closing line ever arrived.
            Mode::Comment { start, saw_name } => {
                self.push_span(
                    start,
                    if saw_name { SpanBody::Unparsed } else { SpanBody::Framing },
                );
            }
            Mode::Statement { start, buf } => self.push_span(start, classify(&buf)),
        }
    }

    /// Feed one outside-block line. Loops internally to let a
    /// comment-block-close or statement-complete transition re-dispatch the
    /// *same* line under the new mode without the caller needing to know.
    pub(crate) fn feed_line(&mut self, offset: u64, raw: &[u8]) {
        let text = String::from_utf8_lossy(raw).into_owned();
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
                    self.database = Some(name);
                    self.push_span(offset, SpanBody::Framing);
                    return None;
                }
                if trimmed.starts_with('\\') {
                    // Any other psql meta-command (`\restrict`,
                    // `\unrestrict`, ...): a single complete line, never
                    // continued, never real SQL.
                    self.push_span(offset, SpanBody::Framing);
                    return None;
                }
                if trimmed.starts_with("--") {
                    self.mode = Mode::Comment {
                        start: offset,
                        saw_name: looks_like_toc_name_line(trimmed),
                    };
                    return None;
                }
                self.mode = Mode::Statement { start: offset, buf: String::new() };
                Some((offset, line.to_string()))
            }
            Mode::Comment { start, saw_name } => {
                if trimmed.starts_with("--") {
                    *saw_name |= looks_like_toc_name_line(trimmed);
                    return None;
                }
                let start = *start;
                let saw_name = *saw_name;
                if saw_name {
                    // The comment block was a real TOC entry: the span
                    // continues into the statement it precedes, starting at
                    // the comment's own offset.
                    self.mode = Mode::Statement { start, buf: String::new() };
                } else {
                    self.push_span(start, SpanBody::Framing);
                    self.mode = Mode::Idle;
                }
                Some((offset, line.to_string()))
            }
            Mode::Statement { start, buf } => {
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
                    let body = classify(buf);
                    self.mode = Mode::Idle;
                    self.push_span(start, body);
                    return Some((offset, line.to_string()));
                }
                push_stmt_line(buf, line);
                if statement_complete(buf) {
                    let start = *start;
                    let body = classify(buf);
                    self.mode = Mode::Idle;
                    self.push_span(start, body);
                }
                None
            }
        }
    }

    pub(crate) fn on_copy_start(&mut self, event: crate::scan::CopyStart) {
        let start = match std::mem::replace(&mut self.mode, Mode::Idle) {
            Mode::Idle => event.header_offset,
            // A TOC comment (`-- Data for Name: ...; Type: TABLE DATA`, or,
            // rarely, none at all) directly precedes the header: absorb it
            // into the `Data` span's outer boundary per
            // `roadmap-phase3-object-inventory.md`'s "COPY blocks are the
            // one exception" — `span.start <= header_offset`.
            Mode::Comment { start, .. } => start,
            // Never observed in a well-formed dump (a statement never
            // precedes a `COPY` header with no separating blank line/TOC
            // comment of its own), but every byte must land somewhere.
            Mode::Statement { start, buf } => {
                self.push_span(start, classify(&buf));
                event.header_offset
            }
        };
        self.pending_data = Some((start, event));
    }

    pub(crate) fn on_copy_end(&mut self, end: crate::scan::CopyEnd) {
        // `on_copy_start` always runs first for a matching block
        // (`crate::scan::CopyScanner` never emits `CopyEnd` without a prior
        // `CopyStart`), so this is always `Some`.
        let Some((start, copy_start)) = self.pending_data.take() else { return };
        let block = CopyBlock {
            header: copy_start.header,
            database: self.database.clone(),
            header_offset: copy_start.header_offset,
            data_offset: copy_start.data_offset,
            terminator_offset: end.terminator_offset,
            end_offset: end.end_offset,
            row_count: end.row_count,
            sparse_index: None,
            column_stats: None,
        };
        self.push_span(start, SpanBody::Data(block));
    }

    /// Finish the scan: whatever's still pending is closed out using `end`
    /// (the scan's own end offset) as the trigger that would otherwise have
    /// opened the next span.
    pub(crate) fn finish(mut self, end: u64) -> Vec<Span> {
        self.flush_pending();
        let n = self.spans.len();
        for i in 0..n {
            self.spans[i].end = if i + 1 < n { self.spans[i + 1].start } else { end };
        }
        self.spans
    }
}

/// A bare `SET ...;` or `SELECT pg_catalog.set_config(...);` — the two
/// statement shapes `_doSetFixedOutputState()` writes ahead of the archive
/// proper (`docs/design/roadmap-phase3-object-inventory.md`, "Framing
/// spans") and `_selectTablespace()` writes ahead of a definition (the
/// `Tablespace:` cross-reference, not read until Phase 3.4). Neither is one
/// of this module's three classified shapes, and treating both uniformly as
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
        }
        ControlFlow::Continue(())
    })
    .await?;

    let end = source.size().await?;
    Ok(builder.finish(end))
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
    fn connect_opens_its_own_framing_span_and_switches_the_database() {
        let spans = spans_of(&["\\connect one", "CREATE EXTENSION pgcrypto;"]);
        assert_eq!(spans[0].body, SpanBody::Framing);
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
}
