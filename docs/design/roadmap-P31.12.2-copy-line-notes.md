# P31.12.2 — A refused field's line numbered as the restore numbers it: notes

What the slices after this one inherit. The mechanism is 31.12's
([`roadmap-P31.12-field-refusal-notes.md`](roadmap-P31.12-field-refusal-notes.md));
the choice of numbering, and the dump file's own line number refused, are
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
refused field is located as a restore locates it".

## What exists

- **`Error::FieldRefused` carries `line`**, `COPY`'s count from the block's
  first data line, beside `line_offset`, and its message opens as the
  restore's context does: `COPY <table>, line N, column <c>: …`, the offset
  last. The table is schema-qualified where the server's context names the
  relation alone.
- **`FieldRefusal::line` is counted by the observer that met the field**:
  `Gatherer::rows` counts every row it is handed, after it stops too, and a
  block folding a piece adds the piece's rows to its own, folded or dropped,
  or adds its own to the piece's refusal. Pieces are folded in file order
  already, so nothing new orders them.
- **`Error::FieldDecode`'s field is `line_offset`**, renamed from
  `row_offset`, and its message is worded as `FieldRefused`'s, naming the
  line by its offset alone.
- **No persisted byte moved**: a refusal is never saved, so
  `CACHE_FORMAT_VERSION` stays 48.
- **Evidence**: `gather.rs`'s
  `a_refusal_is_numbered_by_its_row_in_the_block_however_it_was_cut` (a
  serial prefix and random windows of pieces) and
  `a_refusal_past_a_decline_counts_the_declined_rows`, both failing with the
  fold's count removed; `tests/statistics.rs`'s two parse-refusal tests at
  every worker count and on a back-fill; `tests/decode.rs`'s
  `every_refused_field_fails_a_data_level_parse_reading_it`, every major,
  and the `numeric(p,s)` test, each holding `line` to a count read off the
  file. The koji replica (PG16) answered a four-row `COPY` with `70000` in a
  `smallint` last, an escaped newline in an earlier row, as `COPY t, line 4,
  column a: "70000"`.

## What the slices after this inherit

- **31.14's `strict`** decodes fields no observer keys today — a declined
  block's, a nested leaf, a metadata-level column. Wherever it raises
  `FieldRefused` from, `line` needs the block's rows before the field;
  `Gatherer::rows` already counts past a decline, and a decoder outside the
  observer would need a count of its own.
- **P8's `INSERT` reading** has no `COPY` line to number; its inbox entry
  says so.
