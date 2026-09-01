--
-- PostgreSQL database dump
--

\restrict xt6kZhKX8fk38sVpKJjkwPwvtaAIib4uQCOcvvlHdBcMbTHi1IteeaaoizAcIjG

-- Dumped from database version 15.19 (Debian 15.19-1.pgdg13+2)
-- Dumped by pg_dump version 15.19 (Debian 15.19-1.pgdg13+2)

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
-- Name: intarr; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16855'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16854'::pg_catalog.oid);

CREATE DOMAIN public.intarr AS integer[];


ALTER DOMAIN public.intarr OWNER TO postgres;

--
-- Name: arr_holder; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16858'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16857'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16856'::pg_catalog.oid);

CREATE TYPE public.arr_holder AS (
	label text,
	arr public.intarr[]
);


ALTER TYPE public.arr_holder OWNER TO postgres;

--
-- Name: base_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16650'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16649'::pg_catalog.oid);

CREATE DOMAIN public.base_domain AS integer;


ALTER DOMAIN public.base_domain OWNER TO postgres;

--
-- Name: box_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16805'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16804'::pg_catalog.oid);

CREATE DOMAIN public.box_domain AS box;


ALTER DOMAIN public.box_domain OWNER TO postgres;

--
-- Name: point2d; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16771'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16770'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16769'::pg_catalog.oid);

CREATE TYPE public.point2d AS (
	x integer,
	y text
);


ALTER TYPE public.point2d OWNER TO postgres;

--
-- Name: boxed_point; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16863'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16862'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16861'::pg_catalog.oid);

CREATE TYPE public.boxed_point AS (
	label text,
	pt public.point2d
);


ALTER TYPE public.boxed_point OWNER TO postgres;

--
-- Name: collated_pair; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16718'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16717'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16716'::pg_catalog.oid);

CREATE TYPE public.collated_pair AS (
	plain text,
	c text COLLATE pg_catalog."C"
);


ALTER TYPE public.collated_pair OWNER TO postgres;

--
-- Name: derived_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16652'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16651'::pg_catalog.oid);

CREATE DOMAIN public.derived_domain AS public.base_domain NOT NULL;


ALTER DOMAIN public.derived_domain OWNER TO postgres;

--
-- Name: empty_comp; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16777'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16776'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16775'::pg_catalog.oid);

CREATE TYPE public.empty_comp AS (
);


ALTER TYPE public.empty_comp OWNER TO postgres;

--
-- Name: empty_enum; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16648'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16647'::pg_catalog.oid);

CREATE TYPE public.empty_enum AS ENUM (
);


ALTER TYPE public.empty_enum OWNER TO postgres;

--
-- Name: mood; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16634'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16633'::pg_catalog.oid);

CREATE TYPE public.mood AS ENUM (
);

-- For binary upgrade, must preserve pg_enum oids
SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16636'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'sad';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16638'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'ok';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16640'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'happy';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16642'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has space';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16644'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has,comma';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16646'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has''quote';



ALTER TYPE public.mood OWNER TO postgres;

--
-- Name: mybase; Type: SHELL TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16793'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16796'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16793'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16796'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16816'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16813'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16814'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16815'::pg_catalog.oid);

CREATE TYPE public.myrange AS RANGE (
    subtype = double precision,
    multirange_type_name = public.myrange_multi
);


ALTER TYPE public.myrange OWNER TO postgres;

--
-- Name: pointdom; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16860'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16859'::pg_catalog.oid);

CREATE DOMAIN public.pointdom AS public.point2d;


ALTER DOMAIN public.pointdom OWNER TO postgres;

--
-- Name: rangedom; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16865'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16864'::pg_catalog.oid);

CREATE DOMAIN public.rangedom AS public.myrange;


ALTER DOMAIN public.rangedom OWNER TO postgres;

--
-- Name: shellonly; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16792'::pg_catalog.oid);

CREATE TYPE public.shellonly;


ALTER TYPE public.shellonly OWNER TO postgres;

--
-- Name: tagged; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16774'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16773'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16772'::pg_catalog.oid);

CREATE TYPE public.tagged AS (
	label text,
	tags text[]
);


ALTER TYPE public.tagged OWNER TO postgres;

--
-- Name: text_c; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16715'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16714'::pg_catalog.oid);

CREATE DOMAIN public.text_c AS text COLLATE pg_catalog."C";


ALTER DOMAIN public.text_c OWNER TO postgres;

--
-- Name: textrange; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16833'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16830'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16831'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16832'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16750'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16749'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16748'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16748'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16751'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16751'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16752'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16752'::pg_catalog.oid);

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
SET relfrozenxid = '870', relminmxid = '1'
WHERE oid = 'public.t_array'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '870', relminmxid = '1'
WHERE oid = '16751';


ALTER TABLE public.t_array OWNER TO postgres;

--
-- Name: t_array_shape; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16757'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16756'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16755'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16755'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16758'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16758'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16759'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16759'::pg_catalog.oid);

CREATE TABLE public.t_array_shape (
    id integer NOT NULL,
    v_multidim integer[],
    v_mixed_dim integer[],
    v_lbound integer[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '872', relminmxid = '1'
WHERE oid = 'public.t_array_shape'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '872', relminmxid = '1'
WHERE oid = '16758';


ALTER TABLE public.t_array_shape OWNER TO postgres;

--
-- Name: t_array_spelling; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16764'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16763'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16762'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16762'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16765'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16765'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16766'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16766'::pg_catalog.oid);

CREATE TABLE public.t_array_spelling (
    id integer NOT NULL,
    v_bounded integer[],
    v_bounded_2d integer[],
    v_array_kw integer[],
    v_array_kw_n integer[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '874', relminmxid = '1'
WHERE oid = 'public.t_array_spelling'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '874', relminmxid = '1'
WHERE oid = '16765';


ALTER TABLE public.t_array_spelling OWNER TO postgres;

--
-- Name: t_base_type; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16799'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16798'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16797'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16797'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16800'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16800'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16801'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16801'::pg_catalog.oid);

CREATE TABLE public.t_base_type (
    id integer NOT NULL,
    v_mybase public.mybase,
    v_mybase_array public.mybase[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '888', relminmxid = '1'
WHERE oid = 'public.t_base_type'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '888', relminmxid = '1'
WHERE oid = '16800';


ALTER TABLE public.t_base_type OWNER TO postgres;

--
-- Name: t_bytea; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16697'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16696'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16695'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16695'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16698'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16698'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16699'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16699'::pg_catalog.oid);

CREATE TABLE public.t_bytea (
    id integer NOT NULL,
    v_bytea bytea
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '853', relminmxid = '1'
WHERE oid = 'public.t_bytea'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '853', relminmxid = '1'
WHERE oid = '16698';


ALTER TABLE public.t_bytea OWNER TO postgres;

--
-- Name: t_collate; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16722'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16721'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16720'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16720'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16725'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16725'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16726'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16726'::pg_catalog.oid);

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
    v_src text,
    v_gen_nn text GENERATED ALWAYS AS (upper(COALESCE(v_src, ''::text))) STORED NOT NULL COLLATE pg_catalog."C"
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '862', relminmxid = '1'
WHERE oid = 'public.t_collate'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '862', relminmxid = '1'
WHERE oid = '16725';


ALTER TABLE public.t_collate OWNER TO postgres;

--
-- Name: t_composite; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16780'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16779'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16778'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16778'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16781'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16781'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16782'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16782'::pg_catalog.oid);

CREATE TABLE public.t_composite (
    id integer NOT NULL,
    v_point public.point2d,
    v_points public.point2d[],
    v_tagged public.tagged,
    v_empty_comp public.empty_comp
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '879', relminmxid = '1'
WHERE oid = 'public.t_composite'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '879', relminmxid = '1'
WHERE oid = '16781';


ALTER TABLE public.t_composite OWNER TO postgres;

--
-- Name: t_date; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16677'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16676'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16675'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16675'::pg_catalog.oid);

CREATE TABLE public.t_date (
    id integer NOT NULL,
    v_date date
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '845', relminmxid = '1'
WHERE oid = 'public.t_date'::pg_catalog.regclass;


ALTER TABLE public.t_date OWNER TO postgres;

--
-- Name: t_delimiter; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16808'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16807'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16806'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16806'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16809'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16809'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16810'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16810'::pg_catalog.oid);

CREATE TABLE public.t_delimiter (
    id integer NOT NULL,
    v_box_domain public.box_domain,
    v_box_domain_array public.box_domain[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '891', relminmxid = '1'
WHERE oid = 'public.t_delimiter'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '891', relminmxid = '1'
WHERE oid = '16809';


ALTER TABLE public.t_delimiter OWNER TO postgres;

--
-- Name: t_enum_domain; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16745'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16744'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16743'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16743'::pg_catalog.oid);

CREATE TABLE public.t_enum_domain (
    id integer NOT NULL,
    v_mood public.mood,
    v_domain public.derived_domain,
    v_empty_enum public.empty_enum
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '868', relminmxid = '1'
WHERE oid = 'public.t_enum_domain'::pg_catalog.regclass;


ALTER TABLE public.t_enum_domain OWNER TO postgres;

--
-- Name: t_float; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16672'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16671'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16670'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16670'::pg_catalog.oid);

CREATE TABLE public.t_float (
    id integer NOT NULL,
    v_real real,
    v_double double precision
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '843', relminmxid = '1'
WHERE oid = 'public.t_float'::pg_catalog.regclass;


ALTER TABLE public.t_float OWNER TO postgres;

--
-- Name: t_int; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16655'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16654'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16653'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16653'::pg_catalog.oid);

CREATE TABLE public.t_int (
    id integer NOT NULL,
    v_smallint smallint,
    v_integer integer,
    v_bigint bigint
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '837', relminmxid = '1'
WHERE oid = 'public.t_int'::pg_catalog.regclass;


ALTER TABLE public.t_int OWNER TO postgres;

--
-- Name: t_interval; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16692'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16691'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16690'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16690'::pg_catalog.oid);

CREATE TABLE public.t_interval (
    id integer NOT NULL,
    v_interval interval
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '851', relminmxid = '1'
WHERE oid = 'public.t_interval'::pg_catalog.regclass;


ALTER TABLE public.t_interval OWNER TO postgres;

--
-- Name: t_json; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16731'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16730'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16729'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16729'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16732'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16732'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16733'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16733'::pg_catalog.oid);

CREATE TABLE public.t_json (
    id integer NOT NULL,
    v_json json,
    v_jsonb jsonb
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '864', relminmxid = '1'
WHERE oid = 'public.t_json'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '864', relminmxid = '1'
WHERE oid = '16732';


ALTER TABLE public.t_json OWNER TO postgres;

--
-- Name: t_multirange; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16849'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16848'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16847'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16847'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16850'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16850'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16851'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16851'::pg_catalog.oid);

CREATE TABLE public.t_multirange (
    id integer NOT NULL,
    v_int4multirange int4multirange,
    v_myrange_multi public.myrange_multi
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '899', relminmxid = '1'
WHERE oid = 'public.t_multirange'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '899', relminmxid = '1'
WHERE oid = '16850';


ALTER TABLE public.t_multirange OWNER TO postgres;

--
-- Name: t_nested_array; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16868'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16867'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16866'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16866'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16869'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16869'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16870'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16870'::pg_catalog.oid);

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
SET relfrozenxid = '906', relminmxid = '1'
WHERE oid = 'public.t_nested_array'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '906', relminmxid = '1'
WHERE oid = '16869';


ALTER TABLE public.t_nested_array OWNER TO postgres;

--
-- Name: t_net; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16738'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16737'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16736'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16736'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16739'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16739'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16740'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16740'::pg_catalog.oid);

CREATE TABLE public.t_net (
    id integer NOT NULL,
    v_inet inet,
    v_cidr cidr,
    v_macaddr macaddr,
    v_macaddr8 macaddr8
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '866', relminmxid = '1'
WHERE oid = 'public.t_net'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '866', relminmxid = '1'
WHERE oid = '16739';


ALTER TABLE public.t_net OWNER TO postgres;

--
-- Name: t_numeric; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16665'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16664'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16663'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16663'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16666'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16666'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16667'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16667'::pg_catalog.oid);

CREATE TABLE public.t_numeric (
    id integer NOT NULL,
    v_typed numeric(38,10),
    v_typed39 numeric(39,10),
    v_small numeric(10,2),
    v_untyped numeric
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '841', relminmxid = '1'
WHERE oid = 'public.t_numeric'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '841', relminmxid = '1'
WHERE oid = '16666';


ALTER TABLE public.t_numeric OWNER TO postgres;

--
-- Name: t_oid; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16660'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16659'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16658'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16658'::pg_catalog.oid);

CREATE TABLE public.t_oid (
    id integer NOT NULL,
    v_oid oid
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '839', relminmxid = '1'
WHERE oid = 'public.t_oid'::pg_catalog.regclass;


ALTER TABLE public.t_oid OWNER TO postgres;

--
-- Name: t_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16787'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16786'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16785'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16785'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16788'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16788'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16789'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16789'::pg_catalog.oid);

CREATE TABLE public.t_range (
    id integer NOT NULL,
    v_range int4range
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '881', relminmxid = '1'
WHERE oid = 'public.t_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '881', relminmxid = '1'
WHERE oid = '16788';


ALTER TABLE public.t_range OWNER TO postgres;

--
-- Name: t_text; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16709'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16708'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16707'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16707'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16710'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16710'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16711'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16711'::pg_catalog.oid);

CREATE TABLE public.t_text (
    id integer NOT NULL,
    v_text text,
    v_varchar character varying(10),
    v_char character(10)
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '857', relminmxid = '1'
WHERE oid = 'public.t_text'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '857', relminmxid = '1'
WHERE oid = '16710';


ALTER TABLE public.t_text OWNER TO postgres;

--
-- Name: t_text_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16842'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16841'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16840'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16840'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16843'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16843'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16844'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16844'::pg_catalog.oid);

CREATE TABLE public.t_text_range (
    id integer NOT NULL,
    v_textrange public.textrange
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '897', relminmxid = '1'
WHERE oid = 'public.t_text_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '897', relminmxid = '1'
WHERE oid = '16843';


ALTER TABLE public.t_text_range OWNER TO postgres;

--
-- Name: t_time; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16687'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16686'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16685'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16685'::pg_catalog.oid);

CREATE TABLE public.t_time (
    id integer NOT NULL,
    v_time time without time zone,
    v_timetz time with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '849', relminmxid = '1'
WHERE oid = 'public.t_time'::pg_catalog.regclass;


ALTER TABLE public.t_time OWNER TO postgres;

--
-- Name: t_timestamp; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16682'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16681'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16680'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16680'::pg_catalog.oid);

CREATE TABLE public.t_timestamp (
    id integer NOT NULL,
    v_ts timestamp without time zone,
    v_tstz timestamp with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '847', relminmxid = '1'
WHERE oid = 'public.t_timestamp'::pg_catalog.regclass;


ALTER TABLE public.t_timestamp OWNER TO postgres;

--
-- Name: t_user_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16825'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16824'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16823'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16823'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16826'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16826'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16827'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16827'::pg_catalog.oid);

CREATE TABLE public.t_user_range (
    id integer NOT NULL,
    v_myrange public.myrange
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '894', relminmxid = '1'
WHERE oid = 'public.t_user_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '894', relminmxid = '1'
WHERE oid = '16826';


ALTER TABLE public.t_user_range OWNER TO postgres;

--
-- Name: t_uuid; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16704'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16703'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16702'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16702'::pg_catalog.oid);

CREATE TABLE public.t_uuid (
    id integer NOT NULL,
    v_uuid uuid
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '855', relminmxid = '1'
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

COPY public.t_collate (id, v_text_c, v_text_locale, v_text_ucs, v_name, v_domain_c, v_pair, v_text_def, v_user, v_src) FROM stdin;
1	A	A	A	A	A	(A,A)	A	A	A
2	a	a	a	a	a	(a,a)	a	a	a
3	B	B	B	B	B	(B,B)	B	B	B
4	é	é	é	é	é	(é,é)	é	é	é
5	f	f	f	f	f	(f,f)	f	f	f
6	_x	_x	_x	_x	_x	(_x,_x)	_x	_x	_x
7	ax	ax	ax	ax	ax	(ax,ax)	ax	ax	ax
8						("","")			
9	\N	\N	\N	\N	\N	\N	\N	\N	\N
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


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16753'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16753'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array
    ADD CONSTRAINT t_array_pkey PRIMARY KEY (id);


--
-- Name: t_array_shape t_array_shape_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16760'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16760'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array_shape
    ADD CONSTRAINT t_array_shape_pkey PRIMARY KEY (id);


--
-- Name: t_array_spelling t_array_spelling_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16767'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16767'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array_spelling
    ADD CONSTRAINT t_array_spelling_pkey PRIMARY KEY (id);


--
-- Name: t_base_type t_base_type_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16802'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16802'::pg_catalog.oid);

ALTER TABLE ONLY public.t_base_type
    ADD CONSTRAINT t_base_type_pkey PRIMARY KEY (id);


--
-- Name: t_bytea t_bytea_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16700'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16700'::pg_catalog.oid);

ALTER TABLE ONLY public.t_bytea
    ADD CONSTRAINT t_bytea_pkey PRIMARY KEY (id);


--
-- Name: t_collate t_collate_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16727'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16727'::pg_catalog.oid);

ALTER TABLE ONLY public.t_collate
    ADD CONSTRAINT t_collate_pkey PRIMARY KEY (id);


--
-- Name: t_composite t_composite_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16783'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16783'::pg_catalog.oid);

ALTER TABLE ONLY public.t_composite
    ADD CONSTRAINT t_composite_pkey PRIMARY KEY (id);


--
-- Name: t_date t_date_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16678'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16678'::pg_catalog.oid);

ALTER TABLE ONLY public.t_date
    ADD CONSTRAINT t_date_pkey PRIMARY KEY (id);


--
-- Name: t_delimiter t_delimiter_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16811'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16811'::pg_catalog.oid);

ALTER TABLE ONLY public.t_delimiter
    ADD CONSTRAINT t_delimiter_pkey PRIMARY KEY (id);


--
-- Name: t_enum_domain t_enum_domain_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16746'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16746'::pg_catalog.oid);

ALTER TABLE ONLY public.t_enum_domain
    ADD CONSTRAINT t_enum_domain_pkey PRIMARY KEY (id);


--
-- Name: t_float t_float_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16673'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16673'::pg_catalog.oid);

ALTER TABLE ONLY public.t_float
    ADD CONSTRAINT t_float_pkey PRIMARY KEY (id);


--
-- Name: t_int t_int_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16656'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16656'::pg_catalog.oid);

ALTER TABLE ONLY public.t_int
    ADD CONSTRAINT t_int_pkey PRIMARY KEY (id);


--
-- Name: t_interval t_interval_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16693'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16693'::pg_catalog.oid);

ALTER TABLE ONLY public.t_interval
    ADD CONSTRAINT t_interval_pkey PRIMARY KEY (id);


--
-- Name: t_json t_json_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16734'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16734'::pg_catalog.oid);

ALTER TABLE ONLY public.t_json
    ADD CONSTRAINT t_json_pkey PRIMARY KEY (id);


--
-- Name: t_multirange t_multirange_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16852'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16852'::pg_catalog.oid);

ALTER TABLE ONLY public.t_multirange
    ADD CONSTRAINT t_multirange_pkey PRIMARY KEY (id);


--
-- Name: t_nested_array t_nested_array_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16871'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16871'::pg_catalog.oid);

ALTER TABLE ONLY public.t_nested_array
    ADD CONSTRAINT t_nested_array_pkey PRIMARY KEY (id);


--
-- Name: t_net t_net_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16741'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16741'::pg_catalog.oid);

ALTER TABLE ONLY public.t_net
    ADD CONSTRAINT t_net_pkey PRIMARY KEY (id);


--
-- Name: t_numeric t_numeric_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16668'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16668'::pg_catalog.oid);

ALTER TABLE ONLY public.t_numeric
    ADD CONSTRAINT t_numeric_pkey PRIMARY KEY (id);


--
-- Name: t_oid t_oid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16661'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16661'::pg_catalog.oid);

ALTER TABLE ONLY public.t_oid
    ADD CONSTRAINT t_oid_pkey PRIMARY KEY (id);


--
-- Name: t_range t_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16790'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16790'::pg_catalog.oid);

ALTER TABLE ONLY public.t_range
    ADD CONSTRAINT t_range_pkey PRIMARY KEY (id);


--
-- Name: t_text t_text_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16712'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16712'::pg_catalog.oid);

ALTER TABLE ONLY public.t_text
    ADD CONSTRAINT t_text_pkey PRIMARY KEY (id);


--
-- Name: t_text_range t_text_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16845'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16845'::pg_catalog.oid);

ALTER TABLE ONLY public.t_text_range
    ADD CONSTRAINT t_text_range_pkey PRIMARY KEY (id);


--
-- Name: t_time t_time_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16688'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16688'::pg_catalog.oid);

ALTER TABLE ONLY public.t_time
    ADD CONSTRAINT t_time_pkey PRIMARY KEY (id);


--
-- Name: t_timestamp t_timestamp_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16683'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16683'::pg_catalog.oid);

ALTER TABLE ONLY public.t_timestamp
    ADD CONSTRAINT t_timestamp_pkey PRIMARY KEY (id);


--
-- Name: t_user_range t_user_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16828'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16828'::pg_catalog.oid);

ALTER TABLE ONLY public.t_user_range
    ADD CONSTRAINT t_user_range_pkey PRIMARY KEY (id);


--
-- Name: t_uuid t_uuid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16705'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16705'::pg_catalog.oid);

ALTER TABLE ONLY public.t_uuid
    ADD CONSTRAINT t_uuid_pkey PRIMARY KEY (id);


--
-- PostgreSQL database dump complete
--

\unrestrict xt6kZhKX8fk38sVpKJjkwPwvtaAIib4uQCOcvvlHdBcMbTHi1IteeaaoizAcIjG

