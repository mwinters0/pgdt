-- A second data-carrying database for the `edge_cases/dumpall` fixture.
--
-- WHY THIS EXISTS. `pg_dumpall` output is the only shape in which one file
-- holds several `\connect`-delimited databases, and I1's recurring metadata
-- boundary -- the mapping pass restating a database's DDL at that database's
-- *first* `COPY` block -- can only be exercised by a file where **two**
-- databases carry `COPY` blocks. The cluster's own `template1` and `postgres`
-- carry none, so without this database the first block's offset already closes
-- out every database that could matter, the boundary is reached once, and its
-- recurring half is untested. Loaded into `pgdt_tenant` (scripts/
-- generate_fixtures.py) for the `edge_cases` schema only.
--
-- By I30 `pg_dumpall` orders its segments `template1` first and the rest by
-- `datname`, so this name lands the database between `pgdt_fixture` and
-- `postgres`: two consecutive data-carrying segments, then an empty one, which
-- is the arrangement that also leaves the EOF recompute its own case.
--
-- WHY THE TABLES LOOK LIKE THIS. `public.widgets` deliberately repeats the
-- name `pgdt_fixture` uses, with the same five column names and a **different
-- type on every one of them** -- Int64/FixedSizeBinary/Binary/Int16/Date32
-- against that database's Int32/Utf8View/Utf8View/Boolean/Timestamp. Resolving
-- a `pgdt_tenant` block against `pgdt_fixture`'s DDL is then a wrong answer a
-- test can see, rather than an absent one it has to infer from a missing
-- error. The `tenant` schema and its tables exist in no other fixture
-- database, so DDL that reaches them was provably read here rather than
-- inherited from the segment before.
--
-- Type coverage is not this file's job (scripts/fixture_schema_types.sql), and
-- neither is escaping (scripts/fixture_schema_edge_cases.sql). Four small
-- tables, each carrying rows, is the whole intent: a segment with `COPY`
-- blocks in it.

CREATE SCHEMA IF NOT EXISTS tenant;

-- Same name and column names as pgdt_fixture's; no column shares its type.
CREATE TABLE public.widgets (
    id bigint PRIMARY KEY,
    name uuid,
    description bytea,
    is_active smallint,
    created_at date
);

INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES
    (9000000001, '00000000-0000-0000-0000-000000000001', '\x00ff10'::bytea, 1, '2025-03-01'),
    (9000000002, '3f2504e0-4f89-11d3-9a0c-0305e82c3301', NULL, 0, '2025-03-02'),
    (9000000003, NULL, '\xdeadbeef'::bytea, NULL, NULL);

-- Unique to this database: nothing named this exists in pgdt_fixture.
CREATE TABLE public.tenant_only (
    slug text PRIMARY KEY,
    seats integer NOT NULL
);

INSERT INTO public.tenant_only (slug, seats) VALUES
    ('acme', 12),
    ('globex', 3);

CREATE TABLE tenant.ledger (
    entry_id bigint PRIMARY KEY,
    slug text REFERENCES public.tenant_only(slug),
    amount numeric(12,2),
    posted_at timestamptz
);

INSERT INTO tenant.ledger (entry_id, slug, amount, posted_at) VALUES
    (1, 'acme', 10.50, '2025-03-01 12:00:00+00'),
    (2, 'acme', -2.25, '2025-03-02 12:00:00+00'),
    (3, 'globex', 99.99, NULL);

CREATE TABLE tenant.settings (
    key text PRIMARY KEY,
    value text
);

INSERT INTO tenant.settings (key, value) VALUES
    ('locale', 'en_GB'),
    ('retention_days', '30');
