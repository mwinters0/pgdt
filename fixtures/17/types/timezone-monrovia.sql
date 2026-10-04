--
-- PostgreSQL database dump
--

\restrict 2OuM8WrK8ReXzDUTfTh1CHk6Cemzsz8SnZetjKlWbT7oAnWFWhzt5U2VMXfas5y

-- Dumped from database version 17.11 (Debian 17.11-1.pgdg13+2)
-- Dumped by pg_dump version 17.11 (Debian 17.11-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: c_collation; Type: COLLATION; Schema: public; Owner: postgres
--

CREATE COLLATION public.c_collation (provider = libc, locale = 'C');


ALTER COLLATION public.c_collation OWNER TO postgres;

--
-- Name: nd_collation; Type: COLLATION; Schema: public; Owner: postgres
--

CREATE COLLATION public.nd_collation (provider = icu, deterministic = false, locale = 'und');


ALTER COLLATION public.nd_collation OWNER TO postgres;

--
-- Name: intarr; Type: DOMAIN; Schema: public; Owner: postgres
--

CREATE DOMAIN public.intarr AS integer[];


ALTER DOMAIN public.intarr OWNER TO postgres;

--
-- Name: arr_holder; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.arr_holder AS (
	label text,
	arr public.intarr[]
);


ALTER TYPE public.arr_holder OWNER TO postgres;

--
-- Name: base_domain; Type: DOMAIN; Schema: public; Owner: postgres
--

CREATE DOMAIN public.base_domain AS integer;


ALTER DOMAIN public.base_domain OWNER TO postgres;

--
-- Name: box_domain; Type: DOMAIN; Schema: public; Owner: postgres
--

CREATE DOMAIN public.box_domain AS box;


ALTER DOMAIN public.box_domain OWNER TO postgres;

--
-- Name: point2d; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.point2d AS (
	x integer,
	y text
);


ALTER TYPE public.point2d OWNER TO postgres;

--
-- Name: boxed_point; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.boxed_point AS (
	label text,
	pt public.point2d
);


ALTER TYPE public.boxed_point OWNER TO postgres;

--
-- Name: collated_pair; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.collated_pair AS (
	plain text,
	c text COLLATE pg_catalog."C"
);


ALTER TYPE public.collated_pair OWNER TO postgres;

--
-- Name: dated; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.dated AS (
	label text,
	d date
);


ALTER TYPE public.dated OWNER TO postgres;

--
-- Name: derived_domain; Type: DOMAIN; Schema: public; Owner: postgres
--

CREATE DOMAIN public.derived_domain AS public.base_domain NOT NULL;


ALTER DOMAIN public.derived_domain OWNER TO postgres;

--
-- Name: empty_comp; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.empty_comp AS (
);


ALTER TYPE public.empty_comp OWNER TO postgres;

--
-- Name: empty_enum; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.empty_enum AS ENUM (
);


ALTER TYPE public.empty_enum OWNER TO postgres;

--
-- Name: mood; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.mood AS ENUM (
    'sad',
    'ok',
    'happy',
    'has space',
    'has,comma',
    'has''quote'
);


ALTER TYPE public.mood OWNER TO postgres;

--
-- Name: mybase; Type: SHELL TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.mybase;


--
-- Name: mybase_in(cstring); Type: FUNCTION; Schema: public; Owner: postgres
--

CREATE FUNCTION public.mybase_in(cstring) RETURNS public.mybase
    LANGUAGE internal IMMUTABLE STRICT
    AS $$textin$$;


ALTER FUNCTION public.mybase_in(cstring) OWNER TO postgres;

--
-- Name: mybase_out(public.mybase); Type: FUNCTION; Schema: public; Owner: postgres
--

CREATE FUNCTION public.mybase_out(public.mybase) RETURNS cstring
    LANGUAGE internal IMMUTABLE STRICT
    AS $$textout$$;


ALTER FUNCTION public.mybase_out(public.mybase) OWNER TO postgres;

--
-- Name: mybase; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.mybase (
    INTERNALLENGTH = variable,
    INPUT = public.mybase_in,
    OUTPUT = public.mybase_out,
    ALIGNMENT = int4,
    STORAGE = extended
);


ALTER TYPE public.mybase OWNER TO postgres;

--
-- Name: myrange; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.myrange AS RANGE (
    subtype = double precision,
    multirange_type_name = public.myrange_multi
);


ALTER TYPE public.myrange OWNER TO postgres;

--
-- Name: pointdom; Type: DOMAIN; Schema: public; Owner: postgres
--

CREATE DOMAIN public.pointdom AS public.point2d;


ALTER DOMAIN public.pointdom OWNER TO postgres;

--
-- Name: rangedom; Type: DOMAIN; Schema: public; Owner: postgres
--

CREATE DOMAIN public.rangedom AS public.myrange;


ALTER DOMAIN public.rangedom OWNER TO postgres;

--
-- Name: shellonly; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.shellonly;


ALTER TYPE public.shellonly OWNER TO postgres;

--
-- Name: tagged; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.tagged AS (
	label text,
	tags text[]
);


ALTER TYPE public.tagged OWNER TO postgres;

--
-- Name: text_c; Type: DOMAIN; Schema: public; Owner: postgres
--

CREATE DOMAIN public.text_c AS text COLLATE pg_catalog."C";


ALTER DOMAIN public.text_c OWNER TO postgres;

--
-- Name: textrange; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.textrange AS RANGE (
    subtype = text,
    multirange_type_name = public.textmultirange,
    collation = pg_catalog."C"
);


ALTER TYPE public.textrange OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: t_array; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_array (
    id integer NOT NULL,
    v_empty integer[],
    v_with_null integer[],
    v_null_array integer[],
    v_text_special text[],
    v_enum_array public.mood[]
);


ALTER TABLE public.t_array OWNER TO postgres;

--
-- Name: t_array_shape; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_array_shape (
    id integer NOT NULL,
    v_multidim integer[],
    v_mixed_dim integer[],
    v_lbound integer[]
);


ALTER TABLE public.t_array_shape OWNER TO postgres;

--
-- Name: t_array_spelling; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_array_spelling (
    id integer NOT NULL,
    v_bounded integer[],
    v_bounded_2d integer[],
    v_array_kw integer[],
    v_array_kw_n integer[]
);


ALTER TABLE public.t_array_spelling OWNER TO postgres;

--
-- Name: t_base_type; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_base_type (
    id integer NOT NULL,
    v_mybase public.mybase,
    v_mybase_array public.mybase[]
);


ALTER TABLE public.t_base_type OWNER TO postgres;

--
-- Name: t_bit; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_bit (
    id integer NOT NULL,
    v_bit bit(1),
    v_bit3 bit(3),
    v_varbit bit varying,
    v_varbit5 bit varying(5),
    v_bit_any "bit"
);


ALTER TABLE public.t_bit OWNER TO postgres;

--
-- Name: t_bytea; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_bytea (
    id integer NOT NULL,
    v_bytea bytea
);


ALTER TABLE public.t_bytea OWNER TO postgres;

--
-- Name: t_collate; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_collate (
    id integer NOT NULL,
    v_text_c text COLLATE pg_catalog."C",
    v_text_locale text COLLATE pg_catalog."en_US.utf8",
    v_text_ucs text COLLATE pg_catalog.ucs_basic,
    v_name name,
    v_domain_c public.text_c,
    v_pair public.collated_pair,
    v_text_def text DEFAULT 'x'::text COLLATE pg_catalog."C",
    v_user text COLLATE public.c_collation,
    v_nd text COLLATE public.nd_collation,
    v_src text,
    v_gen_nn text GENERATED ALWAYS AS (upper(COALESCE(v_src, ''::text))) STORED NOT NULL COLLATE pg_catalog."C"
);


ALTER TABLE public.t_collate OWNER TO postgres;

--
-- Name: t_composite; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_composite (
    id integer NOT NULL,
    v_point public.point2d,
    v_points public.point2d[],
    v_tagged public.tagged,
    v_empty_comp public.empty_comp
);


ALTER TABLE public.t_composite OWNER TO postgres;

--
-- Name: t_composite_matrix; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_composite_matrix (
    id integer NOT NULL,
    v_tagged public.tagged
);


ALTER TABLE public.t_composite_matrix OWNER TO postgres;

--
-- Name: t_date; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_date (
    id integer NOT NULL,
    v_date date
);


ALTER TABLE public.t_date OWNER TO postgres;

--
-- Name: t_delimiter; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_delimiter (
    id integer NOT NULL,
    v_box_domain public.box_domain,
    v_box_domain_array public.box_domain[]
);


ALTER TABLE public.t_delimiter OWNER TO postgres;

--
-- Name: t_enum_domain; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_enum_domain (
    id integer NOT NULL,
    v_mood public.mood,
    v_domain public.derived_domain,
    v_empty_enum public.empty_enum
);


ALTER TABLE public.t_enum_domain OWNER TO postgres;

--
-- Name: t_extremes; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_extremes (
    id integer NOT NULL,
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


ALTER TABLE public.t_extremes OWNER TO postgres;

--
-- Name: t_extremes_nested; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_extremes_nested (
    id integer NOT NULL,
    v_date_array date[],
    v_daterange daterange,
    v_dated public.dated,
    v_interval_array interval[]
);


ALTER TABLE public.t_extremes_nested OWNER TO postgres;

--
-- Name: t_float; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_float (
    id integer NOT NULL,
    v_real real,
    v_double double precision
);


ALTER TABLE public.t_float OWNER TO postgres;

--
-- Name: t_int; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_int (
    id integer NOT NULL,
    v_smallint smallint,
    v_integer integer,
    v_bigint bigint
);


ALTER TABLE public.t_int OWNER TO postgres;

--
-- Name: t_int2vector; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_int2vector (
    id integer NOT NULL,
    v_vec int2vector
);


ALTER TABLE public.t_int2vector OWNER TO postgres;

--
-- Name: t_interval; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_interval (
    id integer NOT NULL,
    v_interval interval
);


ALTER TABLE public.t_interval OWNER TO postgres;

--
-- Name: t_json; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_json (
    id integer NOT NULL,
    v_json json,
    v_jsonb jsonb
);


ALTER TABLE public.t_json OWNER TO postgres;

--
-- Name: t_multirange; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_multirange (
    id integer NOT NULL,
    v_int4multirange int4multirange,
    v_myrange_multi public.myrange_multi
);


ALTER TABLE public.t_multirange OWNER TO postgres;

--
-- Name: t_nested_array; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_nested_array (
    id integer NOT NULL,
    v_nested_array public.intarr[],
    v_arr_holder public.arr_holder,
    v_pointdom public.pointdom,
    v_pointdom_array public.pointdom[],
    v_boxed_point public.boxed_point,
    v_myrange_array public.myrange[],
    v_rangedom public.rangedom
);


ALTER TABLE public.t_nested_array OWNER TO postgres;

--
-- Name: t_net; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_net (
    id integer NOT NULL,
    v_inet inet,
    v_cidr cidr,
    v_macaddr macaddr,
    v_macaddr8 macaddr8
);


ALTER TABLE public.t_net OWNER TO postgres;

--
-- Name: t_numeric; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_numeric (
    id integer NOT NULL,
    v_typed numeric(38,10),
    v_typed39 numeric(39,10),
    v_small numeric(10,2),
    v_untyped numeric
);


ALTER TABLE public.t_numeric OWNER TO postgres;

--
-- Name: t_oid; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_oid (
    id integer NOT NULL,
    v_oid oid
);


ALTER TABLE public.t_oid OWNER TO postgres;

--
-- Name: t_range; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_range (
    id integer NOT NULL,
    v_range int4range
);


ALTER TABLE public.t_range OWNER TO postgres;

--
-- Name: t_text; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_text (
    id integer NOT NULL,
    v_text text,
    v_varchar character varying(10),
    v_char character(10)
);


ALTER TABLE public.t_text OWNER TO postgres;

--
-- Name: t_text_range; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_text_range (
    id integer NOT NULL,
    v_textrange public.textrange
);


ALTER TABLE public.t_text_range OWNER TO postgres;

--
-- Name: t_time; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_time (
    id integer NOT NULL,
    v_time time without time zone,
    v_timetz time with time zone
);


ALTER TABLE public.t_time OWNER TO postgres;

--
-- Name: t_timestamp; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_timestamp (
    id integer NOT NULL,
    v_ts timestamp without time zone,
    v_tstz timestamp with time zone
);


ALTER TABLE public.t_timestamp OWNER TO postgres;

--
-- Name: t_type_spelling; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_type_spelling (
    id integer NOT NULL,
    v_ts3 timestamp(3) without time zone,
    v_tstz0 timestamp(0) with time zone,
    v_time3 time(3) without time zone,
    v_timetz2 time(2) with time zone,
    v_ym interval year to month,
    v_ds2 interval day to second(2),
    v_bpchar bpchar
);


ALTER TABLE public.t_type_spelling OWNER TO postgres;

--
-- Name: t_user_range; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_user_range (
    id integer NOT NULL,
    v_myrange public.myrange
);


ALTER TABLE public.t_user_range OWNER TO postgres;

--
-- Name: t_uuid; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.t_uuid (
    id integer NOT NULL,
    v_uuid uuid
);


ALTER TABLE public.t_uuid OWNER TO postgres;

--
-- Data for Name: t_array; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_array (id, v_empty, v_with_null, v_null_array, v_text_special, v_enum_array) FROM stdin;
1	{}	{NULL}	\N	{"a,b","c{d}","e\\"f","g\\\\h"}	{sad,"has space","has,comma",has'quote}
2	{1,2,3}	{1,NULL,3}	{1,2}	{NULL,plain}	{NULL,ok}
\.


--
-- Data for Name: t_array_shape; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_array_shape (id, v_multidim, v_mixed_dim, v_lbound) FROM stdin;
1	{{1,2},{3,4}}	{1,2}	[0:2]={7,8,9}
2	\N	{{1,2},{3,4}}	[-1:0]={10,11}
3	{{5,6},{7,8}}	\N	\N
\.


--
-- Data for Name: t_array_spelling; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_array_spelling (id, v_bounded, v_bounded_2d, v_array_kw, v_array_kw_n) FROM stdin;
1	{1,2,3,4}	{5,6}	{7,8}	{9}
2	{}	\N	{NULL,10}	{}
3	\N	\N	\N	\N
\.


--
-- Data for Name: t_base_type; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_base_type (id, v_mybase, v_mybase_array) FROM stdin;
1	hello	{hello,"a,b"}
2	\N	\N
\.


--
-- Data for Name: t_bit; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_bit (id, v_bit, v_bit3, v_varbit, v_varbit5, v_bit_any) FROM stdin;
1	1	101		10101	1100110011
2	0	000	1010101010101010101010101010101010101010	1	1
3	\N	\N	\N	\N	\N
\.


--
-- Data for Name: t_bytea; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_bytea (id, v_bytea) FROM stdin;
1	\\x
2	\N
3	\\xdeadbeef00ff
4	\\x5c6261636b736c617368
\.


--
-- Data for Name: t_collate; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_collate (id, v_text_c, v_text_locale, v_text_ucs, v_name, v_domain_c, v_pair, v_text_def, v_user, v_nd, v_src) FROM stdin;
1	A	A	A	A	A	(A,A)	A	A	A	A
2	a	a	a	a	a	(a,a)	a	a	a	a
3	B	B	B	B	B	(B,B)	B	B	B	B
4	é	é	é	é	é	(é,é)	é	é	é	é
5	f	f	f	f	f	(f,f)	f	f	f	f
6	_x	_x	_x	_x	_x	(_x,_x)	_x	_x	_x	_x
7	ax	ax	ax	ax	ax	(ax,ax)	ax	ax	ax	ax
8						("","")				
9	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N
\.


--
-- Data for Name: t_composite; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_composite (id, v_point, v_points, v_tagged, v_empty_comp) FROM stdin;
1	(1,"a,b""c")	{"(1,\\"a,b\\"\\"c\\")","(2,plain)"}	("a,b","{""x\\\\""y"",""p q"",NULL}")	()
2	\N	\N	\N	\N
3	(,"")	{NULL,"(3,)"}	("",{})	()
\.


--
-- Data for Name: t_composite_matrix; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_composite_matrix (id, v_tagged) FROM stdin;
1	(a,"{x,y}")
2	(b,{p})
3	\N
4	(c,"{q,NULL}")
5	(d,{})
6	(m,"{{a,b},{c,d}}")
7	(e,{r})
8	\N
9	(f,"{s,t}")
10	(g,{u})
11	(h,)
\.


--
-- Data for Name: t_date; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_date (id, v_date) FROM stdin;
1	infinity
2	-infinity
3	0001-01-01
4	9999-12-31
5	0044-01-01 BC
6	10000-01-01
7	\N
\.


--
-- Data for Name: t_delimiter; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_delimiter (id, v_box_domain, v_box_domain_array) FROM stdin;
1	(1,1),(0,0)	{(1,1),(0,0);(3,3),(2,2)}
2	\N	\N
\.


--
-- Data for Name: t_enum_domain; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_enum_domain (id, v_mood, v_domain, v_empty_enum) FROM stdin;
1	sad	5	\N
2	has space	0	\N
3	has,comma	-5	\N
4	has'quote	100	\N
\.


--
-- Data for Name: t_extremes; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_extremes (id, v_smallint, v_integer, v_bigint, v_oid, v_boolean, v_real, v_double, v_numeric38, v_numeric76, v_date, v_ts, v_tstz, v_time, v_interval, v_uuid, v_bytea, v_int2vector) FROM stdin;
1	-32768	-2147483648	-9223372036854775808	0	f	-3.4028235e+38	-1.7976931348623157e+308	-99999999999999999999999999999999999999	-9999999999999999999999999999999999999999999999999999999999999999999999999999	4714-11-24 BC	4714-11-24 00:00:00 BC	4714-11-23 23:16:52-00:43:08 BC	00:00:00	-178956970 years -8 mons	00000000-0000-0000-0000-000000000000	\\x	-32768
2	32767	2147483647	9223372036854775807	4294967295	t	3.4028235e+38	1.7976931348623157e+308	99999999999999999999999999999999999999	9999999999999999999999999999999999999999999999999999999999999999999999999999	5874897-12-31	294276-12-31 23:59:59.999999	294276-12-31 23:59:59.999999+00	24:00:00	178956970 years 7 mons	ffffffff-ffff-ffff-ffff-ffffffffffff	\\xff	32767
3	\N	\N	\N	\N	\N	-Infinity	-Infinity	\N	\N	-infinity	-infinity	-infinity	\N	-2147483648 days	\N	\N	\N
4	\N	\N	\N	\N	\N	Infinity	Infinity	\N	\N	infinity	infinity	infinity	\N	2147483647 days	\N	\N	\N
5	\N	\N	\N	\N	\N	NaN	NaN	NaN	NaN	\N	\N	\N	\N	\N	\N	\N	\N
6	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	-2562047788:00:54.775807	\N	\N	\N
7	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	2562047788:00:54.775807	\N	\N	\N
8	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	2562047:47:16.854776	\N	\N	\N
9	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	2562047:47:16.854775	\N	\N	\N
10	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	-2562047:47:16.854776	\N	\N	\N
11	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	-2562047:47:16.854775	\N	\N	\N
12	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	294247-01-10 04:00:54.775807	294247-01-10 04:00:54.775807+00	\N	\N	\N	\N	\N
13	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	294247-01-10 04:00:54.775808	294247-01-10 04:00:54.775808+00	\N	\N	\N	\N	\N
14	\N	\N	\N	\N	\N	\N	\N	\N	\N	262142-12-31	262142-12-31 23:59:59.999999	262142-12-31 23:59:59.999999+00	\N	\N	\N	\N	\N
15	\N	\N	\N	\N	\N	\N	\N	\N	\N	262143-01-01	262143-01-01 00:00:00	262143-01-01 00:00:00+00	\N	\N	\N	\N	\N
16	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N
17	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	-infinity	\N	\N	\N
18	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	\N	infinity	\N	\N	\N
\.


--
-- Data for Name: t_extremes_nested; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_extremes_nested (id, v_date_array, v_daterange, v_dated, v_interval_array) FROM stdin;
1	{2024-01-01,infinity}	[2024-01-01,infinity)	(x,infinity)	{"1 day",2562047:47:16.854776}
2	{2024-01-01}	[2024-01-01,2024-02-01)	(infinity,2024-01-01)	{2562047:47:16.854775}
3	\N	\N	\N	\N
\.


--
-- Data for Name: t_float; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_float (id, v_real, v_double) FROM stdin;
1	NaN	NaN
2	Infinity	Infinity
3	-Infinity	-Infinity
4	-0	-0
5	1.1754944e-38	2.2250738585072014e-308
6	3.1415927	3.14159265358979
7	\N	\N
\.


--
-- Data for Name: t_int; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_int (id, v_smallint, v_integer, v_bigint) FROM stdin;
1	-32768	-2147483648	-9223372036854775808
2	32767	2147483647	9223372036854775807
3	0	0	0
4	\N	\N	\N
\.


--
-- Data for Name: t_int2vector; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_int2vector (id, v_vec) FROM stdin;
1	1 2 3
2	
3	-32768 32767
4	0
5	\N
\.


--
-- Data for Name: t_interval; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_interval (id, v_interval) FROM stdin;
1	1 year 2 mons 3 days 04:05:06
2	-1 days
3	00:00:00
4	01:30:00
5	\N
\.


--
-- Data for Name: t_json; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_json (id, v_json, v_jsonb) FROM stdin;
1	{"a":1,"b":[1,2,3]}	{"a": 1, "b": [1, 2, 3]}
2	null	null
3	\N	\N
\.


--
-- Data for Name: t_multirange; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_multirange (id, v_int4multirange, v_myrange_multi) FROM stdin;
1	{[1,10)}	{[1.5,10.5)}
2	{}	{}
3	\N	\N
\.


--
-- Data for Name: t_nested_array; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_nested_array (id, v_nested_array, v_arr_holder, v_pointdom, v_pointdom_array, v_boxed_point, v_myrange_array, v_rangedom) FROM stdin;
1	{"{1,2}","{3}"}	(L,"{""{1,2}""}")	(1,"a,b""c")	{"(1,\\"a,b\\"\\"c\\")","(2,plain)"}	(outer,"(3,""x y"")")	{"[1.5,10.5)",empty}	[2.5,3.5)
2	{"{}","{5,NULL}"}	("",)	(,"")	{NULL}	(,)	{NULL}	\N
3	\N	\N	\N	\N	\N	\N	\N
\.


--
-- Data for Name: t_net; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_net (id, v_inet, v_cidr, v_macaddr, v_macaddr8) FROM stdin;
1	192.168.1.1	192.168.1.0/24	08:00:2b:01:02:03	08:00:2b:01:02:03:04:05
2	::1	::/0	\N	\N
3	\N	\N	\N	\N
\.


--
-- Data for Name: t_numeric; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_numeric (id, v_typed, v_typed39, v_small, v_untyped) FROM stdin;
1	1234567890123456789012345678.1234567890	\N	\N	\N
2	\N	123456789.1234567890	\N	\N
3	\N	\N	NaN	NaN
4	0.0000000000	0.0000000000	0.00	0
5	-1.5000000000	-1.5000000000	-1.50	100.00
6	\N	\N	\N	12345.6789
7	\N	\N	\N	\N
\.


--
-- Data for Name: t_oid; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_oid (id, v_oid) FROM stdin;
1	0
2	2147483647
3	2147483648
4	4294967295
5	\N
\.


--
-- Data for Name: t_range; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_range (id, v_range) FROM stdin;
1	[1,10)
2	empty
3	(,5)
4	\N
\.


--
-- Data for Name: t_text; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_text (id, v_text, v_varchar, v_char) FROM stdin;
1			          
2	\N	\N	\N
3	hello	hello	hi        
\.


--
-- Data for Name: t_text_range; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_text_range (id, v_textrange) FROM stdin;
1	["a,b","c""d")
2	[" lead","trail ")
3	["",a)
4	(,z)
5	empty
6	\N
\.


--
-- Data for Name: t_time; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_time (id, v_time, v_timetz) FROM stdin;
1	24:00:00	24:00:00+00
2	00:00:00.000001	00:00:00.000001-05
3	\N	\N
\.


--
-- Data for Name: t_timestamp; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_timestamp (id, v_ts, v_tstz) FROM stdin;
1	infinity	infinity
2	-infinity	-infinity
3	2024-01-01 00:00:00	2023-12-31 18:30:00+00
4	2024-01-01 00:00:00.123456	2024-01-01 00:00:00.123456+00
5	0001-01-01 00:00:00	0001-12-31 23:16:52-00:43:08 BC
6	0044-01-01 00:00:00 BC	0045-12-31 23:16:52-00:43:08 BC
7	294276-12-31 23:59:59.999999	294276-12-31 23:59:59.999999+00
8	\N	\N
\.


--
-- Data for Name: t_type_spelling; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_type_spelling (id, v_ts3, v_tstz0, v_time3, v_timetz2, v_ym, v_ds2, v_bpchar) FROM stdin;
1	2024-01-01 00:00:00.123	2024-01-01 00:00:01+00	12:34:56.789	12:34:56.79+02	1 year 2 mons	3 days 04:05:06.79	ab  
2	\N	\N	\N	\N	\N	\N	\N
\.


--
-- Data for Name: t_user_range; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_user_range (id, v_myrange) FROM stdin;
1	[1.5,10.5)
2	empty
3	\N
\.


--
-- Data for Name: t_uuid; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_uuid (id, v_uuid) FROM stdin;
1	00000000-0000-0000-0000-000000000000
2	a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11
3	\N
\.


--
-- Name: t_array t_array_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_array
    ADD CONSTRAINT t_array_pkey PRIMARY KEY (id);


--
-- Name: t_array_shape t_array_shape_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_array_shape
    ADD CONSTRAINT t_array_shape_pkey PRIMARY KEY (id);


--
-- Name: t_array_spelling t_array_spelling_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_array_spelling
    ADD CONSTRAINT t_array_spelling_pkey PRIMARY KEY (id);


--
-- Name: t_base_type t_base_type_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_base_type
    ADD CONSTRAINT t_base_type_pkey PRIMARY KEY (id);


--
-- Name: t_bit t_bit_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_bit
    ADD CONSTRAINT t_bit_pkey PRIMARY KEY (id);


--
-- Name: t_bytea t_bytea_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_bytea
    ADD CONSTRAINT t_bytea_pkey PRIMARY KEY (id);


--
-- Name: t_collate t_collate_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_collate
    ADD CONSTRAINT t_collate_pkey PRIMARY KEY (id);


--
-- Name: t_composite_matrix t_composite_matrix_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_composite_matrix
    ADD CONSTRAINT t_composite_matrix_pkey PRIMARY KEY (id);


--
-- Name: t_composite t_composite_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_composite
    ADD CONSTRAINT t_composite_pkey PRIMARY KEY (id);


--
-- Name: t_date t_date_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_date
    ADD CONSTRAINT t_date_pkey PRIMARY KEY (id);


--
-- Name: t_delimiter t_delimiter_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_delimiter
    ADD CONSTRAINT t_delimiter_pkey PRIMARY KEY (id);


--
-- Name: t_enum_domain t_enum_domain_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_enum_domain
    ADD CONSTRAINT t_enum_domain_pkey PRIMARY KEY (id);


--
-- Name: t_extremes_nested t_extremes_nested_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_extremes_nested
    ADD CONSTRAINT t_extremes_nested_pkey PRIMARY KEY (id);


--
-- Name: t_extremes t_extremes_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_extremes
    ADD CONSTRAINT t_extremes_pkey PRIMARY KEY (id);


--
-- Name: t_float t_float_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_float
    ADD CONSTRAINT t_float_pkey PRIMARY KEY (id);


--
-- Name: t_int2vector t_int2vector_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_int2vector
    ADD CONSTRAINT t_int2vector_pkey PRIMARY KEY (id);


--
-- Name: t_int t_int_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_int
    ADD CONSTRAINT t_int_pkey PRIMARY KEY (id);


--
-- Name: t_interval t_interval_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_interval
    ADD CONSTRAINT t_interval_pkey PRIMARY KEY (id);


--
-- Name: t_json t_json_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_json
    ADD CONSTRAINT t_json_pkey PRIMARY KEY (id);


--
-- Name: t_multirange t_multirange_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_multirange
    ADD CONSTRAINT t_multirange_pkey PRIMARY KEY (id);


--
-- Name: t_nested_array t_nested_array_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_nested_array
    ADD CONSTRAINT t_nested_array_pkey PRIMARY KEY (id);


--
-- Name: t_net t_net_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_net
    ADD CONSTRAINT t_net_pkey PRIMARY KEY (id);


--
-- Name: t_numeric t_numeric_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_numeric
    ADD CONSTRAINT t_numeric_pkey PRIMARY KEY (id);


--
-- Name: t_oid t_oid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_oid
    ADD CONSTRAINT t_oid_pkey PRIMARY KEY (id);


--
-- Name: t_range t_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_range
    ADD CONSTRAINT t_range_pkey PRIMARY KEY (id);


--
-- Name: t_text t_text_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_text
    ADD CONSTRAINT t_text_pkey PRIMARY KEY (id);


--
-- Name: t_text_range t_text_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_text_range
    ADD CONSTRAINT t_text_range_pkey PRIMARY KEY (id);


--
-- Name: t_time t_time_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_time
    ADD CONSTRAINT t_time_pkey PRIMARY KEY (id);


--
-- Name: t_timestamp t_timestamp_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_timestamp
    ADD CONSTRAINT t_timestamp_pkey PRIMARY KEY (id);


--
-- Name: t_type_spelling t_type_spelling_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_type_spelling
    ADD CONSTRAINT t_type_spelling_pkey PRIMARY KEY (id);


--
-- Name: t_user_range t_user_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_user_range
    ADD CONSTRAINT t_user_range_pkey PRIMARY KEY (id);


--
-- Name: t_uuid t_uuid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.t_uuid
    ADD CONSTRAINT t_uuid_pkey PRIMARY KEY (id);


--
-- PostgreSQL database dump complete
--

\unrestrict 2OuM8WrK8ReXzDUTfTh1CHk6Cemzsz8SnZetjKlWbT7oAnWFWhzt5U2VMXfas5y

