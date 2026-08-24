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
    pub body: SpanBody,
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
    /// ordinary framing prose.
    Comment {
        start: u64,
        saw_name: bool,
        server_version: Option<String>,
        pg_dump_version: Option<String>,
    },
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
    /// `.1`'s `header_offset` — see [`Builder::on_copy_start`]. `.2` is the
    /// partition-root marker this block's header carried, consumed at
    /// `CopyStart` so a later block cannot inherit it.
    pending_data: Option<(u64, crate::scan::CopyStart, Option<String>)>,
    /// The `-- load via partition root <name>` marker (I2) seen since the
    /// last TOC entry began, waiting for the `COPY` header it belongs to.
    /// Cleared by the header that consumes it, and by the next TOC `Name:`
    /// line — an entry that turned out not to be table data at all leaves
    /// nothing behind for the following one to pick up.
    pending_partition_root: Option<String>,
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
    fn push_span(&mut self, start: u64, body: SpanBody) {
        if let Some(last) = self.spans.last_mut() {
            last.end = start;
        }
        self.spans.push(Span {
            start,
            end: start,
            database: self.database.clone(),
            text: None,
            body,
        });
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
            // TOC comment, and the version-header block, is followed by
            // something else), but classifies the same way `step`'s own
            // comment-close arm would have, had a closing line ever arrived.
            Mode::Comment { start, saw_name, server_version, pg_dump_version } => {
                self.push_span(start, close_comment(saw_name, server_version, pg_dump_version));
            }
            Mode::Statement { start, buf } => self.push_span(start, classify(&buf)),
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
                    self.push_span(offset, SpanBody::Connect { database: name });
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
                    };
                    return None;
                }
                self.mode = Mode::Statement { start: offset, buf: String::new() };
                Some((offset, line.to_string()))
            }
            Mode::Comment { start, saw_name, server_version, pg_dump_version } => {
                if trimmed.starts_with("--") {
                    *saw_name |= looks_like_toc_name_line(trimmed);
                    match version_header_field(trimmed) {
                        Some((true, v)) => *server_version = Some(v),
                        Some((false, v)) => *pg_dump_version = Some(v),
                        None => {}
                    }
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
                    let body = close_comment(false, server_version.take(), pg_dump_version.take());
                    self.push_span(start, body);
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
        self.pending_data = Some((start, event, self.pending_partition_root.take()));
    }

    pub(crate) fn on_copy_end(&mut self, end: crate::scan::CopyEnd) {
        // `on_copy_start` always runs first for a matching block
        // (`crate::scan::CopyScanner` never emits `CopyEnd` without a prior
        // `CopyStart`), so this is always `Some`.
        let Some((start, copy_start, partition_root)) = self.pending_data.take() else { return };
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
        self.push_span(start, SpanBody::Data(block));
    }

    /// Finish the scan: whatever's still pending is closed out using `end`
    /// (the scan's own end offset) as the trigger that would otherwise have
    /// opened the next span.
    pub(crate) fn finish(mut self, end: u64) -> Vec<Span> {
        self.flush_pending();
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
    /// — i.e. `self.mode` is [`Mode::Idle`] — since otherwise the
    /// last-pushed span in `self.spans` is not actually the span open at
    /// `end`, it's the one before it, and stamping its `end` there would be
    /// wrong. Right after [`on_copy_end`](Self::on_copy_end) is exactly such
    /// a boundary (`on_copy_start` always leaves `mode` `Idle` for the
    /// block's duration), which is the caller this exists for —
    /// `crate::stream`'s mapping pass persists its progress after every
    /// completed block, not just once at the true end of its scan.
    pub(crate) fn snapshot(&self, end: u64) -> Vec<Span> {
        let mut spans = self.spans.clone();
        if let Some(last) = spans.last_mut() {
            last.end = end;
        }
        spans
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
