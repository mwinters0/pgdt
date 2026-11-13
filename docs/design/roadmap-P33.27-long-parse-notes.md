# P33.27 notes — a long field's parse reworked

**Every rise 33.26 placed on a long field's parse is reworked, none
accepted ahead of its pair**: the readers it placed them in read a stretch
where they read a byte, and `Unescape` hands its readers a stretch where it
handed them an escape's byte alone
([`decisions.md`](decisions.md), "D156"). Nothing a reader answers moves;
each rework is held to the per-byte reading it replaces.

- **`Unescape`** (`pgdt-core`'s `copy.rs`): an escape read whole by
  `escape_at`'s lookahead over the bytes at hand, one begun at a run's end
  carried as its bytes (never all four, which say what any escape spells)
  and finished by `escape_cut`; the step machine and its `Escape` state are
  gone. `run` stages an escape's byte and the stretches beside it up to
  `STAGE`, handing a longer stretch on as a slice of its run. 33.19's fan-out
  per escape and `from_utf8` per run (`LongRead::text`, `FieldWalk::text`)
  are now per stage. `copy.rs`'s `unescape_over_every_cut_reads_what_unescaped_reads`
  holds the carried escape to the held reading over every three-run cut of
  every field of up to six bytes over `\x7f8a`;
  `unescape_hands_on_stretches_not_escapes` holds the staging.
- **`Unescaped`** reads a plain byte as itself and an escape by
  `escape_at`, as before 33.18, with no machine stepped per byte (33.18's
  rise); `unescape_owned` collects `Unescape::run`'s stretches.
- **`TextFold::fold`** (`pgdt-codec`'s `value/text.rs`) folds a stretch: its
  length added, its characters counted up to its last non-blank and its
  trailing blanks counted as they are (33.13's per-byte fold).
- **`ByteaFold::run`** (`value/bytea.rs`): the hex form's digits read as a
  stretch, a pair at a time where it decodes, any other byte by `byte`;
  `ByteaRuns::push` and a strict check's `Check::run` read a `bytea` by it,
  so `Check::byte` is no longer entered per byte of one (33.18's strict
  `bytea` rise and 33.13.3's `Check::byte` residual).
  `bytea/tests.rs`'s `a_bytea_folded_in_runs_decodes_as_byte_by_byte` holds
  it to `read_bytea` over every three-run cut.
- **`CanonicalHex::push`** (`gather/clip.rs`) reads its prefix a byte at a
  time and the digits after it as a stretch, which `bounds::Canonical` reads
  of a long `bytea`.

## The residual's rises it was given

- **M277's with M278's, in `Gatherer::observe_row`**: the function is gone,
  33.19 replacing it with `BlockObserver::fold` and `Gatherer::field`, where
  the refusal's quote (`printout_head`) still sits behind a closure. Nothing
  there is reworked; the pair reads whether `Gatherer::field` rises.
- **33.13.3's in `Check::byte`**: `bytea` no longer reaches it.

## Its count pair, placed and accepted

**`b3c217cb` against the tree resolves a fall on all four long rows, the
data-level parse, the strings query and the `arrays` row, and a rise on the
typed query and `pgdt sql -c`**; the metadata-level parse does not move
(`runs/count-pair-33.27-20261113/log`). callgrind reproduces both rises on
both rep pairs and places them (`…/callgrind/reading.txt`, the legs in
`/mnt/ssd/fedora/scratch/pgdump_query/count-pair-33.27/legs/`), D108's falls
unplaced. `Gatherer::field` places nothing: the data-level parse falls.

- **Both rises are one, `unescape_owned` over the control's escaped
  column**, whose escapes stand a few bytes apart: the parent read the held
  field through `Unescaped`'s machine a byte at a time, the tree through
  `Unescape::run`'s stage, paying per escape and per short stretch — a
  `memchr` call, `escape_at` called out of line, the stretch copied into the
  stage — more than the bytes between cost the machine. The stage's zeroing,
  the candidate expected first, is a small part. Changed code; no layout.
- **The strings query reads that column by the same function and falls**,
  its `bytea` text read there too, one long stretch where the typed rows
  decode it by `decode_bytea_into`: the same shape, per stretch rather than
  per byte, priced on the other side of where the two cost the same.
- **Accepted** (D108): the shape that would undo it, `unescape_owned`
  reading its held field a byte at a time, is D156's to refuse, and the
  control's stretches are its stress column's, shorter than real escaped
  text's. Spelling `escape_at` inline is the rework resting on codegen D108
  refuses.
