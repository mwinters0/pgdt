# P31 — Correctness evidence: the emitter register and value oracles

What this phase will do and why; how it lands is its slices'. Progress is
`STATUS.md`'s checklist, never this file. **Its first slices exist to produce
evidence**, the register and the oracle, so the fix slices after them are
allocation order as much as schedule. Grilled 2026-10-02.

> **A defect the register finds becomes a P31 slice automatically.** A round
> that files a `KD<k>` of the phase's class (below, "What the register finds,
> and when the phase ends") makes it **(b) owned by `P31`** and appends its
> slice to `STATUS.md`'s checklist **in the same change**, numbered as the
> next free `31.<M>`. This is the maintainer's standing approval, given at the
> grilling: it is neither a "Decision worth another look" nor a reason for an
> unattended loop to stop, and the stand-in need not be asked.

## Premise

**A form `pg_dump` can write that no fixture holds is a defect waiting to be
found by reading, and that class is enumerable from the producer's source.**
Nine defects in two days (`KD60`–`KD68`) were each found by reading
`pg_dump`'s source or a decoder against the forms it can meet, never by a
failing test, each sitting where a grammar or decoder was right about
everything it had seen
([`../status/history/2026-10-02.md`](../status/history/2026-10-02.md)).
Three reconciliations already enumerate a source against its evidence —
resolution outcomes against fixture columns (D69), comparison arms against
oracle cases (D71), the ADBC floor against the mapping (D38) — and each
enumerates *our* side or the driver's. None enumerates **`pg_dump`'s
emitters**, the half `roadmap.md`, "Expand the generated fixtures freely;
verify objectively wherever possible" names as a judgement call. Every one of
the nine is in that half.

And **the round trip is self-consistency, not a second reading** (D73): it
proves `render ∘ decode` is the identity on the fixture's values, so a decoder
that is wrong and invertible passes it. The comparison oracle (D70) is a
genuine second reading, but of order, not of value.

## Scope

**The phase delivers evidence first, then the fixes for what that evidence
finds.** The register and the value oracle land before any fix, and a fix
slice is what turns one of the register's known failures green, so the
register is what proves each fix complete. It owns `KD64`–`KD70`: the five the
2026-10-02 audit filed, plus `KD69` (`parse_create_table`'s non-column
fragments) and `KD70` (a typed table's columns), both emitted by
`dumpTableSchema` and filed by this grilling. **`KD61`–`KD63` stay on
`M199`–`M201`**, one-session fixes already admitted; `M199` and `M200` touch
`preamble.rs` as P31's fix slices do, so they land first.

**The register covers what a dump holds** — DDL, framing and `COPY` text —
not what a query types: a filter literal and a stored bound stay the
comparison oracle's (D70), which already stands in for them.

## The emitter register

**Two halves, because the emitters are two kinds of code.**

- **The DDL half is mechanical.** Its unit is **a literal**: every string
  constant passed to a buffer-append call inside a hand-listed set of
  `pg_dump` functions, a format string contributing its longest constant run.
  A script reads them out of each supported major's checkout
  (`CLAUDE.local.md` names where those live), so a new major is re-read by
  pointing it at the new tree and a literal the major added surfaces
  uncovered. **Each literal resolves to fixture bytes that hold it**, or to
  an exemption the script resolves: an `I<n>` proving no producer we run
  writes it, or a `KD<k>` naming it as a known failure. No C parser: a
  branch is reached when what it appends is in some fixture.
- **The function list is the functions whose output some pgdt reader
  consumes**: `dumpTableSchema`, `dumpCompositeType`, `dumpEnumType`,
  `dumpRangeType`, `dumpDomain`, `dumpBaseType`, `appendPsqlMetaConnect`,
  `_printTocEntry`, `setup_connection`, and `pg_dumpall`'s database and
  tablespace emitters. Their size at 18.6 bounds the register at a few
  hundred literals: `dumpTableSchema` alone is about 1000 lines and 121
  append calls.
- **The value-form half is hand-listed**: the `*_out` spellings a GUC
  `pg_dump` leaves unpinned or a typmod selects (I4), backend-side and
  small. It is what the session-setting axis below exists to reach.
- **The option half is mechanical too**: every long option in `pg_dump`'s and
  `pg_dumpall`'s option tables, at each major, resolves to a flag set some
  schema's matrix runs, to the reason it changes no byte pgdt reads
  (`--host`, `--jobs`, `--no-sync`), or to a `KD<k>` (`--disable-triggers`,
  `KD1`). A literal register cannot see a flag that moves quoting or row
  shape without adding a literal — `--quote-all-identifiers` touches every
  identifier the preamble parses and appends nothing new. At 18.6 the table
  holds about seventy options, and those moving output bytes with no flag set
  today include `--quote-all-identifiers`, `--include-foreign-data`,
  `--rows-per-insert`, `--on-conflict-do-nothing`, `--extra-float-digits` and
  `--disable-triggers`. The pinned `-trixie` images ship `file_fdw`, so a
  foreign table is fixturable.
- **What an entry may resolve to.** A literal resolves only to fixture bytes
  *at the same major*, to an `I<n>`, or to a `KD<k>`: there is no "pgdt does
  not read this" exemption, because every byte passes through the map, which
  must tile it, so an ignored statement still earns its fixture. An option may
  also be exempt as having no output effect (`--host`, `--jobs`,
  `--no-sync`), carrying `Evidence(file, needle)` at the upstream line that
  consumes it so the check resolves it at every major, as D71's exemptions
  do. A format-string run under three characters is no entry.
- **Where it lives**: the dispositions are data inside
  `scripts/emitter_register.py`, beside the extraction, as
  `oracle_register.py` holds its own.
- **Extraction is generation; the join is the check.** The checkouts are
  machine-local, and no script reads one at check time, so extraction writes
  `fixtures/<major>/emitters.tsv` — one row per function, literal and option —
  from a checkout `CLAUDE.local.md` names and an environment variable
  overrides, and the check joins committed rows against committed fixtures,
  running anywhere `mise run check` does. A new major is that file
  regenerated, its new literals arriving in the diff.

What it would have caught is the test of the unit: `KD61`, `KD64`, `KD65`,
`KD66` and `KD68` emit `CREATE TABLESPACE`, `INHERITS (`, `UNLOGGED `,
`/* dummy */` and `-reuse-previous=on`, each a literal. `KD60`, `KD62`,
`KD63` and `KD67` are value or ordering forms, which the value-form half and
the value oracle exist for.

## What the register finds, and when the phase ends

**A defect of the phase's own class — a form `pg_dump` writes that pgdt
misreads — is absorbed**: it becomes a `KD<k>` owned by P31 and a slice
appended in allocation order. A property not yet captured goes to P32's
inbox, and a finding in `INSERT` or archive reading to P8's. **The phase ends
when the register is green with no exemption naming a P31 `KD`**, which
bounds it by its thesis rather than by a list fixed at grilling.

## A known failure is asserted to fail

**A fixture exposing a defect lands before the fix, in a strict known-failure
table keyed by `KD<k>`**: a test walking the fixture tree asserts that each
listed case still fails, citing its entry, so a fix flips the case and fails
the test until the fix's slice deletes the row. It is the test-side mirror of
the register's `KD` exemption, keeping the evidence first without a gap
reading as coverage.

## The value oracle

**The server records a second reading of every typed value, not in its
output spelling**, so a test asserts the Arrow value itself rather than its
rendering. It is its own pass in `generate_fixtures.py`, in the container and
database the comparison oracle uses (where the typed DDL is), written to
`fixtures/<major>/oracle/values.tsv`.

- **Which columns**: every column of the `types` schema whose Arrow mapping
  is not text.
- **Which reading**: an epoch in microseconds for a time or timestamp, the
  Julian day for a `date`, the unscaled integer and scale for a `numeric`,
  `float8send`'s hex for a float, `encode(…, 'hex')` for a `bytea`, and
  months, days and microseconds for an `interval`.
- **Which row**: keyed by `ctid` order, which is the order `COPY` writes.
- **Nested values** are read element by element through the same
  expressions, by `unnest … WITH ORDINALITY` and `(v).field`.
- **A fifth reconciliation**, D71's shape: every `builtin_scalar` arm mapping
  to a non-text Arrow type has a value-oracle case, so a mapping cannot land
  with nothing checking its values.
- **The round trip stays beside it**: it holds that render inverts decode,
  which `SchemaMode::Strings` and text output rest on, and the oracle holds
  that decode is right. D73 gains the line saying which test holds which.

## The session-setting axis

**`pg_dump` pins `DateStyle`, `IntervalStyle` and `extra_float_digits` and
nothing else that moves a spelling (I4)**, so a setting a server, database or
role carries reaches the `COPY` text, and every fixture today is taken at the
image's defaults. **Each such setting that pgdt decodes is one variant of the
`types` schema**, set by `ALTER DATABASE … SET` before the dump — how a real
server produces one, and how the 16.15 probe reached `KD67` — and written as
its own flag set, `fixtures/<major>/types/<setting>.sql`. One variant per
setting, never a cross product.

- **`bytea_output = escape`**, `KD67`'s fixture.
- **One `TimeZone`** with a half-hour offset and seconds-precision historical
  offsets (`Africa/Monrovia`), what I48's parse has to survive.
- **Not `lc_monetary`**: `money` stays text by `KD13`'s stance and its
  spelling is never decoded. **Not `client_encoding`**: it stays an
  `Unsupported` row of [`pg-dump-compatibility.md`](pg-dump-compatibility.md).

**The value oracle serves every variant unchanged**, its readings being
server-side and no setting moving them: a variant's typed read is held to the
same `values.tsv`, which is the second reading the escape decoder and the
zone handling get. `--extra-float-digits=0` writes a lossy spelling, so its
flag set's check is equality within the text's precision; how that is
expressed is the slicing's.

## Version-conditioned schemas

**DDL some majors refuse is a sidecar schema file per minimum major**,
`fixture_schema_<schema>.<major>.sql`, loaded after the base file on every
major at or above it — the shape flag sets already take with their
`(min_version, flags)` tuple. v18's `CONSTRAINT <name> NOT NULL`, `NO
INHERIT` and virtual `GENERATED ALWAYS AS` columns (I37) are its first
content. Refused: a `DO` block testing `server_version_num`, which hides the
DDL inside a string and makes a failed load a runtime branch rather than a
missing file.

## Columns declared elsewhere, and what the preamble holds

**A table's columns are its own plus those its references declare, read
against the preamble's final state.** A table records its `INHERITS` parents
and its `OF` type — and the `--binary-upgrade` forms, `ALTER TABLE ONLY …
INHERIT` and `… OF`, which that mode writes after a full column list — and
resolution looks a name missing locally up through them: one walk closing
`KD64` and `KD70` together. A typed table's column written without a type,
which `dumpTableSchema` does for one carrying a default or `NOT NULL`
(`CREATE TABLE x OF t (a DEFAULT 5)`), defers to the walk rather than being
dropped. Refused: flattening the references into the child's list when the
preamble is folded, which a later `ALTER TABLE parent ADD COLUMN` or `ALTER
TYPE … ADD ATTRIBUTE … CASCADE` would not reach — valid PostgreSQL a
hand-written file may hold, and inheritance being live in the server.

**The preamble holds columns, not table constraints, in this phase.** The
`CREATE TABLE` grammar classifies a `CONSTRAINT`, `CHECK`, `UNIQUE`, `PRIMARY
KEY`, `FOREIGN KEY`, `EXCLUDE` or `LIKE` fragment as not-a-column and the
comma split tracks `[`…`]`, closing `KD69`. Discarding them is P31's scope,
not the goal: capturing every object and property a dump declares is
P32 ([`roadmap.md`](roadmap.md), "P32 — The schema model: every object and
property a dump declares"), whose inbox holds what this grilling found about
it.
