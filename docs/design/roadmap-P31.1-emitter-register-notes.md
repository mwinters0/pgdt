# P31.1 — The emitter register's extraction and join: notes

What the slices after this one inherit. The spec is
[`roadmap-P31-correctness-evidence.md`](roadmap-P31-correctness-evidence.md),
"The emitter register". No product code changed.

## What exists

- **`scripts/emitter_register.py`**, its rules in its docstring and asserted
  in `scripts/test_emitter_register.py`. `--extract` reads
  `release-v<minor>` under `$PGDT_POSTGRES_SOURCE` and writes
  `fixtures/<major>/emitters.tsv` at all six majors; with no flag it joins
  the committed registers against the committed fixtures and prints, per
  major, how many rows each half holds and which no fixture reaches. It exits
  non-zero only on a problem, never on an uncovered row.
- **A register names the release it was read from, and the join refuses one
  that is not the minor `generate_fixtures.ROUTINE_VERSIONS` pins.** A minor
  bump therefore re-runs `--extract` in the same change, or the scripts'
  `unittest` fails; `generate_fixtures.py` does not run the extraction.
- **`generate_fixtures.py` names what every run passes**, `PG_DUMP_ARGS` and
  `PG_DUMPALL_ARGS`, so `--username` and `pg_dumpall`'s
  `--no-role-passwords` count as run. Its behaviour is unchanged.
- **The value-form half is not here**: no row names it, which is a
  "Decision worth another look" in [`../status/STATUS.md`](../status/STATUS.md).
- **No dispositions yet.** Nothing exempts a row; the classification below
  is this document's, and 31.4 turns it into data beside the extraction.

## Rules the spec did not state, and why

- **A buffer whose `data` an execute call reads is a query, and nothing
  appended to it is a row.** Read literally, "every string constant passed to
  a buffer-append call" takes in every catalog query the type emitters build
  (`SELECT … FROM pg_catalog.pg_enum`), none of which a dump holds. The rule
  checks itself: a query buffer read anywhere else is a problem the
  extraction stops on, and none is, in the listed functions at any of the six
  releases. **So `setup_connection` yields no row at any major** — every
  statement it builds goes to the server — and the test pins that it is the
  only listed function contributing nothing.
- **`MIN_RUN` binds a plain constant as well as a format run.** A constant
  under three characters is punctuation every fixture holds.
- **A constant inside `PQfnumber`/`PQgetvalue`/`PQgetisnull` names a result
  column, and `fprintf(stderr, …)` is a diagnostic**; neither is a row.
- **`dropDBs` and `dropTablespaces` join `dumpDatabases` and
  `dumpTablespaces`** as `pg_dumpall`'s database and tablespace emitters: they
  are what `--clean` adds to its globals.

## Negative results

- **The join is by bytes, not by function, so a literal two functions share
  is covered by either.** `" INTEGER /* dummy */"` — `KD66`'s literal in the
  spec — is held by every major's `edge_cases/binary-upgrade.sql` through
  `dumpTableSchema`'s dropped-column placeholder, and so reads covered under
  `dumpCompositeType` too. `KD66` is still caught, by `DROP ATTRIBUTE ` and,
  at 17 and 18, the composite's own dropped-column comment; at 13–16 that
  comment's wording is `dumpTableSchema`'s and covered the same way. A unit
  finer than bytes would need a fixture to carry which function wrote it.
- **A format string's longest run is often its least telling word**:
  `CREATE %s%s %s` contributes `CREATE `, and the distinguishing bytes are
  its argument constants (`UNLOGGED `), which is why those are rows.
- **A spelling selected by a constant under three characters is invisible**:
  `_doSetFixedOutputState`'s `standard_conforming_strings` writes `on` or
  `off` as an argument, so even a listed emitter would leave `off` unasked.
  That is the value-form half's kind of fact, not the literal half's.
- **Widening the function list trips the query-buffer check**:
  `_selectOutputSchema`, `_selectTablespace`, `_selectTableAccessMethod`,
  `_printTableAccessMethodNoStorage`, `dumpTableData_copy`,
  `dumpTableData_insert` and `dumpDatabase` each read one buffer both as a
  query and as output (filed in P32's inbox, which owns the widening).

## Every uncovered row, classified

`cd scripts && uv run emitter_register.py` prints the list this classifies.
**No row needs an `I<n>`**: every uncovered literal is a shape a schema or a
flag set at that major can reach, a judgement by reading that 31.4's fixtures
confirm. Majors are all six unless named.

**Literals that are a filed defect's form.**

| Literals | Class |
|---|---|
| `UNLOGGED ` | `KD65` |
| `\nSERVER `, `\nOPTIONS (\n    `, `ALTER FOREIGN TABLE ONLY `; `ALTER FOREIGN TABLE ` (13–16) | `KD65`, the foreign table |
| `\nINHERITS (`; under `--binary-upgrade` the inheritance comments (`set up inheritance this way`, `recreate inherited column(s)`, `set up inherited constraint(s)`), the `attislocal` and `contype = 'c'` `UPDATE`s in both majors' spellings, `\n  AND conrelid = ` (13–16), `  AND conname IN (` and `::pg_catalog.regclass\n  AND attname IN (` (17, 18), and ` SET NOT NULL;\n` (13–17, a locally `NOT NULL` inherited column) | `KD64` |
| ` OF `, `\n-- For binary upgrade, set up typed tables this way.\n` | `KD70` |
| `DROP ATTRIBUTE `; `\n-- For binary upgrade, recreate dropped column.\n` and `\n  AND attrelid = ` (`dumpCompositeType`, 17, 18) | `KD66` |
| `CREATE TABLESPACE ` | `KD61`, `M199`'s |
| `\\connect -reuse-previous=on `, `\\encoding SQL_ASCII\n` | `KD68` |

**Literals a fixture can reach, with nothing suspected.**

| Literals | What reaches it |
|---|---|
| `dumpBaseType`'s property lines (`ALIGNMENT`, `ANALYZE`, `CATEGORY`, `COLLATABLE`, `DEFAULT`, `DELIMITER`, `ELEMENT`, `PASSEDBYVALUE`, `PREFERRED`, `RECEIVE`, `SEND`, `STORAGE`, `TYPMOD_IN`, `TYPMOD_OUT`; `SUBSCRIPT` 14+) | a base type over `LANGUAGE internal` I/O functions, which the superuser the generator connects as may declare; the preamble reads it as `TypeKind::Base` |
| `dumpRangeType`'s `canonical`, `subtype_diff`, `subtype_opclass` | a range whose canonical function is declared on its shell type, a `float8` range with `float8mi`, and a non-default operator class (`text_pattern_ops`) |
| `\n\tCONSTRAINT ` (`dumpDomain`) | a domain `CHECK`; `CONSTRAINT` already stops the type grammar |
| `DROP TYPE ` (four emitters), `DROP DOMAIN `, `DROP VIEW ` | `--clean` over a type, a domain and a view; only `edge_cases` runs `--clean`, and it declares none |
| `\nWITH (`, ` CHECK OPTION`, ` FORCE ROW LEVEL SECURITY;\n`, ` REPLICA IDENTITY FULL;\n` and `NOTHING;\n`, ` SET STATISTICS `; ` SET COMPRESSION ` (14+) | reloptions, a view `WITH CHECK OPTION`, and the per-table `ALTER`s |
| the materialized view's `relispopulated` comment and `UPDATE`; `\n-- set missing value.\n`, `binary_upgrade_set_missing_value(`, `::pg_catalog.regclass,`; `,\n             ` (17, 18) | `--binary-upgrade` over a populated materialized view, a column added with a default after rows exist, and a table with two dropped columns |
| ` NO INHERIT`, `COMMENT ON CONSTRAINT `, the `contype = 'n'` `UPDATE`, `conkey IN (`, `conname IN (` (18) | v18's named `NOT NULL` constraints (I37): the version sidecar 31.2 builds |
| `dropDBs`' and `dropTablespaces`' lines; `dumpTablespaces`' ` LOCATION `, header, `ALTER TABLESPACE `, `COMMENT ON TABLESPACE `; its binary-upgrade oid lines (15+) | `pg_dumpall` over a cluster holding a commented tablespace with options, under `--clean` and `--binary-upgrade` — the same fixture as `KD61`'s |

**Options.** A `pg_dumpall` option forwarded to each database's `pg_dump`
classes as `pg_dump`'s does and is named once; one whose `pg_dump` flag a
set already runs is the last row but one.

| Options | Class |
|---|---|
| `--quote-all-identifiers` | **suspected defect**: `format_type` quotes a type name it does not special-case, so a declared type may reach the mapping as `"date"`; unprobed, so not filed |
| `--disable-triggers`, and `--superuser`, which only it makes write | `KD1` |
| `--include-foreign-data` | `KD65`, with a foreign table's `COPY` block |
| `--format` other than plain, `--compress` | unsupported input: archives are P8's, compressed plain P15's |
| `--encoding` | unsupported input, [`pg-dump-compatibility.md`](pg-dump-compatibility.md)'s `client_encoding` row |
| `--extra-float-digits` | reachable; a value form, the session-setting axis's |
| `--inserts`'s siblings: `--rows-per-insert`, `--on-conflict-do-nothing`, `--attribute-inserts` | reachable; their reading is P8's |
| selection: `--schema`, `--table`, `--table-and-children`, `--exclude-schema`, `--exclude-table`, `--exclude-table-and-children`, `--exclude-table-data`, `--exclude-table-data-and-children`, `--exclude-extension`, `--extension`, `--filter`, `--strict-names`, `--section`, `--large-objects`/`--blobs`; `pg_dumpall`'s `--exclude-database`, `--globals-only`, `--roles-only`, `--tablespaces-only` | reachable; a subset of the objects, no spelling of its own |
| omission: `--no-tablespaces`, `--no-table-access-method`, `--no-toast-compression`, `--no-publications`, `--no-subscriptions`, `--no-policies`, `--no-unlogged-table-data`, `--no-large-objects`/`--no-blobs`, `--no-data`, `--no-schema`, `--no-statistics` | reachable; statements removed |
| `--statistics-only`, `--sequence-data` (18), `--enable-row-security`, `--disable-dollar-quoting`, `--use-set-session-authorization`; `pg_dumpall`'s `--clean` and `--if-exists` | reachable; each writes a statement or a quoting no fixture holds |
| `pg_dumpall`'s forwards of a flag `pg_dump`'s matrix runs: `--binary-upgrade`, `--column-inserts`, `--inserts`, `--data-only`, `--schema-only`, `--load-via-partition-root`, `--no-comments`, `--no-owner`, `--no-privileges`, `--no-security-labels`, `--verbose`; `--statistics` (18) | reachable; each database's output is a run `pg_dump` shape, and what is new is the globals beside it (the tablespace rows above) |
| `--no-acl` | alias of `--no-privileges`, run |
| no byte pgdt reads: `--host`, `--port`, `--dbname`, `--password`, `--no-password`, `--role`, `--file`, `--jobs`, `--lock-wait-timeout`, `--no-sync`, `--sync-method`, `--snapshot`, `--serializable-deferrable`, `--no-synchronized-snapshots`, `--no-reconnect`, `--help`, `--version`, `--restrict-key` (fixes the key of the `\restrict` every fixture holds), `pg_dumpall`'s `--database` | the spec's no-output-effect exemption |

**The no-output-effect exemption cannot be resolved as the spec words it.**
It carries `Evidence(file, needle)` "at the upstream line that consumes" the
option "so the check resolves it at every major", but the check reads no
checkout. Either the extraction resolves the needle and writes the result into
`emitters.tsv`, or the evidence points at something committed; 31.4 decides.
