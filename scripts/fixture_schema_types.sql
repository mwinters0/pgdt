-- Type-mapping fixture schema: one table per mappable type family, plus the
-- boundary values from docs/design/architecture.md's
-- "Boundary values worth building in" table.
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
-- offset at all -- see the phase 2 notes on what this means for Date32.
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
