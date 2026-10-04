# P31 — Correctness evidence: notes

The phase's notes, consolidated at its wrap. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md);
what it built is [`../status/STATUS.md`](../status/STATUS.md)'s capability
rows, why each mechanism has its shape is the code and
[`decisions.md`](decisions.md) (D36, D55, D73, D103 above all), and every
external fact a refusal rests on is an invariant its `pg-refuses` marker names.
This keeps what neither records: negative results, and facts aimed at later
work.

## The emitter register sees bytes and keywords, not writers or uses

- **The join is by bytes, so a literal two functions write is covered by
  either.** `" INTEGER /* dummy */"` is `dumpTableSchema`'s dropped-column
  placeholder and `dumpCompositeType`'s alike, and a fixture holding one
  covers both. A unit finer than bytes would need a fixture recording which
  function wrote what.
- **A format string's longest run is often its least telling word**:
  `CREATE %s%s %s` contributes `CREATE `, and the distinguishing bytes are
  its argument constants (`UNLOGGED `), which is why those are rows.
- **A spelling a short constant selects is not asked by its row**:
  `standard_conforming_strings`'s `on` or `off` is an argument under the
  minimum run. That is a value form's kind of fact, reached by a flag set
  (`emitters/standard-conforming-strings-off`).
- **The reader check is per keyword, not per use.** A reader that starts
  dispatching on a word `READS` already holds — a `Clause` taken as a new
  statement form — passes unchanged; only a new keyword constant is forced
  into a row. A reader matching by a character is seen by no keyword: the
  map takes any line opening with `\` as a meta-command (`\restrict`).
- **A setting variant's reading needs every comparison path, not only the
  decoder.** A `*_out` with two spellings reaches equality's canonical arm and
  gathering's `Canonical` too: before `bytea`'s escape form was read, `=` on
  such a column answered no row rather than refusing.

## The value oracle's reach

- **It reads the `types` schema alone**; a column another schema declares is
  not held to it.
- **A type outside `READINGS` contributes containers only**; the fifth
  reconciliation is what fails a new typed arm with no case.
- **No fixture holds a multi-dimensional array of composites**, which it reads
  by subscript one dimension deep; one would show as typed nodes it does not
  read. A generated column is not read, `COPY` omitting it.

## Refusals: where a reader and the server's grammar part

- **A marked check can be reached by a shortfall's spelling.** A run-together
  `+0530` reached the offset reader's hour check as an hour of 530, and
  `numeric_in_stores` counted digits before an exponent moved the point. Each
  marked check has to sit behind a grammar no wider than the server's at the
  parts it bounds.
- **The date and time readers split a text outside `*_out`'s form otherwise
  than `ParseDateTime` does**, so their own refusals were wrong there — `+99`
  in `12:34:56.789 t a/b-99:00` is a POSIX zone's, and `1 day  day` a unit
  with no count to 13–16. Every failure of theirs is put to the ported
  grammar, refusals included.
- **No reader was widened.** A blank around a number, a hexadecimal float,
  `nan(…)`, an exponent in a `numeric` field, `0x1F`, `1_000` and
  `y2001m02d04` stay unparsed, and a query decoding one fails with
  `FieldDecode`. A geometric field reads a hexadecimal number where a float
  field does not: its grammar must know where each number ends, so
  `strtod`'s whole prefix is ported there.
- **A cast of a literal is not `COPY`'s reading of a typmod'd field**: the
  cast calls `numeric_in` with no typmod and coerces after, so `1e-16384` as
  `numeric(10,2)` is refused by the cast and read by `COPY`. A probe of a
  typmod'd input calls the input function with the typmod (I82).
- **`oidin` and `numeric_in` break I35's additivity outside the oracle's
  cases** (`08` and `010`; `1e 5` and `0e1073741823`); I35's scope limit and
  the manual's "Where PostgreSQL majors differ" hold them.
- **Modelled as the server's build, not as C promises**: `inet_net_pton`'s
  netmask wrapping as `-fwrapv` builds it; a `double` converted to an integer
  as x86-64 converts it, the least integer, which is what 13 and 14 read
  `P-nanD` by; glibc's `ERANGE` on a hexadecimal subnormal taken as unset; a
  byte past `0x7F` read as read, its class being the server's locale's.
- **The differential runs behind I83 and I84 are machine-local**,
  `runs/datetime-oracle/` and `runs/interval-qualifier-oracle/`, each driving
  an `#[ignore]` test that was never committed and is re-added to run. The
  other probes were not kept; each was a container per major, every case
  hex-encoded into a table and put to the input function, with its typmod,
  through a PL/pgSQL function trapping its error.

## What a strict parse still does not see

- **A `json` document's depth**: the restoring server's `max_stack_depth`
  decides it, a setting outside the promise. `json_in` reads `\u0000`, a lone
  surrogate half and a number past `numeric`'s range, each of which `jsonb_in`
  refuses. Neither a `json` nor a bit-string filter literal is checked, there
  being no comparison of either here.
- **A NULL in a `NOT NULL` column is refused by a filter term on the row path
  alone**: a group its statistics prune or answer reads no row, as a pruned
  group's field past a `varchar(n)` is not refused. A `default` parse keys no
  nested column, so it refuses no NULL beneath a `NOT NULL` domain and no
  nested leaf past its length.
- **A `NOT NULL` added after the data binds nothing**, as it binds no `COPY`;
  nor is one followed through an `ALTER TABLE` of several subcommands or an
  `ALTER DOMAIN … SET NOT NULL`, which no `pg_dump` writes before the data.
- **An ignoring parse's record of a declined block stops at the decline**, the
  reach a refusing parse's check has; only a strict re-read covers the rest.
- **An inexact enum with no label read resolves as an empty enum**, a text
  column; only `info` tells it from one whose labels were read.
- **Where no fixture is the evidence**: the types refused whole or read by a
  grammar refusing nothing (I78, I79), a `NOT NULL` domain beneath a container
  (no `pg_dump` writes a NULL a restore refuses), a range bound, a `DEFAULT`
  partition and a partition of a partition (`partitions` holds list and hash
  partitions one level deep), a role named `"PUBLIC"`, and a CR in a row or a
  literal are each held by source reading, the koji replica and hand-written
  dumps instead.
- **A v15+ server writes a non-ASCII `typdelim` as `charout`'s octal escape**,
  `DELIMITER = '\377'`, which a restore takes as `\`; the preamble reads that
  byte and the split declines it.

## What later work inherits

- **P23**: what a strict observer holds to check — a comparison plan per
  column — is charged to no statistics account (D81). It is fixed per block,
  shared with the block's pieces and freed with the observer.
- **Whoever reasons from a scan figure**: the row scan's search for a row's
  end became a two-byte search when row endings were checked, and no scan
  figure was re-taken over it in this phase; `measure.py --stale` says which
  are red.
- **A reader of bare-CR blocks**: `KD101`'s marker in `scan.rs` says what it
  inherits.
- **P8 and P32**: their inboxes hold this phase's facts — an `INSERT`
  refusal's raise and numbering, and the table constraints, `ALTER` forms, a
  `LIKE`'s columns and `KD102`'s partition columns no model captures.
