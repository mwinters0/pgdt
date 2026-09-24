-- Per-row-group statistics fixture schema: the column shapes bounds,
-- dictionaries, null counts and block sortedness are gathered from and pruned
-- against.
--
-- **Every table is filled by one INSERT into a fresh table and never updated**,
-- so the heap holds the rows in insertion order and `pg_dump`'s COPY writes them
-- in that order. That is what makes a column declared "ascending" below
-- ascending in the file. It is asserted on the bytes, not assumed:
-- pgdump_query/tests/statistics_fixture.rs reads each block and checks every
-- shape this header names, on every major.
--
-- No value here needs COPY escaping, so a field's text in the file is the value
-- the server rendered.
--
-- **Not held yet**: a `numeric` `Infinity` (PostgreSQL 13 cannot), and a
-- `bytea` column carrying bounds, so statistics_fixture.rs reaches neither;
-- add a column rather than reason about bytes.

-- ---------------------------------------------------------------------
-- Row order. 1000 rows, enough that a group stated at a few KiB holds more
-- than 64 of them, so the two cardinality columns differ in whether a
-- dictionary fits.
-- ---------------------------------------------------------------------
CREATE TABLE public.ordered (
    -- Strictly ascending.
    id integer,
    -- Strictly descending.
    reversed integer,
    -- Ascending with equal neighbours: ten rows per value.
    stepped integer,
    -- A permutation of 0..999, in neither order.
    unsorted integer,
    -- Every value equal: holds both order flags.
    constant integer,
    -- No non-null value at all.
    all_null integer,
    -- Ascending over its non-null values, with NULLs between them, which
    -- physical order puts nowhere in particular.
    gappy integer,
    -- Exactly one non-null value, the other shape that holds both flags.
    single integer,
    -- Four distinct texts, repeating: a dictionary fits any group.
    low_card text COLLATE "C",
    -- Two hundred distinct texts, cycling: more than 64 in any group holding
    -- more than 64 rows.
    high_card text COLLATE "C",
    -- Ascending bytewise ('Z' is 0x5A, 'a' is 0x61) and not under the
    -- database's default collation, which sorts 'a' before 'Z'.
    c_text text COLLATE "C",
    -- The mirror: ascending under the default collation, not bytewise.
    default_text text
);
INSERT INTO public.ordered
SELECT
    i,
    1001 - i,
    (i - 1) / 10,
    (i * 389) % 1000,
    7,
    NULL,
    CASE WHEN i % 7 = 0 THEN NULL ELSE i END,
    CASE WHEN i = 500 THEN 42 END,
    (ARRAY['amber', 'blue', 'cyan', 'dusk'])[i % 4 + 1],
    'v' || lpad((i % 200)::text, 3, '0'),
    CASE WHEN i <= 500 THEN 'Z' ELSE 'a' END || lpad(i::text, 4, '0'),
    CASE WHEN i <= 500 THEN 'a' ELSE 'Z' END || lpad(i::text, 4, '0')
FROM generate_series(1, 1000) AS i;

-- The two collation claims above are the server's to make, so the server is
-- asked, and a false one fails the generator rather than reaching the file.
-- `id` is file order, which the test asserts. A DO block is never dumped.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM (
            SELECT default_text AS t, lag(default_text) OVER (ORDER BY id) AS prev
            FROM public.ordered
        ) AS s WHERE t < prev
    ) THEN
        RAISE EXCEPTION 'default_text is not ascending under the default collation';
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM (
            SELECT c_text COLLATE "default" AS t,
                   lag(c_text COLLATE "default") OVER (ORDER BY id) AS prev
            FROM public.ordered
        ) AS s WHERE t < prev
    ) THEN
        RAISE EXCEPTION 'c_text is ascending under the default collation as well';
    END IF;
END
$$;

-- ---------------------------------------------------------------------
-- Special values, in PostgreSQL's own order where a column says ascending:
-- NaN sorts above every other float and numeric, and -0 equals 0.
-- ---------------------------------------------------------------------
CREATE TABLE public.specials (
    id integer,
    -- Ascending under PostgreSQL's order, -0 and 0 equal neighbours.
    f8 double precision,
    -- The same values, not in order.
    f8_unsorted double precision,
    -- Ascending under PostgreSQL's order, as real.
    f4 real,
    -- Equal values under three spellings, then a larger one, then NaN: a
    -- decoded comparison finds 1.50 from 1.5, a bytewise one does not.
    n numeric
);
INSERT INTO public.specials VALUES
    (1, '-Infinity', 'NaN', '-Infinity', '1.5'),
    (2, -1.5, 0, -1.5, '1.50'),
    (3, '-0', '-Infinity', '-0', '1.500'),
    (4, 0, 'Infinity', 0, '2'),
    (5, 1.5, -1.5, 1.5, NULL),
    (6, 'Infinity', 1.5, 'Infinity', '2.0'),
    (7, 'NaN', '-0', 'NaN', 'NaN');

-- ---------------------------------------------------------------------
-- A value longer than the read chunk and the default row group size,
-- both one mebibyte, so a scan carries one row across chunks, a group holds a
-- row running past its end, a small stated group size leaves groups in which
-- no row starts, and a leader cut can land inside the row. Past the stored
-- value cap too, as is the 300-byte value, which is the one a truncated bound
-- is taken from without the megabyte behind it. Ascending bytewise. It stays
-- past the read chunk on every major: a shorter one hid a quadratic carry scan
-- (scan.rs, `a_line_many_chunks_long_is_scanned_once`).
-- ---------------------------------------------------------------------
CREATE TABLE public.long_value (
    id integer,
    v text COLLATE "C"
);
INSERT INTO public.long_value VALUES
    (1, 'a-short'),
    (2, repeat('m', 300)),
    (3, 'n' || repeat('0123456789abcdef', 66000)),
    (4, 'z-short');

-- ---------------------------------------------------------------------
-- What the DataFusion provider's plan answers are checked against
-- (docs/design/roadmap-P25-plan-answers.md, "Evidence"): each table below
-- holds a shape one of those answers can get wrong with no error.
-- ---------------------------------------------------------------------

-- A table whose rows arrive as **several blocks of one table** under
-- `--load-via-partition-root` (I2), and as one table per partition without
-- it. Rows are routed in insertion order, and pg_dump writes the partitions in
-- name order, so a column ascending here ascends across every block boundary;
-- the harness asserts both on the bytes rather than trusting this.
CREATE TABLE public.spans (
    -- The partition key: constant in each block, ascending across them.
    part integer,
    -- Strictly ascending across every block.
    id integer,
    -- Strictly descending across every block.
    reversed integer,
    -- Ascending inside each block and restarting in the next: sorted in every
    -- block, and not across a boundary.
    local integer,
    -- Ascending over its non-null values, with one NULL.
    gappy integer,
    -- Ascending bytewise across every block.
    label text COLLATE "C",
    -- Four values in every block.
    colour text COLLATE "C",
    -- Three values in each block, none shared between blocks: the table's
    -- distinct set is the union, larger than any one block's.
    tint text COLLATE "C",
    -- Three hundred values, more than a dictionary holds in any block.
    wide text COLLATE "C",
    -- Two values in the first two blocks, five hundred in the third.
    mixed text COLLATE "C",
    -- Five values and NULLs.
    small smallint,
    flag boolean,
    -- Three instants written from three offsets, two of them the same
    -- instant: the dump renders both in its session's zone.
    stamp timestamptz,
    -- 'a' and 'a ' pad to one value.
    padded character(4) COLLATE "C",
    -- 'a' and 'a ' stay two texts, which PostgreSQL calls equal.
    bare bpchar COLLATE "C",
    -- Integers whose sum passes their own type's range: `i2` and `i4` fit
    -- in the widened sum, `big` wraps it.
    i2 smallint,
    i4 integer,
    big bigint,
    ident oid,
    amount numeric(12, 2),
    -- Values whose sum passes what a `Decimal128` holds.
    huge numeric(38, 0)
) PARTITION BY LIST (part);
CREATE TABLE public.spans_1 PARTITION OF public.spans FOR VALUES IN (1);
CREATE TABLE public.spans_2 PARTITION OF public.spans FOR VALUES IN (2);
CREATE TABLE public.spans_3 PARTITION OF public.spans FOR VALUES IN (3);
INSERT INTO public.spans
SELECT
    (i - 1) / 500 + 1,
    i,
    1501 - i,
    (i - 1) % 500,
    CASE WHEN i = 700 THEN NULL ELSE i END,
    'k' || lpad(i::text, 4, '0'),
    (ARRAY['amber', 'blue', 'cyan', 'dusk'])[i % 4 + 1],
    't' || ((i - 1) / 500 + 1) || '-' || i % 3,
    'w' || lpad((i % 300)::text, 3, '0'),
    CASE WHEN i > 1000 THEN 'm' || lpad(i::text, 4, '0') ELSE 'm' || i % 2 END,
    CASE WHEN i % 11 = 0 THEN NULL ELSE (i % 5)::smallint END,
    i % 2 = 0,
    (ARRAY['2026-01-01 00:00:00+00', '2026-01-01 05:00:00+05',
           '2026-07-01 12:00:00-07'])[i % 3 + 1]::timestamptz,
    (ARRAY['a', 'a ', 'b'])[i % 3 + 1],
    (ARRAY['a', 'a ', 'b'])[i % 3 + 1]::bpchar,
    (30000 + i % 700)::smallint,
    2000000000 - i,
    9000000000000000000 - i,
    (4000000000 + i)::oid,
    (i * 1.25)::numeric(12, 2),
    (9 * 10::numeric ^ 37 + i)::numeric(38, 0)
FROM generate_series(1, 1500) AS i;

-- Both zeros at a float column's extremes, in both orders of appearance.
-- PostgreSQL calls them equal, so a bound gathered in its order keeps the one
-- seen first; Arrow's `total_cmp` puts `-0` below `0`.
CREATE TABLE public.zeros (
    id integer,
    -- The minimum is a zero, `0` seen first.
    min_pos_first double precision,
    -- The minimum is a zero, `-0` seen first.
    min_neg_first double precision,
    -- The maximum is a zero, `-0` seen first.
    max_neg_first double precision,
    -- The maximum is a zero, `0` seen first.
    max_pos_first double precision,
    r_min_pos_first real,
    r_max_neg_first real
);
INSERT INTO public.zeros VALUES
    (1, 0, '-0', -1, 0, 0, -1),
    (2, '-0', 0, '-0', -1, '-0', '-0'),
    (3, 1, 1, 0, '-0', 1, 0);

-- A text minimum longer than a stored value may be, so its stored lower bound
-- is a clipped prefix rather than a value the column holds.
CREATE TABLE public.long_min (
    id integer,
    v text COLLATE "C",
    vc varchar(400) COLLATE "C"
);
INSERT INTO public.long_min VALUES
    (1, 'b-short', 'b-short'),
    (2, repeat('a', 300), repeat('a', 300)),
    (3, 'c-short', 'c-short');

-- An enum whose labels' text order is the reverse of their declared order.
-- Arrow orders an enum, which it reads as a dictionary, by label text, so
-- `m` is ascending there and descending in PostgreSQL's order: the column a
-- declared ordering over an enum is read against.
CREATE TYPE public.mood AS ENUM ('sad', 'ok', 'happy');
CREATE TABLE public.moods (
    id integer,
    -- 'happy' < 'ok' < 'sad' bytewise, three rows each.
    m public.mood
);
INSERT INTO public.moods
SELECT i, (ARRAY['happy', 'ok', 'sad']::public.mood[])[(i - 1) / 3 + 1]
FROM generate_series(1, 9) AS i;
