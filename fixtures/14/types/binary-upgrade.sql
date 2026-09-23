--
-- PostgreSQL database dump
--

\restrict AJfg24YMV7CdwzPYFsBOv2PLWPXKcKg7ajUOKQelS9VgVHjhQztmtl8hjUIZh3m

-- Dumped from database version 14.24 (Debian 14.24-1.pgdg13+2)
-- Dumped by pg_dump version 14.24 (Debian 14.24-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
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

CREATE COLLATION public.nd_collation (provider = icu, deterministic = false, locale = 'und', version = '153.128');


ALTER COLLATION public.nd_collation OWNER TO postgres;

--
-- Name: intarr; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16620'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16619'::pg_catalog.oid);

CREATE DOMAIN public.intarr AS integer[];


ALTER DOMAIN public.intarr OWNER TO postgres;

--
-- Name: arr_holder; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16623'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16622'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16621'::pg_catalog.oid);

CREATE TYPE public.arr_holder AS (
	label text,
	arr public.intarr[]
);


ALTER TYPE public.arr_holder OWNER TO postgres;

--
-- Name: base_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16402'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16401'::pg_catalog.oid);

CREATE DOMAIN public.base_domain AS integer;


ALTER DOMAIN public.base_domain OWNER TO postgres;

--
-- Name: box_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16570'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16569'::pg_catalog.oid);

CREATE DOMAIN public.box_domain AS box;


ALTER DOMAIN public.box_domain OWNER TO postgres;

--
-- Name: point2d; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16536'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16535'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16534'::pg_catalog.oid);

CREATE TYPE public.point2d AS (
	x integer,
	y text
);


ALTER TYPE public.point2d OWNER TO postgres;

--
-- Name: boxed_point; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16628'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16627'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16626'::pg_catalog.oid);

CREATE TYPE public.boxed_point AS (
	label text,
	pt public.point2d
);


ALTER TYPE public.boxed_point OWNER TO postgres;

--
-- Name: collated_pair; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16482'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16481'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16480'::pg_catalog.oid);

CREATE TYPE public.collated_pair AS (
	plain text,
	c text COLLATE pg_catalog."C"
);


ALTER TYPE public.collated_pair OWNER TO postgres;

--
-- Name: derived_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16404'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16403'::pg_catalog.oid);

CREATE DOMAIN public.derived_domain AS public.base_domain NOT NULL;


ALTER DOMAIN public.derived_domain OWNER TO postgres;

--
-- Name: empty_comp; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16542'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16541'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16540'::pg_catalog.oid);

CREATE TYPE public.empty_comp AS (
);


ALTER TYPE public.empty_comp OWNER TO postgres;

--
-- Name: empty_enum; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16400'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16399'::pg_catalog.oid);

CREATE TYPE public.empty_enum AS ENUM (
);


ALTER TYPE public.empty_enum OWNER TO postgres;

--
-- Name: mood; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16386'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16385'::pg_catalog.oid);

CREATE TYPE public.mood AS ENUM (
);

-- For binary upgrade, must preserve pg_enum oids
SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16388'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'sad';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16390'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'ok';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16392'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'happy';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16394'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has space';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16396'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has,comma';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16398'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has''quote';



ALTER TYPE public.mood OWNER TO postgres;

--
-- Name: mybase; Type: SHELL TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16558'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16561'::pg_catalog.oid);

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


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16558'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16561'::pg_catalog.oid);

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


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16581'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16578'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16579'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16580'::pg_catalog.oid);

CREATE TYPE public.myrange AS RANGE (
    subtype = double precision,
    multirange_type_name = public.myrange_multi
);


ALTER TYPE public.myrange OWNER TO postgres;

--
-- Name: pointdom; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16625'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16624'::pg_catalog.oid);

CREATE DOMAIN public.pointdom AS public.point2d;


ALTER DOMAIN public.pointdom OWNER TO postgres;

--
-- Name: rangedom; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16630'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16629'::pg_catalog.oid);

CREATE DOMAIN public.rangedom AS public.myrange;


ALTER DOMAIN public.rangedom OWNER TO postgres;

--
-- Name: shellonly; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16557'::pg_catalog.oid);

CREATE TYPE public.shellonly;


ALTER TYPE public.shellonly OWNER TO postgres;

--
-- Name: tagged; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16539'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16538'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16537'::pg_catalog.oid);

CREATE TYPE public.tagged AS (
	label text,
	tags text[]
);


ALTER TYPE public.tagged OWNER TO postgres;

--
-- Name: text_c; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16479'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16478'::pg_catalog.oid);

CREATE DOMAIN public.text_c AS text COLLATE pg_catalog."C";


ALTER DOMAIN public.text_c OWNER TO postgres;

--
-- Name: textrange; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16598'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16595'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16596'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16597'::pg_catalog.oid);

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


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16515'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16514'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16513'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16516'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16517'::pg_catalog.oid);

CREATE TABLE public.t_array (
    id integer NOT NULL,
    v_empty integer[],
    v_with_null integer[],
    v_null_array integer[],
    v_text_special text[],
    v_enum_array public.mood[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '777', relminmxid = '1'
WHERE oid = 'public.t_array'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '777', relminmxid = '1'
WHERE oid = '16516';


ALTER TABLE public.t_array OWNER TO postgres;

--
-- Name: t_array_shape; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16522'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16521'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16520'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16523'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16524'::pg_catalog.oid);

CREATE TABLE public.t_array_shape (
    id integer NOT NULL,
    v_multidim integer[],
    v_mixed_dim integer[],
    v_lbound integer[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '779', relminmxid = '1'
WHERE oid = 'public.t_array_shape'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '779', relminmxid = '1'
WHERE oid = '16523';


ALTER TABLE public.t_array_shape OWNER TO postgres;

--
-- Name: t_array_spelling; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16529'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16528'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16527'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16530'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16531'::pg_catalog.oid);

CREATE TABLE public.t_array_spelling (
    id integer NOT NULL,
    v_bounded integer[],
    v_bounded_2d integer[],
    v_array_kw integer[],
    v_array_kw_n integer[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '781', relminmxid = '1'
WHERE oid = 'public.t_array_spelling'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '781', relminmxid = '1'
WHERE oid = '16530';


ALTER TABLE public.t_array_spelling OWNER TO postgres;

--
-- Name: t_base_type; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16564'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16563'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16562'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16565'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16566'::pg_catalog.oid);

CREATE TABLE public.t_base_type (
    id integer NOT NULL,
    v_mybase public.mybase,
    v_mybase_array public.mybase[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '795', relminmxid = '1'
WHERE oid = 'public.t_base_type'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '795', relminmxid = '1'
WHERE oid = '16565';


ALTER TABLE public.t_base_type OWNER TO postgres;

--
-- Name: t_bytea; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16461'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16460'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16459'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16462'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16463'::pg_catalog.oid);

CREATE TABLE public.t_bytea (
    id integer NOT NULL,
    v_bytea bytea
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '759', relminmxid = '1'
WHERE oid = 'public.t_bytea'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '759', relminmxid = '1'
WHERE oid = '16462';


ALTER TABLE public.t_bytea OWNER TO postgres;

--
-- Name: t_collate; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16487'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16486'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16485'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16490'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16491'::pg_catalog.oid);

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

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '769', relminmxid = '1'
WHERE oid = 'public.t_collate'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '769', relminmxid = '1'
WHERE oid = '16490';


ALTER TABLE public.t_collate OWNER TO postgres;

--
-- Name: t_composite; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16545'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16544'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16543'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16546'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16547'::pg_catalog.oid);

CREATE TABLE public.t_composite (
    id integer NOT NULL,
    v_point public.point2d,
    v_points public.point2d[],
    v_tagged public.tagged,
    v_empty_comp public.empty_comp
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '786', relminmxid = '1'
WHERE oid = 'public.t_composite'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '786', relminmxid = '1'
WHERE oid = '16546';


ALTER TABLE public.t_composite OWNER TO postgres;

--
-- Name: t_date; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16434'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16433'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16432'::pg_catalog.oid);

CREATE TABLE public.t_date (
    id integer NOT NULL,
    v_date date
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '749', relminmxid = '1'
WHERE oid = 'public.t_date'::pg_catalog.regclass;


ALTER TABLE public.t_date OWNER TO postgres;

--
-- Name: t_delimiter; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16573'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16572'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16571'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16574'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16575'::pg_catalog.oid);

CREATE TABLE public.t_delimiter (
    id integer NOT NULL,
    v_box_domain public.box_domain,
    v_box_domain_array public.box_domain[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '798', relminmxid = '1'
WHERE oid = 'public.t_delimiter'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '798', relminmxid = '1'
WHERE oid = '16574';


ALTER TABLE public.t_delimiter OWNER TO postgres;

--
-- Name: t_enum_domain; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16510'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16509'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16508'::pg_catalog.oid);

CREATE TABLE public.t_enum_domain (
    id integer NOT NULL,
    v_mood public.mood,
    v_domain public.derived_domain,
    v_empty_enum public.empty_enum
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '775', relminmxid = '1'
WHERE oid = 'public.t_enum_domain'::pg_catalog.regclass;


ALTER TABLE public.t_enum_domain OWNER TO postgres;

--
-- Name: t_float; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16429'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16428'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16427'::pg_catalog.oid);

CREATE TABLE public.t_float (
    id integer NOT NULL,
    v_real real,
    v_double double precision
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '747', relminmxid = '1'
WHERE oid = 'public.t_float'::pg_catalog.regclass;


ALTER TABLE public.t_float OWNER TO postgres;

--
-- Name: t_int; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16407'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16406'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16405'::pg_catalog.oid);

CREATE TABLE public.t_int (
    id integer NOT NULL,
    v_smallint smallint,
    v_integer integer,
    v_bigint bigint
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '739', relminmxid = '1'
WHERE oid = 'public.t_int'::pg_catalog.regclass;


ALTER TABLE public.t_int OWNER TO postgres;

--
-- Name: t_int2vector; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16417'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16416'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16415'::pg_catalog.oid);

CREATE TABLE public.t_int2vector (
    id integer NOT NULL,
    v_vec int2vector
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '743', relminmxid = '1'
WHERE oid = 'public.t_int2vector'::pg_catalog.regclass;


ALTER TABLE public.t_int2vector OWNER TO postgres;

--
-- Name: t_interval; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16449'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16448'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16447'::pg_catalog.oid);

CREATE TABLE public.t_interval (
    id integer NOT NULL,
    v_interval interval
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '755', relminmxid = '1'
WHERE oid = 'public.t_interval'::pg_catalog.regclass;


ALTER TABLE public.t_interval OWNER TO postgres;

--
-- Name: t_json; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16496'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16495'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16494'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16497'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16498'::pg_catalog.oid);

CREATE TABLE public.t_json (
    id integer NOT NULL,
    v_json json,
    v_jsonb jsonb
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '771', relminmxid = '1'
WHERE oid = 'public.t_json'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '771', relminmxid = '1'
WHERE oid = '16497';


ALTER TABLE public.t_json OWNER TO postgres;

--
-- Name: t_multirange; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16614'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16613'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16612'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16615'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16616'::pg_catalog.oid);

CREATE TABLE public.t_multirange (
    id integer NOT NULL,
    v_int4multirange int4multirange,
    v_myrange_multi public.myrange_multi
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '806', relminmxid = '1'
WHERE oid = 'public.t_multirange'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '806', relminmxid = '1'
WHERE oid = '16615';


ALTER TABLE public.t_multirange OWNER TO postgres;

--
-- Name: t_nested_array; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16633'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16632'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16631'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16634'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16635'::pg_catalog.oid);

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

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '813', relminmxid = '1'
WHERE oid = 'public.t_nested_array'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '813', relminmxid = '1'
WHERE oid = '16634';


ALTER TABLE public.t_nested_array OWNER TO postgres;

--
-- Name: t_net; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16503'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16502'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16501'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16504'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16505'::pg_catalog.oid);

CREATE TABLE public.t_net (
    id integer NOT NULL,
    v_inet inet,
    v_cidr cidr,
    v_macaddr macaddr,
    v_macaddr8 macaddr8
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '773', relminmxid = '1'
WHERE oid = 'public.t_net'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '773', relminmxid = '1'
WHERE oid = '16504';


ALTER TABLE public.t_net OWNER TO postgres;

--
-- Name: t_numeric; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16422'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16421'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16420'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16423'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16424'::pg_catalog.oid);

CREATE TABLE public.t_numeric (
    id integer NOT NULL,
    v_typed numeric(38,10),
    v_typed39 numeric(39,10),
    v_small numeric(10,2),
    v_untyped numeric
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '745', relminmxid = '1'
WHERE oid = 'public.t_numeric'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '745', relminmxid = '1'
WHERE oid = '16423';


ALTER TABLE public.t_numeric OWNER TO postgres;

--
-- Name: t_oid; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16412'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16411'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16410'::pg_catalog.oid);

CREATE TABLE public.t_oid (
    id integer NOT NULL,
    v_oid oid
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '741', relminmxid = '1'
WHERE oid = 'public.t_oid'::pg_catalog.regclass;


ALTER TABLE public.t_oid OWNER TO postgres;

--
-- Name: t_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16552'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16551'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16550'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16553'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16554'::pg_catalog.oid);

CREATE TABLE public.t_range (
    id integer NOT NULL,
    v_range int4range
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '788', relminmxid = '1'
WHERE oid = 'public.t_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '788', relminmxid = '1'
WHERE oid = '16553';


ALTER TABLE public.t_range OWNER TO postgres;

--
-- Name: t_text; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16473'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16472'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16471'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16474'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16475'::pg_catalog.oid);

CREATE TABLE public.t_text (
    id integer NOT NULL,
    v_text text,
    v_varchar character varying(10),
    v_char character(10)
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '763', relminmxid = '1'
WHERE oid = 'public.t_text'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '763', relminmxid = '1'
WHERE oid = '16474';


ALTER TABLE public.t_text OWNER TO postgres;

--
-- Name: t_text_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16607'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16606'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16605'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16608'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16609'::pg_catalog.oid);

CREATE TABLE public.t_text_range (
    id integer NOT NULL,
    v_textrange public.textrange
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '804', relminmxid = '1'
WHERE oid = 'public.t_text_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '804', relminmxid = '1'
WHERE oid = '16608';


ALTER TABLE public.t_text_range OWNER TO postgres;

--
-- Name: t_time; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16444'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16443'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16442'::pg_catalog.oid);

CREATE TABLE public.t_time (
    id integer NOT NULL,
    v_time time without time zone,
    v_timetz time with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '753', relminmxid = '1'
WHERE oid = 'public.t_time'::pg_catalog.regclass;


ALTER TABLE public.t_time OWNER TO postgres;

--
-- Name: t_timestamp; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16439'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16438'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16437'::pg_catalog.oid);

CREATE TABLE public.t_timestamp (
    id integer NOT NULL,
    v_ts timestamp without time zone,
    v_tstz timestamp with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '751', relminmxid = '1'
WHERE oid = 'public.t_timestamp'::pg_catalog.regclass;


ALTER TABLE public.t_timestamp OWNER TO postgres;

--
-- Name: t_type_spelling; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16454'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16453'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16452'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16455'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16456'::pg_catalog.oid);

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

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '757', relminmxid = '1'
WHERE oid = 'public.t_type_spelling'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '757', relminmxid = '1'
WHERE oid = '16455';


ALTER TABLE public.t_type_spelling OWNER TO postgres;

--
-- Name: t_user_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16590'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16589'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16588'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16591'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16592'::pg_catalog.oid);

CREATE TABLE public.t_user_range (
    id integer NOT NULL,
    v_myrange public.myrange
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '801', relminmxid = '1'
WHERE oid = 'public.t_user_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '801', relminmxid = '1'
WHERE oid = '16591';


ALTER TABLE public.t_user_range OWNER TO postgres;

--
-- Name: t_uuid; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16468'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16467'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16466'::pg_catalog.oid);

CREATE TABLE public.t_uuid (
    id integer NOT NULL,
    v_uuid uuid
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '761', relminmxid = '1'
WHERE oid = 'public.t_uuid'::pg_catalog.regclass;


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
5	0001-01-01 00:00:00	0001-01-01 00:00:00+00
6	0044-01-01 00:00:00 BC	0044-01-01 00:00:00+00 BC
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


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16518'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array
    ADD CONSTRAINT t_array_pkey PRIMARY KEY (id);


--
-- Name: t_array_shape t_array_shape_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16525'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array_shape
    ADD CONSTRAINT t_array_shape_pkey PRIMARY KEY (id);


--
-- Name: t_array_spelling t_array_spelling_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16532'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array_spelling
    ADD CONSTRAINT t_array_spelling_pkey PRIMARY KEY (id);


--
-- Name: t_base_type t_base_type_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16567'::pg_catalog.oid);

ALTER TABLE ONLY public.t_base_type
    ADD CONSTRAINT t_base_type_pkey PRIMARY KEY (id);


--
-- Name: t_bytea t_bytea_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16464'::pg_catalog.oid);

ALTER TABLE ONLY public.t_bytea
    ADD CONSTRAINT t_bytea_pkey PRIMARY KEY (id);


--
-- Name: t_collate t_collate_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16492'::pg_catalog.oid);

ALTER TABLE ONLY public.t_collate
    ADD CONSTRAINT t_collate_pkey PRIMARY KEY (id);


--
-- Name: t_composite t_composite_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16548'::pg_catalog.oid);

ALTER TABLE ONLY public.t_composite
    ADD CONSTRAINT t_composite_pkey PRIMARY KEY (id);


--
-- Name: t_date t_date_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16435'::pg_catalog.oid);

ALTER TABLE ONLY public.t_date
    ADD CONSTRAINT t_date_pkey PRIMARY KEY (id);


--
-- Name: t_delimiter t_delimiter_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16576'::pg_catalog.oid);

ALTER TABLE ONLY public.t_delimiter
    ADD CONSTRAINT t_delimiter_pkey PRIMARY KEY (id);


--
-- Name: t_enum_domain t_enum_domain_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16511'::pg_catalog.oid);

ALTER TABLE ONLY public.t_enum_domain
    ADD CONSTRAINT t_enum_domain_pkey PRIMARY KEY (id);


--
-- Name: t_float t_float_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16430'::pg_catalog.oid);

ALTER TABLE ONLY public.t_float
    ADD CONSTRAINT t_float_pkey PRIMARY KEY (id);


--
-- Name: t_int2vector t_int2vector_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16418'::pg_catalog.oid);

ALTER TABLE ONLY public.t_int2vector
    ADD CONSTRAINT t_int2vector_pkey PRIMARY KEY (id);


--
-- Name: t_int t_int_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16408'::pg_catalog.oid);

ALTER TABLE ONLY public.t_int
    ADD CONSTRAINT t_int_pkey PRIMARY KEY (id);


--
-- Name: t_interval t_interval_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16450'::pg_catalog.oid);

ALTER TABLE ONLY public.t_interval
    ADD CONSTRAINT t_interval_pkey PRIMARY KEY (id);


--
-- Name: t_json t_json_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16499'::pg_catalog.oid);

ALTER TABLE ONLY public.t_json
    ADD CONSTRAINT t_json_pkey PRIMARY KEY (id);


--
-- Name: t_multirange t_multirange_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16617'::pg_catalog.oid);

ALTER TABLE ONLY public.t_multirange
    ADD CONSTRAINT t_multirange_pkey PRIMARY KEY (id);


--
-- Name: t_nested_array t_nested_array_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16636'::pg_catalog.oid);

ALTER TABLE ONLY public.t_nested_array
    ADD CONSTRAINT t_nested_array_pkey PRIMARY KEY (id);


--
-- Name: t_net t_net_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16506'::pg_catalog.oid);

ALTER TABLE ONLY public.t_net
    ADD CONSTRAINT t_net_pkey PRIMARY KEY (id);


--
-- Name: t_numeric t_numeric_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16425'::pg_catalog.oid);

ALTER TABLE ONLY public.t_numeric
    ADD CONSTRAINT t_numeric_pkey PRIMARY KEY (id);


--
-- Name: t_oid t_oid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16413'::pg_catalog.oid);

ALTER TABLE ONLY public.t_oid
    ADD CONSTRAINT t_oid_pkey PRIMARY KEY (id);


--
-- Name: t_range t_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16555'::pg_catalog.oid);

ALTER TABLE ONLY public.t_range
    ADD CONSTRAINT t_range_pkey PRIMARY KEY (id);


--
-- Name: t_text t_text_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16476'::pg_catalog.oid);

ALTER TABLE ONLY public.t_text
    ADD CONSTRAINT t_text_pkey PRIMARY KEY (id);


--
-- Name: t_text_range t_text_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16610'::pg_catalog.oid);

ALTER TABLE ONLY public.t_text_range
    ADD CONSTRAINT t_text_range_pkey PRIMARY KEY (id);


--
-- Name: t_time t_time_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16445'::pg_catalog.oid);

ALTER TABLE ONLY public.t_time
    ADD CONSTRAINT t_time_pkey PRIMARY KEY (id);


--
-- Name: t_timestamp t_timestamp_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16440'::pg_catalog.oid);

ALTER TABLE ONLY public.t_timestamp
    ADD CONSTRAINT t_timestamp_pkey PRIMARY KEY (id);


--
-- Name: t_type_spelling t_type_spelling_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16457'::pg_catalog.oid);

ALTER TABLE ONLY public.t_type_spelling
    ADD CONSTRAINT t_type_spelling_pkey PRIMARY KEY (id);


--
-- Name: t_user_range t_user_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16593'::pg_catalog.oid);

ALTER TABLE ONLY public.t_user_range
    ADD CONSTRAINT t_user_range_pkey PRIMARY KEY (id);


--
-- Name: t_uuid t_uuid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16469'::pg_catalog.oid);

ALTER TABLE ONLY public.t_uuid
    ADD CONSTRAINT t_uuid_pkey PRIMARY KEY (id);


--
-- PostgreSQL database dump complete
--

\unrestrict AJfg24YMV7CdwzPYFsBOv2PLWPXKcKg7ajUOKQelS9VgVHjhQztmtl8hjUIZh3m

