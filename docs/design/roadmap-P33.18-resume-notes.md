# P33.18 notes — the input walks resume

**Every reader a strict check reads a field by is a push machine**, fed the
field's bytes a run at a time and looking at no byte past the one it is fed
([`decisions.md`](decisions.md), "D144"), the form `resumable-reader-micro`
chose ([`roadmap-P33.17-form-notes.md`](roadmap-P33.17-form-notes.md)). No
reading was taken. What proves it is
`crates/pgdt-query/tests/decoded_copies.rs`'s
`a_strict_check_fed_over_runs_holds_no_field` — eleven fields of eight MiB,
arrays, ranges, `json`, `jsonb`, `bytea` and a long text under its typmod,
two of them refused, fed as `COPY` writes them in runs of 64 KiB, each run
made and dropped, the peak under four runs — which fails on its parent, whose
walk has no entry a run is fed to: it pulls a cloneable iterator, so a caller
holding a field's runs only in turn would have to hold the field.

- **`Unescape`** (`pgdt-core`'s `copy.rs`): `COPY`'s escapes over runs, an
  escape begun at a run's end carried; a plain stretch handed on as a slice
  of its run. `Unescaped`, the iterator a held field is read by, reads each
  escape by the same `escape_at`, so the escapes are read in one place.
- **What a grammar is fed** (`pgdt-codec`'s `value/source.rs`): `Parts`, what
  a container's walk hands its caller of each part; `Word`, a keyword's
  letters.
- **The input walks** (`value/nested/input/`): `ArrayWalk`, `RecordWalk`,
  `RangeWalk`, `MultirangeWalk` and `Int2VectorWalk`, each a state machine
  fed a byte at a time; `quoted.rs` the lexing a field and a bound share;
  `parse_*` collect them. `input/differential.rs` still holds every
  `parse_*` to the slice scanners in `prior_shape.rs`.
- **`JsonWalk`** (`value/json/walk.rs`), the open containers a bit each, and
  **`ByteaFold`** (`value/bytea.rs`), one reading answering whether
  `decode_bytea` reads a text and whether `byteain` refuses it, which
  `bytea_unread`, `read_bytea` and `bytea_escape_head` are folds of.
- **`FieldWalk`** (`grammar/walk.rs`): a field's escaped or unescaped runs
  through `Unescape`, a `TextFold` for its UTF-8 and the `Check` of its
  plan, a container's part read by its position's `Check`, reset per part.
  `walks` now names a `bytea` and a text with a typmod too, so a strict
  parse's `check_row` reads every field it walks one way. `grammar_refuses`,
  `checked_key` and `checked_range` moved beside it, `check.rs` keeping what
  a column is checked by and what is unchecked, so the two run one way.
- **What proves a run's end changes no answer**: `check/tests.rs`'s
  `walked_in_runs`, through which every walked field in
  `a_walked_check_refuses_what_the_keys_refused` and
  `a_walked_scalar_is_refused_as_its_text_is` is fed whole, in runs of one
  to three bytes and cut in two at every byte; `copy.rs`'s
  `unescape_over_runs_reads_what_unescape_field_returns` the same for the
  escapes.

## What 33.13.3 inherits

- **A scalar part is still collected** (`Check::Scalar`, `Check::Grammar`)
  and a range's bounds (`RangeWalk`), each a `Vec` reused across parts; the
  scalar readers are what resume on `Parts`' bytes next.

## What 33.19 inherits

- **`FieldWalk` is the strict check fed a field's runs** as `COPY` wrote
  them, but the `\N` marker: its caller says a field is NULL, which across
  runs is a two-byte look.
- **A `FieldWalk` is built per field**, a nested plan's `Check` boxed per
  container; one kept per column and readied per field is a `start` away.
- **The gatherer still reads a long field off `Unescaped`**, held whole
  (`gather/clip.rs`, `LongText`).

## Negative results

- **A run's end is invisible to a walk fed a byte at a time**, so only what
  takes runs — `Unescape`, `TextFold`, a leaf's `Check`, `FieldWalk` — is
  tested over cuts; the walks are tested whole.
- **No count pair was taken**: a strict parse's `bytea` loses the slice path
  `bytea_reads` read plain hex by, and P33's spec adopts no pricing of a
  hot-path change ([`decisions.md`](decisions.md), "D108").
- **`resumable-reader-micro` reads stale on this tree**: its bench's comments
  no longer call the pull walk today's, a hunk inside a comment, which
  `measure.py --stale` skips once committed.
