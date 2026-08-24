# Phase 3.5 — CLI surface: how it landed

Companion to
[`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
slice table row for 3.5. Per `CLAUDE.md`, consolidated into the phase-level
notes doc (and removed) once all of Phase 3 lands.

## What landed

Everything is in `pgdump_query-cli/src/main.rs` — a pure presentation layer
over data every earlier slice already computed and tested. No library code
changed, and no new fixtures or library tests were needed; the whole slice is
CLI formatting plus the manual page.

**Role/tablespace/object-kind summaries print by default**, not gated behind
`--verbose`, matching the spec row's "by default" — `print_cross_references`
prints `DumpIndex::roles`/`tablespaces` (present since Phase 3.4) when
non-empty, and `print_object_kinds` counts every span's `Span::toc.kind`
(present since Phase 3.3) into a sorted `BTreeMap` and prints it as `object
kinds:` — the same closed vocabulary the TOC-coverage diagnostic already
counts against, just bucketed by name instead of summed. Both print nothing
for an empty result (a plain single-owner dump with no custom tablespaces
shows no `roles`/`tablespaces` line at all), and both return whether they
printed anything so `print_index` knows whether to add a separating blank
line before the table/span listing.

**`--map` lists every span** (`DumpIndex::spans`, not the `blocks()` filter)
instead of the per-table listing: `[start, end) <one-line label>`, in file
order, with the same database-header grouping convention the ordinary block
listing already uses. `span_summary` matches on `SpanBody`: `Data(Copy(_))`
reuses the existing `qualified_name`/`row_count` shape, `Data(InsertRun(_))`
and `Data(LargeObjects(_))` get their own short forms, `Table`/`TypeDef`/
`Extension`/`Connect`/`VersionHeader`/`AlterTypeAddValue`/`Framing` each get a
fixed label, and `Unparsed` reads `span.toc` when present (kind + name, the
same information `object kinds:` counted) falling back to a bare `unparsed`
for the header-less-input case `crate::map`'s own docs describe. `TypeKind`
gets a `type_kind_label` mapping to the six-shape vocabulary
`docs/manual/type-handling.md` already documents for readers, rather than
the full `Debug` derive.

**`--map` and `--preamble-only` are mutually exclusive**, rejected with a
plain `anyhow::bail!` before any scan runs — `--preamble-only` never reaches
a `DumpIndex` at all (it returns bare `DumpMetadata`, no `spans` to list), so
combining the two has no sane behavior to fall back to rather than an
outright conflict.

The final summary line under `--map` reports span count instead of block
count (`{spans} span(s), {bytes} bytes scanned`) — block/row counts stay
meaningless for spans that aren't `COPY` blocks, so this is a distinct line
rather than a variant of the existing one.

**`docs/manual/dump-inspection.md`** is the new manual page, covering the
default view, `--verbose`, `--map`, and `--preamble-only` in the same
practical, no-design-rationale style as `type-handling.md`. `README.md`'s
manual pointer lists both pages now.

## Verified by

Manual invocation against existing fixtures (no new fixture or test needed —
the data being formatted was already tested by the slices that produced it):
`fixtures/16/objects/default.sql` for the default view (roles, object kinds,
TOC-kind spans including `large objects`, `unparsed` spans for
un-TOC'd trailing statements) and `--map`'s output, and
`fixtures/16/edge_cases/inserts.sql` for `INSERT run:` span rendering. The
`--map`/`--preamble-only` conflict was exercised directly. `cargo
test --workspace`/`clippy`/`fmt --check` all pass unchanged — this slice adds
no library-level surface for them to exercise.

## Known gap, found incidentally

`DumpIndex::diagnostics` (tiling failures, the cache mtime warning, the
TOC-coverage figure) is fully populated by every scan but **the CLI never
prints any of it** — not a regression from this slice, a pre-existing gap
that writing `print_index` surfaced. `docs/design/roadmap-phase3-object-inventory.md`'s
own "Diagnostics" section defers the *sink abstraction* (a caller-supplied
drain) to Phase 6, but a plain "print what's in `index.diagnostics`" is
smaller than that and this slice's spec row didn't ask for it, so it's left
as a gap rather than added here — recorded in `STATUS.md`'s "Known gaps"
rather than silently fixed.

## What the next phase inherits

**Phase 3's slice checklist is now fully ticked.** What remains is the
process's own "Wrap the phase" step (`docs/process.md`): consolidate every
per-slice notes doc (3.1 through 3.6) into one
`roadmap-phase3-object-inventory-notes.md`, delete the per-slice files, and
rewrite `STATUS.md`'s Phase 3 section as the terse "complete" summary Phase 1
and 2 already carry. That did not happen in this slice — it's a separate,
mechanical-but-large pass better reviewed on its own rather than bundled with
this slice's CLI code, per the unattended-session rule against mixing
different kinds of work in one review cycle. A future session should do it
before specifying Phase 4, per step 6 ("Grill again").

`--map`'s per-`SpanBody`-variant label function
(`pgdump_query-cli::span_summary`) is the one place in the codebase that
already matches every `SpanBody`/`DataBlock` variant for display purposes —
a future CLI feature wanting the same labels (a `--filter-kind` on `--map`,
say) should extend it rather than duplicating the match.
