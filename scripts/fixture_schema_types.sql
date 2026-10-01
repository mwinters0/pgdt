-- Type-mapping fixture schema: one table per mappable type family, plus the
-- boundary values docs/design/decisions.md, "D73" calls for.
--
-- Deliberately NOT koji-derived: koji contains no numeric, date, uuid, bytea,
-- or interval columns and no user-defined type at all, so this is the only
-- fixture coverage those type-mapping rows have. Structural/escaping edge
-- cases are scripts/fixture_schema_edge_cases.sql's job, not this file's --
-- see that file's header for why they're kept apart.

CREATE TYPE public.mood AS ENUM ('sad', 'ok', 'happy', 'has space', 'has,comma', 'has''quote');

-- An enum with no labels at all. Legal SQL, and the one CREATE TYPE form that
-- carries no information a column could ever be decoded from: only SQL NULL
-- fits such a column. It exists so the EmptyEnum resolution outcome has a real
-- pg_dump value behind it (roadmap.md, "Expand the generated fixtures freely")
-- -- pg_dump writes it `CREATE TYPE public.empty_enum AS ENUM (\n);`, an empty
-- label list spread over two lines, which is also the grammar's empty-body case.
CREATE TYPE public.empty_enum AS ENUM ();

-- Domain over a domain, with a NOT NULL refinement on the outer one.
CREATE DOMAIN public.base_domain AS integer;
CREATE DOMAIN public.derived_domain AS public.base_domain NOT NULL;

CREATE TABLE public.t_int (
    id integer PRIMARY KEY,
    v_smallint smallint,
    v_integer integer,
    v_bigint bigint
);
INSERT INTO public.t_int VALUES
    (1, -32768, -2147483648, -9223372036854775808),
    (2, 32767, 2147483647, 9223372036854775807),
    (3, 0, 0, 0),
    (4, NULL, NULL, NULL);

-- `oid` is PostgreSQL's one unsigned integer type, and the values are chosen
-- for the boundary a signed reading gets wrong: 2147483648 is 2^31, the first
-- OID an Int32 turns negative, and 4294967295 is the type's maximum. `oidout`
-- is `snprintf("%u")`, so the file holds all four as plain digits.
CREATE TABLE public.t_oid (
    id integer PRIMARY KEY,
    v_oid oid
);
INSERT INTO public.t_oid VALUES
    (1, 0),
    (2, 2147483647),
    (3, 2147483648),
    (4, 4294967295),
    (5, NULL);

-- `int2vector` is a pg_catalog type that is not an array type and is not
-- reached by the array recursion: that recursion is a spelling test over the
-- declared name (I28) and `int2vector` carries no array spelling, so this
-- table sits with the other scalar families rather than with t_array* -- the
-- census says nothing about it and never will. It is here because the ADBC
-- floor answers `list<item: int16>` for it, which is a floor row we are below
-- until the type is mapped.
--
-- `int2vectorout` writes the elements space-separated with no quoting, no
-- escaping and no NULL element possible, so the four non-NULL values below are
-- the whole of the grammar. Row 2 is the value that matters most: an empty
-- vector is legal, `int2vectorout` writes it as the empty string, and COPY
-- TEXT writes an empty field for it -- so `\N` and an empty list are still
-- distinguishable in the file, which is what lets an empty vector decode to an
-- empty list rather than to NULL. Rows 3 and 4 are the int16 bounds
-- `int2vectorin` checks against SHRT_MIN/SHRT_MAX and a single element.
CREATE TABLE public.t_int2vector (
    id integer PRIMARY KEY,
    v_vec int2vector
);
INSERT INTO public.t_int2vector VALUES
    (1, '1 2 3'),
    (2, ''),
    (3, '-32768 32767'),
    (4, '0'),
    (5, NULL);

-- v_typed is exactly the Decimal128 boundary (38 total digits); v_typed39
-- crosses it into Decimal256. v_small carries a typmod but still accepts
-- NaN -- PostgreSQL's precision/scale check does not apply to NaN, so NaN is
-- reachable through *any* numeric column, typed or not.
CREATE TABLE public.t_numeric (
    id integer PRIMARY KEY,
    v_typed numeric(38,10),
    v_typed39 numeric(39,10),
    v_small numeric(10,2),
    v_untyped numeric
);
INSERT INTO public.t_numeric VALUES
    (1, 1234567890123456789012345678.1234567890, NULL, NULL, NULL),
    (2, NULL, 123456789.1234567890, NULL, NULL),
    (3, NULL, NULL, 'NaN', 'NaN'),
    (4, 0, 0, 0, 0),
    (5, -1.5000000000, -1.5000000000, -1.50, 100.00),
    (6, NULL, NULL, NULL, 12345.6789),
    (7, NULL, NULL, NULL, NULL);

CREATE TABLE public.t_float (
    id integer PRIMARY KEY,
    v_real real,
    v_double double precision
);
INSERT INTO public.t_float VALUES
    (1, 'NaN', 'NaN'),
    (2, 'Infinity', 'Infinity'),
    (3, '-Infinity', '-Infinity'),
    (4, '-0.0', '-0.0'),
    (5, 1.1754944e-38, 2.2250738585072014e-308),
    (6, 3.14159274, 3.14159265358979),
    (7, NULL, NULL);

-- infinity/-infinity are PostgreSQL date pseudo-values with no numeric day
-- offset at all, so they have no Date32 representation and decode as a
-- FieldDecode by design (docs/design/decisions.md, "D42").
CREATE TABLE public.t_date (
    id integer PRIMARY KEY,
    v_date date
);
INSERT INTO public.t_date VALUES
    (1, 'infinity'),
    (2, '-infinity'),
    (3, '0001-01-01'),
    (4, '9999-12-31'),
    (5, '0044-01-01 BC'),
    (6, '10000-01-01'),
    (7, NULL);

CREATE TABLE public.t_timestamp (
    id integer PRIMARY KEY,
    v_ts timestamp without time zone,
    v_tstz timestamp with time zone
);
INSERT INTO public.t_timestamp VALUES
    (1, 'infinity', 'infinity'),
    (2, '-infinity', '-infinity'),
    (3, '2024-01-01 00:00:00', '2024-01-01 00:00:00+05:30'),
    (4, '2024-01-01 00:00:00.123456', '2024-01-01 00:00:00.123456+00'),
    (5, '0001-01-01 00:00:00', '0001-01-01 00:00:00+00'),
    (6, '0044-01-01 00:00:00 BC', '0044-01-01 00:00:00 BC+00'),
    (7, '294276-12-31 23:59:59.999999', '294276-12-31 23:59:59.999999+00'),
    (8, NULL, NULL);

CREATE TABLE public.t_time (
    id integer PRIMARY KEY,
    v_time time without time zone,
    v_timetz time with time zone
);
INSERT INTO public.t_time VALUES
    (1, '24:00:00', '24:00:00+00'),
    (2, '00:00:00.000001', '00:00:00.000001-05'),
    (3, NULL, NULL);

CREATE TABLE public.t_interval (
    id integer PRIMARY KEY,
    v_interval interval
);
INSERT INTO public.t_interval VALUES
    (1, '1 year 2 months 3 days 04:05:06'),
    (2, '-1 day'),
    (3, '0'),
    (4, '1.5 hours'),
    (5, NULL);

-- The spellings `format_type` writes around a typmod -- mid-name before a
-- zone, after an `interval` field qualifier -- and the one internal name it
-- writes, `bpchar`, for a `character` column with no typmod. Each is read as
-- the type the grammar reads it as (roadmap.md, "The input contract is valid
-- PostgreSQL").
CREATE TABLE public.t_type_spelling (
    id integer PRIMARY KEY,
    v_ts3 timestamp(3) without time zone,
    v_tstz0 timestamp(0) with time zone,
    v_time3 time(3) without time zone,
    v_timetz2 time(2) with time zone,
    v_ym interval year to month,
    v_ds2 interval day to second(2),
    v_bpchar bpchar
);
INSERT INTO public.t_type_spelling VALUES
    (1, '2024-01-01 00:00:00.123456', '2024-01-01 00:00:00.6+00', '12:34:56.7891',
        '12:34:56.789+02', '1 year 2 months', '3 days 04:05:06.789', 'ab  '),
    (2, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

CREATE TABLE public.t_bytea (
    id integer PRIMARY KEY,
    v_bytea bytea
);
INSERT INTO public.t_bytea VALUES
    (1, ''),
    (2, NULL),
    (3, '\xdeadbeef00ff'),
    (4, E'\\x5c6261636b736c617368'::bytea);

CREATE TABLE public.t_uuid (
    id integer PRIMARY KEY,
    v_uuid uuid
);
INSERT INTO public.t_uuid VALUES
    (1, '00000000-0000-0000-0000-000000000000'),
    (2, 'A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11'),
    (3, NULL);

CREATE TABLE public.t_text (
    id integer PRIMARY KEY,
    v_text text,
    v_varchar varchar(10),
    v_char char(10)
);
INSERT INTO public.t_text VALUES
    (1, '', '', ''),
    (2, NULL, NULL, NULL),
    (3, 'hello', 'hello', 'hi');

-- The COLLATE clause. Every column below is Utf8View and every one is compared
-- bytewise; what the clause moves is the *verdict* -- whether bytewise is
-- PostgreSQL's own answer for that column (docs/design/decisions.md,
-- "D55"). t_text, above, carries the divergent
-- half of that rule; this table is the agreeing half, which nothing in the
-- tree had until it existed.
--
-- pg_dump writes a clause only where the column's collation differs from its
-- type's own default (I37), so the two silent columns here are silent for two
-- different reasons: `name`'s type default *is* C, and v_domain_c's type
-- default is the domain's own COLLATE "C", which text_c's DDL carries instead.
-- Do not "fix" either into an explicit clause: the absence is the fact under
-- test.
--
-- en_US.utf8 is the one genuinely non-bytewise collation available at all six
-- majors without generating a locale -- it is also the database's own
-- collation (fixtures/<v>/oracle/meta.tsv, datcollate), and naming it
-- explicitly still emits a clause, because pg_dump compares collation OIDs
-- rather than semantics. ucs_basic earns its column for the opposite reason:
-- it is collcollate = C, bytewise in fact, and not named C, so the register
-- must call it divergent.
--
-- ICU stays out of the *comparison* columns, and out of the oracle: `unicode`
-- and the *-x-icu family carry a collversion that moves with the ICU release,
-- which is an apparatus key guaranteed to drift. That exclusion is about
-- answers, and it does not reach the one ICU shape below whose fact is a
-- statement rather than an order -- see nd_collation.
CREATE DOMAIN public.text_c AS text COLLATE "C";

-- I37's third emission site, dumpCompositeType: a per-attribute COLLATE inside
-- a CREATE TYPE, which nothing else in the tree reaches. Nothing reads a
-- field's collation yet -- a nested column is refused a step earlier, on the
-- NestedPlan -- and the nested structural comparison slice is what will want
-- it.
CREATE TYPE public.collated_pair AS (plain text, c text COLLATE "C");

-- A user-defined collation, so that the reference form pg_dump writes for one
-- -- schema-qualified, unquoted, outside pg_catalog -- exists in a real dump
-- rather than in a unit test's guess. It is `FROM "C"`, and the dump says so
-- (`locale = 'C'`), which is the point: the register still answers
-- NonBytewiseCollation for v_user below, because nothing stops a user defining
-- a non-bytewise collation called "C" in their own schema and a wrong "agrees"
-- is the one error it must not make. Do not "fix" that into agreement.
CREATE COLLATION public.c_collation FROM "C";

-- The one ICU collation in the tree, and the only one whose dump text states
-- something pgdt acts on: `deterministic = false` is emitted unconditionally
-- wherever the catalog says so (I42), so v_nd below is a column whose *equality*
-- is knowably not a byte comparison. A non-deterministic collation is ICU-only
-- -- the server refuses the option for every other provider -- so this shape
-- cannot be written any other way.
--
-- The drift objection that keeps ICU out of the comparison columns does not
-- reach it, because the drift is guarded rather than avoided: `version =` is
-- written only under --binary-upgrade, so the collversion the server computed
-- reaches exactly one flag set, and tests/preamble.rs's
-- the_icu_collversion_reaches_binary_upgrade_alone_and_agrees_across_majors
-- asserts that shape plus six-way agreement on the value without naming it --
-- so an image bump is a regeneration and a split between majors is a fault.
-- All six majors write this statement byte for byte alike, including the
-- option order -- provider, determinism, locale -- which is dumpCollation's
-- own append order and not this file's.
--
-- `und` is ICU's root locale, so it exists at every ICU version without a
-- locale being generated, and it is deliberately *not* asked as an oracle case:
-- the oracle builds its own temp tables per case, so a t_collate column obliges
-- none, and adding one would import exactly the drift this exclusion avoids.
CREATE COLLATION public.nd_collation (provider = icu, locale = 'und', deterministic = false);

-- The COLLATE clause is written *after* DEFAULT/GENERATED and after NOT NULL
-- in a table column (I37), wherever it was written in the input -- which is
-- why extract_collation scans the whole fragment instead of looking at the
-- token after the type. Every clause below is written in canonical input
-- position, directly after the type; the dump displaces the three that have a
-- constraint behind them, and that displacement is what these columns exist to
-- put in committed bytes.
CREATE TABLE public.t_collate (
    id integer PRIMARY KEY,
    v_text_c text COLLATE "C",
    v_text_locale text COLLATE "en_US.utf8",
    v_text_ucs text COLLATE "ucs_basic",
    v_name name,
    v_domain_c public.text_c,
    v_pair public.collated_pair,
    v_text_def text COLLATE "C" DEFAULT 'x',
    v_user text COLLATE public.c_collation,
    -- The non-deterministic column. It is the only one in the tree whose
    -- divergence reaches `=`: every other collated column here is libc and so
    -- deterministic, which makes texteq a byte comparison whatever the order
    -- is. The alphabet it carries is the same one, so the row set is bytewise
    -- like every sibling and only the note separates them.
    v_nd text COLLATE public.nd_collation,
    v_src text,
    -- All three displacers in one fragment, at no cost to the alphabet: a
    -- STORED generated column is excluded from the COPY column list, so no row
    -- has to hold a value for it and NOT NULL costs nothing. It is also the
    -- only fixture stress on extract_collation's paren- and quote-aware scan,
    -- the other generated fixture column being an integer with nothing after
    -- its expression. COALESCE is load-bearing, not decoration: v_src carries
    -- the alphabet including its NULL row, and upper(NULL) would violate the
    -- NOT NULL -- it also nests the parens one deeper and puts a quoted
    -- literal inside them, which is exactly what the scan must step over.
    v_gen_nn text COLLATE "C" GENERATED ALWAYS AS (upper(COALESCE(v_src, ''))) STORED NOT NULL
);

-- One alphabet, replicated across every column that carries data, so a filter
-- over two of them differs only by the collation. v_gen_nn is the sole
-- exception and carries none: it is generated, so the server writes it and the
-- dump omits it from COPY entirely.
--
-- These are the values the comparison oracle
-- already answers on: A/a and a/B diverge because case is a lower-weight
-- difference than letter, é/f because an accent sorts with its base letter,
-- and _x/ax because punctuation is ignored at the primary level -- all on
-- glibc 2.41. Do not "fix" them into placeholders: a set that could not
-- separate the two orders would pass every note-level assertion and support no
-- stronger one.
--
-- The column list is explicit because v_gen_nn is generated: a positional
-- VALUES would try to supply it a value, which PostgreSQL refuses.
INSERT INTO public.t_collate
    (id, v_text_c, v_text_locale, v_text_ucs, v_name, v_domain_c, v_pair,
     v_text_def, v_user, v_nd, v_src)
VALUES
    (1, 'A', 'A', 'A', 'A', 'A', ROW('A', 'A')::public.collated_pair, 'A', 'A', 'A', 'A'),
    (2, 'a', 'a', 'a', 'a', 'a', ROW('a', 'a')::public.collated_pair, 'a', 'a', 'a', 'a'),
    (3, 'B', 'B', 'B', 'B', 'B', ROW('B', 'B')::public.collated_pair, 'B', 'B', 'B', 'B'),
    (4, 'é', 'é', 'é', 'é', 'é', ROW('é', 'é')::public.collated_pair, 'é', 'é', 'é', 'é'),
    (5, 'f', 'f', 'f', 'f', 'f', ROW('f', 'f')::public.collated_pair, 'f', 'f', 'f', 'f'),
    (6, '_x', '_x', '_x', '_x', '_x', ROW('_x', '_x')::public.collated_pair, '_x', '_x', '_x', '_x'),
    (7, 'ax', 'ax', 'ax', 'ax', 'ax', ROW('ax', 'ax')::public.collated_pair, 'ax', 'ax', 'ax', 'ax'),
    (8, '', '', '', '', '', ROW('', '')::public.collated_pair, '', '', '', ''),
    (9, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

CREATE TABLE public.t_json (
    id integer PRIMARY KEY,
    v_json json,
    v_jsonb jsonb
);
INSERT INTO public.t_json VALUES
    (1, '{"a":1,"b":[1,2,3]}', '{"a":1,"b":[1,2,3]}'),
    (2, 'null', 'null'),
    (3, NULL, NULL);

CREATE TABLE public.t_net (
    id integer PRIMARY KEY,
    v_inet inet,
    v_cidr cidr,
    v_macaddr macaddr,
    v_macaddr8 macaddr8
);
INSERT INTO public.t_net VALUES
    (1, '192.168.1.1', '192.168.1.0/24', '08:00:2b:01:02:03', '08:00:2b:01:02:03:04:05'),
    (2, '::1', '::/0', NULL, NULL),
    (3, NULL, NULL, NULL, NULL);

CREATE TABLE public.t_enum_domain (
    id integer PRIMARY KEY,
    v_mood public.mood,
    v_domain public.derived_domain,
    v_empty_enum public.empty_enum
);
INSERT INTO public.t_enum_domain VALUES
    (1, 'sad', 5, NULL),
    (2, 'has space', 0, NULL),
    (3, 'has,comma', -5, NULL),
    (4, 'has''quote', 100, NULL);

-- Every array column here is one-dimensional with lower bound 1, so the whole
-- table decodes on the optimistic (no-census) array path; the shapes that
-- vary live in t_array_shape, below. {}, {NULL} and a NULL
-- array are three distinct values that look similar and are the classic way an
-- array decoder goes wrong; v_enum_array is array_out's quoting rules (I20)
-- applied to element text that contains a space, a comma and a quote.
CREATE TABLE public.t_array (
    id integer PRIMARY KEY,
    v_empty integer[],
    v_with_null integer[],
    v_null_array integer[],
    v_text_special text[],
    v_enum_array public.mood[]
);
INSERT INTO public.t_array VALUES
    (1, '{}', '{NULL}', NULL, ARRAY['a,b', 'c{d}', 'e"f', 'g\h'],
        ARRAY['sad', 'has space', 'has,comma', 'has''quote']::public.mood[]),
    (2, '{1,2,3}', '{1,NULL,3}', '{1,2}', ARRAY[NULL, 'plain'],
        ARRAY[NULL, 'ok']::public.mood[]);

-- Array *shape* varies per value, never per column (I21) -- this table is the
-- input to the shape census, not to the decoder. v_multidim is uniformly 2-D,
-- v_mixed_dim disagrees between two rows of one column, and v_lbound carries
-- array_out's `[lb:ub]=` decoration (I20). All three are declared `integer[]`
-- in the dump whatever the DDL said, which is the whole point.
--
-- Do not "fix" these into plain 1-D values. Each is a deliberate
-- FieldDecode on the optimistic path, and with a census v_mixed_dim and
-- v_lbound are a deliberate degradation back to text.
CREATE TABLE public.t_array_shape (
    id integer PRIMARY KEY,
    v_multidim integer[][],
    v_mixed_dim integer[],
    v_lbound integer[]
);
INSERT INTO public.t_array_shape VALUES
    (1, '{{1,2},{3,4}}', '{1,2}', '[0:2]={7,8,9}'),
    (2, NULL, '{{1,2},{3,4}}', '[-1:0]={10,11}'),
    (3, '{{5,6},{7,8}}', NULL, NULL);

-- The array-declaration spellings pg_dump never writes. PostgreSQL accepts
-- six ways of declaring an array-typed column and every one is the same type
-- (I28): the bounds and the dimension count are discarded by the parser, so
-- format_type -- and therefore pg_dump -- writes all six back as `integer[]`.
-- These four columns are the four spellings no other fixture covers, and the
-- prediction this table carries is that the dumped DDL reads `integer[]` for
-- every one of them, on every major. That collapse is the repo's own proof of
-- I28, in checked-in bytes; the spellings themselves survive only here, in the
-- SQL the generator loads.
--
-- Deliberately NOT columns of t_array_shape. Every value here is an ordinary
-- one-dimensional array with lower bound 1, so the census has nothing to say
-- about them; t_array_shape is read as "the shapes the census reports", which
-- is the same reason t_nested_array is its own table too.
--
-- Do not "fix" a spelling into `integer[]`. The declaration is the whole
-- content of the table, and a bound constrains nothing about the values --
-- v_bounded is declared `integer[3]` and row 1 holds four elements, which
-- PostgreSQL accepts (I28).
CREATE TABLE public.t_array_spelling (
    id integer PRIMARY KEY,
    v_bounded integer[3],
    v_bounded_2d integer[3][4],
    v_array_kw integer ARRAY,
    v_array_kw_n integer ARRAY[4]
);
INSERT INTO public.t_array_spelling VALUES
    (1, '{1,2,3,4}', '{5,6}', '{7,8}', '{9}'),
    (2, '{}', NULL, '{NULL,10}', '{}'),
    (3, NULL, NULL, NULL, NULL);

CREATE TYPE public.point2d AS (x integer, y text);

-- A composite with an array field: record_out's doubling convention wrapped
-- around array_out's backslash convention. Together with t_composite's
-- v_points (an array of composites) this is I20's both-escape-conventions-at-
-- once case in both nesting orders -- the one shape a single-convention
-- decoder passes every other fixture value on and still gets wrong.
CREATE TYPE public.tagged AS (label text, tags text[]);

-- A composite with no fields at all. Legal SQL, and record_out writes it `()`
-- -- which is also exactly what a one-field composite holding SQL NULL writes,
-- so the literal alone cannot say which it is. The declared field list is the
-- only discriminator, and this column is what proves the zero-field side of
-- that ambiguity is a real value rather than a curiosity.
CREATE TYPE public.empty_comp AS ();

CREATE TABLE public.t_composite (
    id integer PRIMARY KEY,
    v_point public.point2d,
    v_points public.point2d[],
    v_tagged public.tagged,
    v_empty_comp public.empty_comp
);
INSERT INTO public.t_composite VALUES
    (1, ROW(1, 'a,b"c'),
        ARRAY[ROW(1, 'a,b"c')::public.point2d, ROW(2, 'plain')::public.point2d],
        ROW('a,b', ARRAY['x"y', 'p q', NULL])::public.tagged,
        '()'),
    (2, NULL, NULL, NULL, NULL),
    (3, ROW(NULL, ''),
        ARRAY[NULL::public.point2d, ROW(3, NULL)::public.point2d],
        ROW('', ARRAY[]::text[])::public.tagged,
        '()');

-- `KD2`'s shape: a two-dimensional array in a composite field, which PostgreSQL
-- accepts for `text[]` whatever its declared dimensions. The census is keyed by
-- column, so this field stays on the one-dimensional path and row 6 fails to
-- decode wherever a query emits it; a filter comparing it reads it structurally
-- and answers. A table of its own, so every other composite reads whole; the
-- generated pruning check's error leg reads it (pgdump_query/tests/pruning.rs,
-- `check`), with rows either side for its terms over `id` to prune around.
CREATE TABLE public.t_composite_matrix (
    id integer PRIMARY KEY,
    v_tagged public.tagged
);
INSERT INTO public.t_composite_matrix VALUES
    (1, ROW('a', ARRAY['x', 'y'])::public.tagged),
    (2, ROW('b', ARRAY['p'])::public.tagged),
    (3, NULL),
    (4, ROW('c', ARRAY['q', NULL])::public.tagged),
    (5, ROW('d', ARRAY[]::text[])::public.tagged),
    (6, ROW('m', ARRAY[['a', 'b'], ['c', 'd']])::public.tagged),
    (7, ROW('e', ARRAY['r'])::public.tagged),
    (8, NULL),
    (9, ROW('f', ARRAY['s', 't'])::public.tagged),
    (10, ROW('g', ARRAY['u'])::public.tagged),
    (11, ROW('h', NULL)::public.tagged);

CREATE TABLE public.t_range (
    id integer PRIMARY KEY,
    v_range int4range
);
INSERT INTO public.t_range VALUES
    (1, '[1,10)'),
    (2, 'empty'),
    (3, '(,5)'),
    (4, NULL);

-- Base type and shell type: reachable from pure SQL, no compiled extension
-- needed (postgres-invariants.md I11). pg_dump emits mybase TWICE under one
-- name (a SHELL TYPE entry, then the completed TYPE entry); shellonly is
-- never completed, so it emits once, also under Type: TYPE (not SHELL TYPE
-- -- that description is reserved for a base type's first half).
CREATE TYPE public.shellonly;

CREATE TYPE public.mybase;
CREATE FUNCTION public.mybase_in(cstring) RETURNS public.mybase
    AS 'textin' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION public.mybase_out(public.mybase) RETURNS cstring
    AS 'textout' LANGUAGE internal IMMUTABLE STRICT;
CREATE TYPE public.mybase (INPUT = public.mybase_in,
    OUTPUT = public.mybase_out, INTERNALLENGTH = VARIABLE,
    STORAGE = extended);

-- v_mybase_array is the element type this project declines to decode: a
-- TypeKind::Base element carries its own typdelim, so splitting its array on
-- a hardcoded `,` would be a guess. The column exists so that refusal has a
-- real pg_dump value to be tested against, not so it eventually resolves.
CREATE TABLE public.t_base_type (
    id integer PRIMARY KEY,
    v_mybase public.mybase,
    v_mybase_array public.mybase[]
);
INSERT INTO public.t_base_type VALUES
    (1, 'hello', '{hello,"a,b"}'),
    (2, NULL, NULL);

-- The delimiter trap in disguise (I22). `box` is the only built-in whose
-- typdelim is `;` rather than `,`, and a domain over it inherits that
-- delimiter while its own DDL records nothing about it -- `CREATE DOMAIN
-- public.box_domain AS box;` is the whole trace. So v_box_domain_array's
-- literal is semicolon-separated with commas *inside* every element, and a
-- decoder splitting it on a hardcoded `,` produces seven element boundaries
-- where the value has two -- silently, since the wrong split still re-renders
-- byte-for-byte. This is the case an array-element refusal that tests the
-- declared spelling (`box`, TypeKind::Base) rather than the domain walk's
-- terminal gets wrong; v_box_domain is the same type undisguised, one level
-- up, where nothing is at stake because a scalar box is a string either way.
--
-- Do not "fix" this into a comma-separated array. There is no such value:
-- PostgreSQL writes what the element type's typdelim says.
CREATE DOMAIN public.box_domain AS box;

CREATE TABLE public.t_delimiter (
    id integer PRIMARY KEY,
    v_box_domain public.box_domain,
    v_box_domain_array public.box_domain[]
);
INSERT INTO public.t_delimiter VALUES
    (1, '(1,1),(0,0)', '{"(1,1),(0,0)";"(3,3),(2,2)"}'),
    (2, NULL, NULL);

-- User-defined range type: pg_dump has never been observed emitting this
-- grammar for real -- the existing coverage (t_range, above) is a built-in
-- range, and the parser's range grammar was otherwise tested only against
-- hand-written single-line `subtype = int4` text. Real output is multi-line
-- with one parameter per line, and the subtype here is deliberately a
-- multi-word type name. PG14+ auto-creates a companion multirange type and
-- lets it be named explicitly via `multirange_type_name`; on 13 that
-- parameter doesn't exist at all (I10).
SELECT current_setting('server_version_num')::int >= 140000 AS has_multirange \gset
\if :has_multirange
CREATE TYPE public.myrange AS RANGE (
    subtype = double precision,
    multirange_type_name = public.myrange_multi
);
\else
CREATE TYPE public.myrange AS RANGE (
    subtype = double precision
);
\endif

CREATE TABLE public.t_user_range (
    id integer PRIMARY KEY,
    v_myrange public.myrange
);
INSERT INTO public.t_user_range VALUES
    (1, '[1.5,10.5)'),
    (2, 'empty'),
    (3, NULL);

-- The only range in the tree whose bounds ever need quoting: int4range,
-- myrange and the multiranges all have numeric bounds, so range_bound_escape's
-- doubling convention (I20 -- `"` -> `""`, unlike an array's `\"`) appears
-- nowhere else. An absent bound and an empty-string bound are both "nothing
-- visible between the separators" unless one of them is quoted, which is the
-- pair a range decoder most easily conflates.
--
-- `collation` is pinned to C rather than left to default: pg_dump emits the
-- clause only when the range's collation differs from the subtype's default,
-- so pinning it both fixes the emitted text across container locales and
-- gives the range grammar a qualified, quoted parameter value to parse.
CREATE TYPE public.textrange AS RANGE (
    subtype = text,
    collation = pg_catalog."C"
);

CREATE TABLE public.t_text_range (
    id integer PRIMARY KEY,
    v_textrange public.textrange
);
INSERT INTO public.t_text_range VALUES
    (1, '["a,b","c""d")'),
    (2, '[" lead","trail ")'),
    (3, '["","a")'),
    (4, '(,"z")'),
    (5, 'empty'),
    (6, NULL);

-- Multirange types (PG14+). The six built-ins appear bare, exactly like the
-- six built-in range types; `myrange`'s auto-created companion has no
-- `CREATE TYPE` of its own anywhere in the dump -- its only trace is the
-- `multirange_type_name` parameter above (I10), which is exactly what makes
-- a column declared with that companion name unrecoverable without it.
\if :has_multirange
CREATE TABLE public.t_multirange (
    id integer PRIMARY KEY,
    v_int4multirange int4multirange,
    v_myrange_multi public.myrange_multi
);
INSERT INTO public.t_multirange VALUES
    (1, '{[1,10)}', '{[1.5,10.5)}'),
    (2, '{}', '{}'),
    (3, NULL, NULL);
\endif

-- Nesting that goes through more than one hop. Two of these columns are a
-- *refusal* we make at resolution; the other five work and were pinned by
-- nothing until this table existed (roadmap.md, "Expand the generated
-- fixtures freely; never infer what pg_dump writes").
--
-- v_nested_array is the shape whose literal depth and resolved type depth
-- disagree (I26). `public.intarr[]` is an array whose *element* is an array,
-- and array_out writes it ONE brace deep -- `{"{1,2}","{3}"}` -- because it
-- force-quotes any element whose text contains `{` (I25). Its census entry is
-- therefore (1, 1): correct, and the one place where reading it as an ordinary
-- one-dimensional array is exactly the mistake. That is why this column is here
-- and not on t_array_shape, which is read as "the shapes the census reports".
-- v_arr_holder is the same shape reached through a composite field, where the
-- refusal has to compose rather than be handled again.
--
-- Do not "fix" either into a plain array. There is no such value: pg_dump
-- writes what array_out writes, and the point of the columns is that we
-- decline the shape instead of mis-decoding it.
CREATE DOMAIN public.intarr AS integer[];
CREATE TYPE public.arr_holder AS (label text, arr public.intarr[]);

-- The five that already worked. Each composes two mechanisms the recursion
-- handles separately elsewhere, and "the recursion handles it" is exactly the
-- reasoning that let the two columns above through.
CREATE DOMAIN public.pointdom AS public.point2d;
CREATE TYPE public.boxed_point AS (label text, pt public.point2d);
CREATE DOMAIN public.rangedom AS public.myrange;

CREATE TABLE public.t_nested_array (
    id integer PRIMARY KEY,
    v_nested_array public.intarr[],
    v_arr_holder public.arr_holder,
    v_pointdom public.pointdom,
    v_pointdom_array public.pointdom[],
    v_boxed_point public.boxed_point,
    v_myrange_array public.myrange[],
    v_rangedom public.rangedom
);
INSERT INTO public.t_nested_array VALUES
    (1, ARRAY['{1,2}'::public.intarr, '{3}'::public.intarr],
        ROW('L', ARRAY['{1,2}'::public.intarr])::public.arr_holder,
        ROW(1, 'a,b"c')::public.point2d,
        ARRAY[ROW(1, 'a,b"c')::public.point2d,
              ROW(2, 'plain')::public.point2d]::public.pointdom[],
        ROW('outer', ROW(3, 'x y')::public.point2d)::public.boxed_point,
        ARRAY['[1.5,10.5)'::public.myrange, 'empty'::public.myrange],
        '[2.5,3.5)'),
    (2, '{"{}","{5,NULL}"}',
        ROW('', NULL)::public.arr_holder,
        ROW(NULL, '')::public.point2d,
        ARRAY[NULL::public.point2d]::public.pointdom[],
        ROW(NULL, NULL)::public.boxed_point,
        ARRAY[NULL::public.myrange],
        NULL),
    (3, NULL, NULL, NULL, NULL, NULL, NULL, NULL);

-- The extremes: PostgreSQL's least and greatest value of every type
-- `builtin_scalar` maps to an Arrow type other than `Utf8View`, one column per
-- such arm, and the special values each admits. Each must decode to a value
-- its Arrow type holds or be counted unrepresentable, and which is decided by
-- DataFusion's own path rather than by a bound written here
-- (docs/design/decisions.md, "D102"); the ones that are not
-- held are recorded in datafusion-pgdump/tests/unrepresentable.rs.
-- `floor_mapping.py` holds the columns to the arms both ways, so a newly
-- typed arm arrives with its extremes.
--
-- Rows are kinds, not types: 1 is each column's least and 2 its greatest,
-- 3 and 4 the negative and positive specials, 5 NaN, and the rest walk an
-- edge from both sides -- `interval`'s time part, `timestamp`'s `i64`, and
-- the calendar `arrow-cast` displays a date through. PostgreSQL keeps no
-- catalog of a type's least and greatest, so each is written by hand: `date`
-- and the timestamps run from Julian day 0 to `datetime.h`'s END_*, and an
-- `interval`'s three fields are taken one at a time -- from 17, all three at
-- their bound at once is the infinity.
--
-- Do not "fix" a value into one Arrow holds: the ones that are not are the
-- evidence.
CREATE TABLE public.t_extremes (
    id integer PRIMARY KEY,
    v_smallint smallint,
    v_integer integer,
    v_bigint bigint,
    v_oid oid,
    v_boolean boolean,
    v_real real,
    v_double double precision,
    v_numeric38 numeric(38,0),
    v_numeric76 numeric(76,0),
    v_date date,
    v_ts timestamp without time zone,
    v_tstz timestamp with time zone,
    v_time time without time zone,
    v_interval interval,
    v_uuid uuid,
    v_bytea bytea,
    v_int2vector int2vector
);
INSERT INTO public.t_extremes VALUES
    (1, -32768, -2147483648, -9223372036854775808, 0, false,
        '-3.4028235e+38', '-1.7976931348623157e+308',
        -99999999999999999999999999999999999999,
        -9999999999999999999999999999999999999999999999999999999999999999999999999999,
        '4714-11-24 BC', '4714-11-24 00:00:00 BC', '4714-11-24 00:00:00+00 BC',
        '00:00:00', '-178956970 years -8 months',
        '00000000-0000-0000-0000-000000000000', '', '-32768'),
    (2, 32767, 2147483647, 9223372036854775807, 4294967295, true,
        '3.4028235e+38', '1.7976931348623157e+308',
        99999999999999999999999999999999999999,
        9999999999999999999999999999999999999999999999999999999999999999999999999999,
        '5874897-12-31', '294276-12-31 23:59:59.999999', '294276-12-31 23:59:59.999999+00',
        '24:00:00', '178956970 years 7 months',
        'ffffffff-ffff-ffff-ffff-ffffffffffff', '\xff', '32767'),
    (3, NULL, NULL, NULL, NULL, NULL, '-Infinity', '-Infinity', NULL, NULL,
        '-infinity', '-infinity', '-infinity', NULL, '-2147483648 days', NULL, NULL, NULL),
    (4, NULL, NULL, NULL, NULL, NULL, 'Infinity', 'Infinity', NULL, NULL,
        'infinity', 'infinity', 'infinity', NULL, '2147483647 days', NULL, NULL, NULL),
    (5, NULL, NULL, NULL, NULL, NULL, 'NaN', 'NaN', 'NaN', 'NaN',
        NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);
-- `interval`'s time part: the longest either way a dump can hold, then a
-- microsecond past Arrow's nanosecond `i64` and the last microsecond inside
-- it, either way. The longest is `i64` microseconds from 15; before it
-- `interval2tm` puts the hours in an `int` and `interval_out` refuses past
-- one, so a table can hold a longer value than `pg_dump` can write.
SELECT current_setting('server_version_num')::int >= 150000 AS has_int64_hours \gset
\if :has_int64_hours
INSERT INTO public.t_extremes (id, v_interval) VALUES
    (6, '-2562047788:00:54.775807'),
    (7, '2562047788:00:54.775807');
\else
INSERT INTO public.t_extremes (id, v_interval) VALUES
    (6, '-2147483647:59:59.999999'),
    (7, '2147483647:59:59.999999');
\endif
INSERT INTO public.t_extremes (id, v_interval) VALUES
    (8, '2562047:47:16.854776'),
    (9, '2562047:47:16.854775'),
    (10, '-2562047:47:16.854776'),
    (11, '-2562047:47:16.854775');
-- The last microsecond `Timestamp(Microsecond)` counts from 1970, and the next.
INSERT INTO public.t_extremes (id, v_ts, v_tstz) VALUES
    (12, '294247-01-10 04:00:54.775807', '294247-01-10 04:00:54.775807+00'),
    (13, '294247-01-10 04:00:54.775808', '294247-01-10 04:00:54.775808+00');
-- The last day `arrow-cast` can display, and the next: it formats a date
-- through `chrono`, whose calendar ends before `Date32`'s `i32` does.
INSERT INTO public.t_extremes (id, v_date, v_ts, v_tstz) VALUES
    (14, '262142-12-31', '262142-12-31 23:59:59.999999', '262142-12-31 23:59:59.999999+00'),
    (15, '262143-01-01', '262143-01-01 00:00:00', '262143-01-01 00:00:00+00');
INSERT INTO public.t_extremes (id) VALUES (16);
-- PostgreSQL 17 admits an infinite interval, every field at its bound.
SELECT current_setting('server_version_num')::int >= 170000 AS has_interval_infinity \gset
\if :has_interval_infinity
INSERT INTO public.t_extremes (id, v_interval) VALUES
    (17, '-infinity'),
    (18, 'infinity');
\endif

-- A nested value holding one is one (docs/design/decisions.md, "D96"): an
-- array element, a range bound, a composite's field. Row 2 is
-- the same shapes holding none, and its composite's `text` field reads
-- `infinity` beside a finite `date` -- a leaf is its own type's, so that one
-- is text and counts for nothing.
CREATE TYPE public.dated AS (label text, d date);

CREATE TABLE public.t_extremes_nested (
    id integer PRIMARY KEY,
    v_date_array date[],
    v_daterange daterange,
    v_dated public.dated,
    v_interval_array interval[]
);
INSERT INTO public.t_extremes_nested VALUES
    (1, '{2024-01-01,infinity}', '[2024-01-01,infinity)',
        ROW('x', 'infinity')::public.dated, '{"1 day","2562047:47:16.854776"}'),
    (2, '{2024-01-01}', '[2024-01-01,2024-02-01)',
        ROW('infinity', '2024-01-01')::public.dated, '{"2562047:47:16.854775"}'),
    (3, NULL, NULL, NULL, NULL);
