# P36.9.1 notes — the column record its finding

`pgdt_codec::ResolvedColumn` implements `pgdt_model::Finding`: it holds the
declared type and the resolution itself, and its message reads the name off
its field, so each is stored once. `ColumnNote`, `ResolvedColumn::note()` and
`ResolvedSchema::notes()` are gone; a caller drains `resolved.columns()` and a
sink downcasts to `ResolvedColumn`.

## The shape

- **Severity and message moved whole**, their rustdoc with them, onto
  `impl Finding for ResolvedColumn` in `resolve.rs`; the sentence is
  unchanged, so no test's expected message moved.
- **The provider reports the records**: `PgDumpTable::report_counting`
  wraps each `ResolvedColumn` in its `Located`, whose `as_any` is the
  record's, so `registration.rs`'s sink classes it as before.
- **`column_divergences` iterates the records**, reading `declared()` and
  `name()` where it read the note's fields.
- **The codec's severity tests build a record** through `column_of`, a text
  column declared nothing, where they built a bare note.

## The count: one fall, no rise

`measure.py --count-pair HEAD` on the uncommitted tree, its parent
`86bf88c1`, ran detached (`runs/count-pair-36.9.1-20261019/log`, its
`HANDOFF.md` beside it) and exited 0 after 25.5 minutes; no checkout file
changed after the launch, so the tree leg is this slice's code. Written
before it ran: what moves on a counted path is the record — one `String`
fewer, the name `resolve_columns` cloned into the note per column each time
it resolves a schema (per block where the data-level `parse` gathers, once a
table on a query), and the declared type and the resolution inline, which
shifts the offsets `render_field_into` and `decode_field` read the plan at,
per field; nothing per row reads the note; so no row resolves, or a row
resolves a fall, and a resolved rise is attributed exactly before it is
reworked or accepted (spec, "What a hot-path slice owes").

**That held.** `query --schema-mode typed` resolved a fall, -0.034%; the
other four rows are unresolved. No rise is resolved, so nothing is owed
the attribution, and 36.9.1 is ticked.

**The fall is unattributed.** The clone gone runs once a column on a
query, which cannot come to eleven million instructions, so it is not that.
That only `typed` moves points at the typed decode, but no reading has named
the machine code. Callgrind on the two legs would split it, with
`runs/attr-36.9-20261019-callgrind/`'s apparatus; the legs,
`runs/pgdt-count-pair-{parent,tree}`, are overwritten by the next pair, and
the spec owes an attribution only of a rise.

## What the next round inherits

- **The next counted slice is counted against its own parent**, which
  carries this slice.
- **A sink names `ResolvedColumn`** where it downcast to `ColumnNote`; 36.11's
  public `Located` and 36.12's embedder test build on it.
