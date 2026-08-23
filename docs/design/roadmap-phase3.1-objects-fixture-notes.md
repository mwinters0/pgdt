# Phase 3.1 — the `objects` fixture schema: how it landed

Companion to [`roadmap-phase3-object-inventory.md`](roadmap-phase3-object-inventory.md)'s
slice table. That doc states the design; this one records what slice 3.1
produced and what later slices inherit from it. Per `CLAUDE.md`, this file is
consolidated into a single phase-level notes doc (and removed) once all of
Phase 3 lands.

## What landed

- **`scripts/fixture_schema_objects.sql`** — a third fixture schema, one
  object per TOC `Type:` kind that neither `edge_cases` nor `types` produces,
  plus two large objects. Its header is the authoritative list of what it
  deliberately does *not* cover and why; don't re-derive that from the SQL.
- **`fixtures/<version>/objects/{default,verbose}.sql`** for all 6 routine
  versions (13–18), real `pg_dump` output. `--verbose` is the flag set's
  whole reason: it is the one documented way to widen the TOC comment block
  past three lines, adding `-- TOC entry <id> (class <oid> OID <oid>)` and,
  where the entry has them, `-- Dependencies: <ids>`.
- **`scripts/generate_fixtures.py`** — the `objects` schema entry, and a
  `drop_fixture_db` that clears the schema's subscription and role.

Size, for calibration: `fixtures/18/objects/default.sql` is 17KB, `verbose.sql`
20KB. Every file is small enough to assert against byte-for-byte in a test.

## What later slices inherit

**The TOC comment block is not a fixed height.** Three lines at minimum
(`--`, `-- Name: …`, `--`), more under `--verbose`, so a reader must run to
the closing `--` rather than assume an offset. The `-- Name:` line also
carries an optional `; Tablespace: <name>` suffix. Slice 3.3's enrichment
layer is the direct consumer.

**Large objects change shape at v17, and the routine matrix spans the
change.** v13–16 emit one shared `BLOBS` entry with one `BEGIN;`/`COMMIT;`
around every object's `lo_open`/`lowrite`/`lo_close` run; v17+ renamed the
definition entry to `BLOB METADATA` and gives each *group* of large objects
its own `BLOBS` entry and its own `BEGIN;`/`COMMIT;`. So "the large-object
region ends at the first `COMMIT;`" is false on v17+ — finding the region's
end means walking past every consecutive `BLOB METADATA`/`BLOBS` header. This
is what slice 3.6's fast path has to implement; recorded as
[`postgres-invariants.md`](postgres-invariants.md) I12, with the version-split
table and source line numbers. A two-object fixture establishes that grouping
happens, not what the group-size threshold is.

**Three shapes needed a specific construction to appear at all**, each
confirmed against `pg_dump` source rather than guessed:

- **`SEQUENCE OWNED BY` needs `serial`, not `GENERATED ALWAYS AS IDENTITY`.**
  `dumpSequence` special-cases `is_identity_sequence` and folds the ownership
  into the identity clause, emitting no separate entry.
- **A separate `DEFAULT` entry needs a *view* column** (or a
  `--binary-upgrade` dropped column, which `edge_cases` already covers).
  `dumpAttrDef`'s `separate` flag is set for nothing else, so an ordinary
  table default folds into `CREATE TABLE`.
- **`PUBLICATION TABLES IN SCHEMA` is PG15+.** The schema carries a psql
  `\gset`/`\if` gate on `server_version_num`, falling back to a second `FOR
  TABLE` publication below 15. Both branches are exercised across the matrix.
  No other object in the schema needed a version gate — everything else has
  been available since PG10, inside the matrix's PG13 floor.

**The fixture database needs explicit teardown.** A subscription blocks
`dropdb` on its own database, and a role is cluster-global, so it leaks into
whatever schema runs next in the same container. `drop_fixture_db` clears
`objects_sub` and `fixture_reader` by name; both statements are `IF EXISTS`
and no-ops for the other schemas. A future fixture schema that adds its own
subscriptions or cluster-global objects has to grow this cleanup rather than
inherit a generic one — see STATUS's "Decisions worth another look".

## Coverage

Added by this schema: `ACL`, `AGGREGATE`, `BLOB METADATA`/`BLOB`, `BLOBS`,
`CAST`, `COLLATION`, `COMMENT`, `CONVERSION`, `DEFAULT`, `DEFAULT ACL`,
`EVENT TRIGGER`, `INDEX`, `INDEX ATTACH`, `MATERIALIZED VIEW`(`DATA`),
`POLICY`, `PUBLICATION`(`TABLE`/`TABLES IN SCHEMA`), `ROW SECURITY`, `RULE`,
`SEQUENCE`(`OWNED BY`/`SET`), `SERVER`, `STATISTICS`, `SUBSCRIPTION`, `TABLE
ATTACH`, `TEXT SEARCH CONFIGURATION`/`DICTIONARY`, `TRIGGER`, `USER MAPPING`,
`VIEW`.

Still unexercised by any fixture, all needing an extension or an unavailable
provider: `SECURITY LABEL`, `ACCESS METHOD`, `OPERATOR`(`CLASS`/`FAMILY`),
`TRANSFORM`, `TEXT SEARCH PARSER`/`TEMPLATE`. Plus the dump-level metadata
kinds (`DATABASE`(` PROPERTIES`)/`ENCODING`/`SEARCHPATH`/`STDSTRINGS`), which
are not DDL objects this phase inventories, and `STATISTICS DATA`
(`--with-statistics`, PG18+, outside the routine matrix). TOC coverage is
reported per file by design, so an uncovered kind degrades to "unrecognized",
never a crash.
