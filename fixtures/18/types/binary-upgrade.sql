--
-- PostgreSQL database dump
--

\restrict KWSlll2feuDa89bIIZqYEbmgMah0J618wUS9uNT52uFmsHzJy9R5obPbFBXZufX

-- Dumped from database version 18.6
-- Dumped by pg_dump version 18.6

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
-- Name: base_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16458'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16457'::pg_catalog.oid);

CREATE DOMAIN public.base_domain AS integer;


ALTER DOMAIN public.base_domain OWNER TO postgres;

--
-- Name: derived_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16460'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16459'::pg_catalog.oid);

CREATE DOMAIN public.derived_domain AS public.base_domain NOT NULL;


ALTER DOMAIN public.derived_domain OWNER TO postgres;

--
-- Name: mood; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16444'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16443'::pg_catalog.oid);

CREATE TYPE public.mood AS ENUM (
);

-- For binary upgrade, must preserve pg_enum oids
SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16446'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'sad';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16448'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'ok';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16450'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'happy';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16452'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has space';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16454'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has,comma';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16456'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has''quote';



ALTER TYPE public.mood OWNER TO postgres;

--
-- Name: mybase; Type: SHELL TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16578'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16581'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16578'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16581'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16593'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16590'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16591'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16592'::pg_catalog.oid);

CREATE TYPE public.myrange AS RANGE (
    subtype = double precision,
    multirange_type_name = public.myrange_multi
);


ALTER TYPE public.myrange OWNER TO postgres;

--
-- Name: point2d; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16560'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16559'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16558'::pg_catalog.oid);

CREATE TYPE public.point2d AS (
	x integer,
	y text
);


ALTER TYPE public.point2d OWNER TO postgres;

--
-- Name: shellonly; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16577'::pg_catalog.oid);

CREATE TYPE public.shellonly;


ALTER TYPE public.shellonly OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: t_array; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16552'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16551'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16550'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16550'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16554'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16554'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16555'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16555'::pg_catalog.oid);

CREATE TABLE public.t_array (
    id integer NOT NULL,
    v_empty integer[],
    v_with_null integer[],
    v_null_array integer[],
    v_text_special text[],
    v_multidim integer[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '801', relminmxid = '1'
WHERE oid = 'public.t_array'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '801', relminmxid = '1'
WHERE oid = '16554';


ALTER TABLE public.t_array OWNER TO postgres;

--
-- Name: t_base_type; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16584'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16583'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16582'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16582'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16586'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16586'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16587'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16587'::pg_catalog.oid);

CREATE TABLE public.t_base_type (
    id integer NOT NULL,
    v_mybase public.mybase
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '813', relminmxid = '1'
WHERE oid = 'public.t_base_type'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '813', relminmxid = '1'
WHERE oid = '16586';


ALTER TABLE public.t_base_type OWNER TO postgres;

--
-- Name: t_bytea; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16508'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16507'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16506'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16506'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16510'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16510'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16511'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16511'::pg_catalog.oid);

CREATE TABLE public.t_bytea (
    id integer NOT NULL,
    v_bytea bytea
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '789', relminmxid = '1'
WHERE oid = 'public.t_bytea'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '789', relminmxid = '1'
WHERE oid = '16510';


ALTER TABLE public.t_bytea OWNER TO postgres;

--
-- Name: t_composite; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16563'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16562'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16561'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16561'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16565'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16565'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16566'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16566'::pg_catalog.oid);

CREATE TABLE public.t_composite (
    id integer NOT NULL,
    v_point public.point2d
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '804', relminmxid = '1'
WHERE oid = 'public.t_composite'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '804', relminmxid = '1'
WHERE oid = '16565';


ALTER TABLE public.t_composite OWNER TO postgres;

--
-- Name: t_date; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16484'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16483'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16482'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16482'::pg_catalog.oid);

CREATE TABLE public.t_date (
    id integer NOT NULL,
    v_date date
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '781', relminmxid = '1'
WHERE oid = 'public.t_date'::pg_catalog.regclass;


ALTER TABLE public.t_date OWNER TO postgres;

--
-- Name: t_enum_domain; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16546'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16545'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16544'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16544'::pg_catalog.oid);

CREATE TABLE public.t_enum_domain (
    id integer NOT NULL,
    v_mood public.mood,
    v_domain public.derived_domain
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '799', relminmxid = '1'
WHERE oid = 'public.t_enum_domain'::pg_catalog.regclass;


ALTER TABLE public.t_enum_domain OWNER TO postgres;

--
-- Name: t_float; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16478'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16477'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16476'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16476'::pg_catalog.oid);

CREATE TABLE public.t_float (
    id integer NOT NULL,
    v_real real,
    v_double double precision
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '779', relminmxid = '1'
WHERE oid = 'public.t_float'::pg_catalog.regclass;


ALTER TABLE public.t_float OWNER TO postgres;

--
-- Name: t_int; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16464'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16463'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16462'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16462'::pg_catalog.oid);

CREATE TABLE public.t_int (
    id integer NOT NULL,
    v_smallint smallint,
    v_integer integer,
    v_bigint bigint
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '775', relminmxid = '1'
WHERE oid = 'public.t_int'::pg_catalog.regclass;


ALTER TABLE public.t_int OWNER TO postgres;

--
-- Name: t_interval; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16502'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16501'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16500'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16500'::pg_catalog.oid);

CREATE TABLE public.t_interval (
    id integer NOT NULL,
    v_interval interval
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '787', relminmxid = '1'
WHERE oid = 'public.t_interval'::pg_catalog.regclass;


ALTER TABLE public.t_interval OWNER TO postgres;

--
-- Name: t_json; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16530'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16529'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16528'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16528'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16532'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16532'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16533'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16533'::pg_catalog.oid);

CREATE TABLE public.t_json (
    id integer NOT NULL,
    v_json json,
    v_jsonb jsonb
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '795', relminmxid = '1'
WHERE oid = 'public.t_json'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '795', relminmxid = '1'
WHERE oid = '16532';


ALTER TABLE public.t_json OWNER TO postgres;

--
-- Name: t_multirange; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16610'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16609'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16608'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16608'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16612'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16612'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16613'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16613'::pg_catalog.oid);

CREATE TABLE public.t_multirange (
    id integer NOT NULL,
    v_int4multirange int4multirange,
    v_myrange_multi public.myrange_multi
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '818', relminmxid = '1'
WHERE oid = 'public.t_multirange'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '818', relminmxid = '1'
WHERE oid = '16612';


ALTER TABLE public.t_multirange OWNER TO postgres;

--
-- Name: t_net; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16538'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16537'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16536'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16536'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16540'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16540'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16541'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16541'::pg_catalog.oid);

CREATE TABLE public.t_net (
    id integer NOT NULL,
    v_inet inet,
    v_cidr cidr,
    v_macaddr macaddr,
    v_macaddr8 macaddr8
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '797', relminmxid = '1'
WHERE oid = 'public.t_net'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '797', relminmxid = '1'
WHERE oid = '16540';


ALTER TABLE public.t_net OWNER TO postgres;

--
-- Name: t_numeric; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16470'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16469'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16468'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16468'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16472'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16472'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16473'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16473'::pg_catalog.oid);

CREATE TABLE public.t_numeric (
    id integer NOT NULL,
    v_typed numeric(38,10),
    v_typed39 numeric(39,10),
    v_small numeric(10,2),
    v_untyped numeric
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '777', relminmxid = '1'
WHERE oid = 'public.t_numeric'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '777', relminmxid = '1'
WHERE oid = '16472';


ALTER TABLE public.t_numeric OWNER TO postgres;

--
-- Name: t_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16571'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16570'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16569'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16569'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16573'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16573'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16574'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16574'::pg_catalog.oid);

CREATE TABLE public.t_range (
    id integer NOT NULL,
    v_range int4range
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '806', relminmxid = '1'
WHERE oid = 'public.t_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '806', relminmxid = '1'
WHERE oid = '16573';


ALTER TABLE public.t_range OWNER TO postgres;

--
-- Name: t_text; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16522'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16521'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16520'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16520'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16524'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16524'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16525'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16525'::pg_catalog.oid);

CREATE TABLE public.t_text (
    id integer NOT NULL,
    v_text text,
    v_varchar character varying(10),
    v_char character(10)
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '793', relminmxid = '1'
WHERE oid = 'public.t_text'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '793', relminmxid = '1'
WHERE oid = '16524';


ALTER TABLE public.t_text OWNER TO postgres;

--
-- Name: t_time; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16496'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16495'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16494'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16494'::pg_catalog.oid);

CREATE TABLE public.t_time (
    id integer NOT NULL,
    v_time time without time zone,
    v_timetz time with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '785', relminmxid = '1'
WHERE oid = 'public.t_time'::pg_catalog.regclass;


ALTER TABLE public.t_time OWNER TO postgres;

--
-- Name: t_timestamp; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16490'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16489'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16488'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16488'::pg_catalog.oid);

CREATE TABLE public.t_timestamp (
    id integer NOT NULL,
    v_ts timestamp without time zone,
    v_tstz timestamp with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '783', relminmxid = '1'
WHERE oid = 'public.t_timestamp'::pg_catalog.regclass;


ALTER TABLE public.t_timestamp OWNER TO postgres;

--
-- Name: t_user_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16602'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16601'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16600'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16600'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16604'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16604'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16605'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16605'::pg_catalog.oid);

CREATE TABLE public.t_user_range (
    id integer NOT NULL,
    v_myrange public.myrange
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '816', relminmxid = '1'
WHERE oid = 'public.t_user_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '816', relminmxid = '1'
WHERE oid = '16604';


ALTER TABLE public.t_user_range OWNER TO postgres;

--
-- Name: t_uuid; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16516'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16515'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16514'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16514'::pg_catalog.oid);

CREATE TABLE public.t_uuid (
    id integer NOT NULL,
    v_uuid uuid
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '791', relminmxid = '1'
WHERE oid = 'public.t_uuid'::pg_catalog.regclass;


ALTER TABLE public.t_uuid OWNER TO postgres;

--
-- Data for Name: t_array; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_array (id, v_empty, v_with_null, v_null_array, v_text_special, v_multidim) FROM stdin;
1	{}	{NULL}	\N	{"a,b","c{d}","e\\"f","g\\\\h"}	{{1,2},{3,4}}
2	{1,2,3}	{1,NULL,3}	{1,2}	{NULL,plain}	\N
\.


--
-- Data for Name: t_base_type; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_base_type (id, v_mybase) FROM stdin;
1	hello
2	\N
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
-- Data for Name: t_composite; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_composite (id, v_point) FROM stdin;
1	(1,"a,b""c")
2	\N
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
-- Data for Name: t_enum_domain; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_enum_domain (id, v_mood, v_domain) FROM stdin;
1	sad	5
2	has space	0
3	has,comma	-5
4	has'quote	100
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
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16556'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16556'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array
    ADD CONSTRAINT t_array_pkey PRIMARY KEY (id);


--
-- Name: t_base_type t_base_type_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16588'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16588'::pg_catalog.oid);

ALTER TABLE ONLY public.t_base_type
    ADD CONSTRAINT t_base_type_pkey PRIMARY KEY (id);


--
-- Name: t_bytea t_bytea_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16512'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16512'::pg_catalog.oid);

ALTER TABLE ONLY public.t_bytea
    ADD CONSTRAINT t_bytea_pkey PRIMARY KEY (id);


--
-- Name: t_composite t_composite_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16567'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16567'::pg_catalog.oid);

ALTER TABLE ONLY public.t_composite
    ADD CONSTRAINT t_composite_pkey PRIMARY KEY (id);


--
-- Name: t_date t_date_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16486'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16486'::pg_catalog.oid);

ALTER TABLE ONLY public.t_date
    ADD CONSTRAINT t_date_pkey PRIMARY KEY (id);


--
-- Name: t_enum_domain t_enum_domain_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16548'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16548'::pg_catalog.oid);

ALTER TABLE ONLY public.t_enum_domain
    ADD CONSTRAINT t_enum_domain_pkey PRIMARY KEY (id);


--
-- Name: t_float t_float_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16480'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16480'::pg_catalog.oid);

ALTER TABLE ONLY public.t_float
    ADD CONSTRAINT t_float_pkey PRIMARY KEY (id);


--
-- Name: t_int t_int_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16466'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16466'::pg_catalog.oid);

ALTER TABLE ONLY public.t_int
    ADD CONSTRAINT t_int_pkey PRIMARY KEY (id);


--
-- Name: t_interval t_interval_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16504'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16504'::pg_catalog.oid);

ALTER TABLE ONLY public.t_interval
    ADD CONSTRAINT t_interval_pkey PRIMARY KEY (id);


--
-- Name: t_json t_json_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16534'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16534'::pg_catalog.oid);

ALTER TABLE ONLY public.t_json
    ADD CONSTRAINT t_json_pkey PRIMARY KEY (id);


--
-- Name: t_multirange t_multirange_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16614'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16614'::pg_catalog.oid);

ALTER TABLE ONLY public.t_multirange
    ADD CONSTRAINT t_multirange_pkey PRIMARY KEY (id);


--
-- Name: t_net t_net_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16542'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16542'::pg_catalog.oid);

ALTER TABLE ONLY public.t_net
    ADD CONSTRAINT t_net_pkey PRIMARY KEY (id);


--
-- Name: t_numeric t_numeric_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16474'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16474'::pg_catalog.oid);

ALTER TABLE ONLY public.t_numeric
    ADD CONSTRAINT t_numeric_pkey PRIMARY KEY (id);


--
-- Name: t_range t_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16575'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16575'::pg_catalog.oid);

ALTER TABLE ONLY public.t_range
    ADD CONSTRAINT t_range_pkey PRIMARY KEY (id);


--
-- Name: t_text t_text_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16526'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16526'::pg_catalog.oid);

ALTER TABLE ONLY public.t_text
    ADD CONSTRAINT t_text_pkey PRIMARY KEY (id);


--
-- Name: t_time t_time_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16498'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16498'::pg_catalog.oid);

ALTER TABLE ONLY public.t_time
    ADD CONSTRAINT t_time_pkey PRIMARY KEY (id);


--
-- Name: t_timestamp t_timestamp_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16492'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16492'::pg_catalog.oid);

ALTER TABLE ONLY public.t_timestamp
    ADD CONSTRAINT t_timestamp_pkey PRIMARY KEY (id);


--
-- Name: t_user_range t_user_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16606'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16606'::pg_catalog.oid);

ALTER TABLE ONLY public.t_user_range
    ADD CONSTRAINT t_user_range_pkey PRIMARY KEY (id);


--
-- Name: t_uuid t_uuid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16518'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16518'::pg_catalog.oid);

ALTER TABLE ONLY public.t_uuid
    ADD CONSTRAINT t_uuid_pkey PRIMARY KEY (id);


--
-- PostgreSQL database dump complete
--

\unrestrict KWSlll2feuDa89bIIZqYEbmgMah0J618wUS9uNT52uFmsHzJy9R5obPbFBXZufX

