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
    v_domain public.derived_domain
);
INSERT INTO public.t_enum_domain VALUES
    (1, 'sad', 5),
    (2, 'has space', 0),
    (3, 'has,comma', -5),
    (4, 'has''quote', 100);

-- Deferred to Phase 3 -- kept here (not in edge_cases) because the boundary
-- values matter to that future decoder and the schema is already set up for
-- it. {}, {NULL} and a NULL array are three distinct values that look
-- similar and are the classic way an array decoder goes wrong.
CREATE TABLE public.t_array (
    id integer PRIMARY KEY,
    v_empty integer[],
    v_with_null integer[],
    v_null_array integer[],
    v_text_special text[],
    v_multidim integer[][]
);
INSERT INTO public.t_array VALUES
    (1, '{}', '{NULL}', NULL, ARRAY['a,b', 'c{d}', 'e"f', 'g\h'], '{{1,2},{3,4}}'),
    (2, '{1,2,3}', '{1,NULL,3}', '{1,2}', ARRAY[NULL, 'plain'], NULL);

CREATE TYPE public.point2d AS (x integer, y text);

CREATE TABLE public.t_composite (
    id integer PRIMARY KEY,
    v_point public.point2d
);
INSERT INTO public.t_composite VALUES
    (1, ROW(1, 'a,b"c')),
    (2, NULL);

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

CREATE TABLE public.t_base_type (
    id integer PRIMARY KEY,
    v_mybase public.mybase
);
INSERT INTO public.t_base_type VALUES
    (1, 'hello'),
    (2, NULL);

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
