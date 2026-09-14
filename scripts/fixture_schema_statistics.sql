-- Per-row-group statistics fixture schema: the column shapes bounds,
-- dictionaries, null counts and block sortedness are gathered from and pruned
-- against (docs/design/roadmap-P10-row-group-statistics.md, "Shapes the phase
-- must hold").
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
-- A value longer than the read chunk and the default statistics group size,
-- both one mebibyte, so a scan carries one row across chunks, a group holds a
-- row running past its end, a small stated group size leaves groups in which
-- no row starts, and a leader cut can land inside the row. Past the stored
-- value cap too, as is the 300-byte value, which is the one a truncated bound
-- is taken from without the megabyte behind it. Ascending bytewise.
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
