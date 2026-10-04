# P31.22.1 — Dates, times, timestamps and `interval` refused where every major refuses them: notes

What the slices after this one inherit. The mechanism is 31.22's, a
classifier run where a reader fails
([`roadmap-P31.22-reader-refusals-notes.md`](roadmap-P31.22-reader-refusals-notes.md)),
but the classifier is a whole grammar: `datetime.c`'s input path, ported.

## What exists

- **`datetime_in.rs`**, an L2 module: `ParseDateTime`, `DecodeDateTime`,
  `DecodeTimeOnly`, `DecodeDate`, `DecodeNumber`, `DecodeNumberField`,
  `DecodeTimezone`, `ValidateDate`, `tm2timestamp`, `DecodeInterval` and
  `DecodeISO8601Interval`, each read as its C, gated on the majors where they
  differ (13 and 14 alike, then 15, 16, 17, 18). `datetime_reads` (I83) and
  `interval_reads` (I84) answer whether some major reads a text under some
  setting.
- **Every failure of a reader is put to the grammar**, its refusals included
  (`predicate::datetime_read`, `interval_read`): the readers split a text
  outside `*_out`'s form otherwise than `ParseDateTime` does, so their marked
  refusals were wrong there — `+99` in `12:34:56.789 t a/b-99:00` read as an
  offset past fifteen hours where the server reads a POSIX zone, `day` in
  `1 day  day` as a unit given twice where 13 to 16 read a unit with no
  count. The readers are unchanged; a filter literal still reads by them
  alone.
- **The settings are read at their most permissive**: each `DateStyle` order
  and `IntervalStyle`; a word no keyword names as a fixed zone abbreviation,
  a keyword as one only where a shipped file names it (`SAT`, `Australia`:
  `SHADOWED_KEYWORDS`); a word with punctuation as a zone file — no level
  empty or hidden — or a POSIX spec (`posix_zone`); a zone the text does not
  fix as any offset within 168 hours, which `TimeZone = 'FOO-167'` reaches.
- **`CACHE_FORMAT_VERSION` is 61**: a block a strict parse checked in full
  under 60 may hold a field this build refuses. No persisted byte moved; both
  pinned digests re-pinned to the version alone.
- **Evidence**: `datetime_in`'s
  `a_text_is_read_exactly_where_some_major_reads_it_under_some_setting`, 707
  cases with the majors reading each, and `predicate.rs`'s
  `a_date_or_time_field_is_refused_only_where_no_major_reads_it`. The cases
  came from a differential run (I83's "Observed") whose scripts are in
  `runs/datetime-oracle/`, machine-local: `gen_cases.py` writes the cases and
  the abbreviation files, `run_oracle.sh` the six servers' verdicts, and
  `compare.py` sets them beside this build's, which `cycle.sh` reads from a
  `#[ignore]` test printing `field_key`'s answer per case of
  `PGDT_DT_CASES` — not committed, so re-added to run. Generated texts are
  hex-encoded into a table in a container per major and put to each type
  through `EXECUTE format('SELECT %L::%s', …)` in a PL/pgSQL function trapping
  the error — a literal, so `interval_in` is handed the qualifier's typmod —
  under `set_config` of each `DateStyle`, `IntervalStyle` and abbreviation
  file; a file per word is written to the share directory's `timezonesets`
  as `@INCLUDE Default`, `@OVERRIDE` and the word, its name letters only.

## Negative results and limits

- **A keyword shadowed by a file of the server's own is not modelled** (D55):
  `4714-11-23 BC` reads as AD under a file naming `bc`, which the run's first
  pass showed. Only `Australia`'s `SAT` is read so.
- **A conversion of a `double` to an integer is x86-64's**, the least integer
  past its range or for NaN, which is what 13 and 14 read `P-nanD` by; and
  glibc's `ERANGE` on a hexadecimal subnormal is taken as not set.
- **A byte past `0x7F` reads as read**: its class is the server's locale's.
- **No reader was widened**: `y2001m02d04` or `2020-01-01 foo5` still fails a
  query decoding it with `FieldDecode`.

## What the slices after this inherit

- **31.22.2** (`KD98`) owes `interval`'s field qualifier to the comparison kind:
  `pgtype::builtin_name` drops `year to month` as it names the type, and
  `datetime_in::INTERVAL_RANGES` is tried whole in its place. A qualifier
  changes the unit a bare number takes and reads `a:b` as minutes and seconds
  under `minute to second`; `scripts/oracle_register.py` parses the
  `agrees(K::Interval)` arm (D71), so the kind's shape is that script's too.
