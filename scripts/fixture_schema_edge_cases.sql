-- Scanner edge-case fixture schema: structural and escaping cases.
--
-- Deliberately not koji-derived: covers the edge-case breadth called out in
-- docs/design/decisions.md, "D69" (multiple schemas, NULLs, an empty table,
-- COPY TEXT escaping of newlines/tabs/backslashes/quotes, and a data value
-- containing a COPY-directive-like substring mid-line) rather than koji's
-- specific structure.
--
-- Type coverage is deliberately NOT this file's job -- see
-- scripts/fixture_schema_types.sql. Keeping the two apart keeps this file's
-- insta snapshots small and stable.

CREATE SCHEMA IF NOT EXISTS logs;

CREATE TABLE public.widgets (
    id integer PRIMARY KEY,
    name text,
    description text,
    is_active boolean,
    created_at timestamptz
);

INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES
    (1, 'alpha', 'a simple widget', true, '2024-01-01 00:00:00+00'),
    (2, 'beta', NULL, false, '2024-01-02 00:00:00+00'),
    (3, 'gamma', E'multi\nline\tdescription with a literal backslash \\ and a quote '' inside', true, NULL),
    (4, 'delta', 'contains a COPY-like phrase: COPY public.widgets (id, name) TO stdout; -- not a real directive', true, '2024-01-04 00:00:00+00'),
    (5, '', 'empty name above', NULL, '2024-01-05 00:00:00+00');

CREATE TABLE public.empty_table (
    id integer PRIMARY KEY,
    value text
);

CREATE TABLE logs.events (
    event_id bigint PRIMARY KEY,
    widget_id integer REFERENCES public.widgets(id),
    message text,
    logged_at timestamptz DEFAULT now()
);

INSERT INTO logs.events (event_id, widget_id, message) VALUES
    (100, 1, 'created'),
    (101, 2, NULL),
    (102, 3, E'updated\twith a tab char');

-- Round-trip coverage for COPY TEXT escaping. One row per codepoint means the
-- test can compare against a value it computes itself, rather than against a
-- hand-transcribed literal that could encode the same misreading twice.
--
-- chr(0) is rejected by PostgreSQL (NUL is not valid in text), so this starts
-- at 1. Codepoints 8, 9, 10, 11, 12, 13 and 92 are the ones pg_dump emits as
-- \b \t \n \v \f \r and \; the rest of the low range is written out as raw
-- control bytes, which is itself worth exercising.
CREATE TABLE public.escapes (
    codepoint integer PRIMARY KEY,
    value text NOT NULL
);

INSERT INTO public.escapes (codepoint, value)
SELECT g, chr(g) FROM generate_series(1, 127) AS g;

-- A spread of 2-, 3- and 4-byte UTF-8 codepoints.
INSERT INTO public.escapes (codepoint, value)
SELECT g, chr(g) FROM unnest(ARRAY[233, 1071, 12354, 8364, 128169]) AS g;

-- COPY's column list can be a strict subset of CREATE TABLE's -- dropped and
-- generated columns are in the DDL (dropped columns only under
-- --binary-upgrade, as a placeholder INTEGER /* dummy */ column with a
-- mangled name) and never in COPY.
CREATE TABLE public.dropped_column (
    id integer PRIMARY KEY,
    keep_me text,
    drop_me integer,
    also_keep boolean
);

INSERT INTO public.dropped_column (id, keep_me, drop_me, also_keep) VALUES
    (1, 'x', 5, true),
    (2, 'y', NULL, false);

ALTER TABLE public.dropped_column DROP COLUMN drop_me;

CREATE TABLE public.generated_column (
    id integer PRIMARY KEY,
    a integer,
    b integer,
    total integer GENERATED ALWAYS AS (a + b) STORED
);

INSERT INTO public.generated_column (id, a, b) VALUES
    (1, 2, 3),
    (2, 10, -4);

-- Dollar-quoted function bodies, pg_dump's own way of writing a value out
-- verbatim (docs/design/decisions.md, "D36"). check_function_bodies is off for
-- sample_fn only, so PostgreSQL doesn't reject its deliberately-invalid
-- pseudo-PL/pgSQL body at CREATE time -- pg_dump doesn't re-validate, so
-- whatever prosrc stored comes back out unchanged.
SET check_function_bodies = false;

-- The first two COPY-like lines are near-misses parse_copy_header already
-- rejects on grammar; the third is a syntactically perfect header followed
-- by rows and a bare terminator line -- the exact shape an unguarded scanner
-- mistakes for a real block, swallowing every block that follows.
CREATE FUNCTION public.sample_fn() RETURNS void
    LANGUAGE plpgsql
    AS $$
BEGIN
COPY public.widgets TO stdout;
COPY public.widgets FROM stdin WITH (FORMAT csv);
COPY public.widgets (id, name) FROM stdin;
1	adversarial
2	rows
\.
END;
$$;

RESET check_function_bodies;

-- A body containing an untagged $$ pair forces pg_dump's own delimiter
-- picker (appendStringLiteralDQ) to choose a tagged wrapper instead ($_$) to
-- avoid colliding with it -- exercising tag matching, not just "some
-- dollar-quote is open".
CREATE FUNCTION public.tagged_fn() RETURNS text
    LANGUAGE sql
    AS $func$
    SELECT 'contains an inner $$ marker' AS note;
$func$;
