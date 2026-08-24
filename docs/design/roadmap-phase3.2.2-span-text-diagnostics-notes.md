# Phase 3.2.2 — span text and the diagnostic channel: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
"Span text comes from the file, not from the parser", "Diagnostics: a
file-level channel on `DumpIndex`" and "Tiling is verified at runtime and
reported as a diagnostic". Per `CLAUDE.md`, consolidated into the phase-level
notes doc (and removed) once all of Phase 3 lands.

## Span text

`Span::text: Option<SpanText>` (`{ text: String, truncated: bool }`), filled
by `map::attach_text(source, &mut spans)` and persisted (cache format v4→v5).

**A pass over finished spans, not a slice taken as each span closes.** The
spec's wording suggests the latter, and its rationale — "the bytes are in the
scan buffer already" — is true, but only the *driver* owns the buffer while
only the `Builder` knows where a span started, and a span can be the whole
file (`--inserts`). Doing it at close time therefore means feeding a
retention watermark from the builder back into `scan.rs`'s buffer management,
in two drivers. The pass gets the identical result — text is a pure function
of the span's offsets, which is the property the decision actually wants —
with no coupling to buffer lifetime.

**It is not one read per span.** Spans tile, and the spans that store text are
exactly the ones *between* `Data` blocks, so `attach_text` coalesces each
contiguous run into a single `read_range`: one read per gap between data
blocks, over schema-sized regions the scan just walked (and which are still in
page cache on a fresh scan). The read is capped at `spans_in_run * TEXT_CAP`
so a multi-gigabyte `Unparsed` region is never pulled into memory whole.

`Data` spans store no text (the design says so — their bytes are unbounded).
Neither does `Unscanned`, which is new here and follows from what the variant
means: those bytes are by definition unread.

**`build_map` attaches text too**, not just `build_index`. They are the same
map, and `build_index_spans_match_build_map_exactly` holds them to that.

**The mapping pass re-slices wholesale at every checkpoint** rather than
incrementally. That is deliberate and correct, not laziness: the last span's
`end` grows as the scan advances, so its text has to be re-taken anyway. The
cost is one DDL-sized coalesced read per completed block — on koji, roughly
200 × 154KB against an hour-long scan.

## Diagnostics

New L1 module `diagnostic.rs` (`layering.md`'s table and Arrow-free grep
updated in the same change). `DumpIndex::diagnostics: Vec<Diagnostic>`,
`#[serde(skip)]`. Two producers today: `TilingBroken` from `check_tiling`, and
`CacheMtimeChanged` from `CacheMode::load`.

**The spec asked for one enum; it got one *vocabulary*, and the difference is
layering.** "`Diagnostic` gains a severity and a kind, so Phase 2's per-column
resolution outcomes, a cache-identity warning, the TOC-coverage figure and an
unrecognized span all speak one vocabulary" cannot be taken literally:
`DumpIndex` is L1, `resolve::ColumnResolution` is an L2 conclusion about
PostgreSQL type semantics, and a `DiagnosticKind` variant carrying one would
make L1 name an L2 type — [`layering.md`](layering.md) rule 1, and against
L1's whole "parses a declared type as an opaque string and never interprets
it" premise.

So `Severity` and the `{severity, kind}` shape are the shared vocabulary, in
L1; `resolve::Diagnostic` keeps its structured per-column payload at L2 and
gains the same `Severity` (`Mapped` → `Info`, anything falling back to
`Utf8View` → `Warning`). A caller draining both channels filters on one scale.
Flagged in `STATUS.md`'s "Decisions worth another look" as a departure from
the spec sentence.

**Not persisting is load-bearing, and the test says why.** A stored
`CacheMtimeChanged` would replay a warning about a check *this* run performed
successfully — the mismatch is between the cache and the current observation,
so it has to be recomputed on load. `diagnostics_do_not_round_trip_through_the_cache`
pins that by writing one into an index before saving and asserting it is gone
after loading.

## What the next slice inherits

- `check_tiling` has production callers now, so a map with a hole surfaces as
  a high-severity diagnostic rather than only as a test failure. Anything that
  adds a span kind gets that check for free.
- Nothing *reads* `diagnostics` yet — `pgdq info` surfacing them is 3.5. The
  channel exists so 3.3's TOC-coverage figure has somewhere to go without
  another design round.
- `Severity` is `Ord` (`Info < Warning < Error`), so a future sink can filter
  by threshold without a match.
