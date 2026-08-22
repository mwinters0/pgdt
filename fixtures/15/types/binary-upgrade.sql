--
-- PostgreSQL database dump
--

\restrict HOmXCk7GTQbednefXSsoBIj3t20lI6jG6gh5mp8YTLMv9f7K3ygKYssEaWnTVmZ

-- Dumped from database version 15.19
-- Dumped by pg_dump version 15.19

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
-- Name: base_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16450'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16449'::pg_catalog.oid);

CREATE DOMAIN public.base_domain AS integer;


ALTER DOMAIN public.base_domain OWNER TO postgres;

--
-- Name: derived_domain; Type: DOMAIN; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16452'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16451'::pg_catalog.oid);

CREATE DOMAIN public.derived_domain AS public.base_domain NOT NULL;


ALTER DOMAIN public.derived_domain OWNER TO postgres;

--
-- Name: mood; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16437'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16436'::pg_catalog.oid);

CREATE TYPE public.mood AS ENUM (
);

-- For binary upgrade, must preserve pg_enum oids
SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16438'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'sad';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16440'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'ok';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16442'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'happy';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16444'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has space';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16446'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has,comma';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16448'::pg_catalog.oid);
ALTER TYPE public.mood ADD VALUE 'has''quote';



ALTER TYPE public.mood OWNER TO postgres;

--
-- Name: point2d; Type: TYPE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16537'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16536'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16535'::pg_catalog.oid);

CREATE TYPE public.point2d AS (
	x integer,
	y text
);


ALTER TYPE public.point2d OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: t_array; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16530'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16529'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16528'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16528'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16531'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16531'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16532'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16532'::pg_catalog.oid);

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
SET relfrozenxid = '773', relminmxid = '1'
WHERE oid = 'public.t_array'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '773', relminmxid = '1'
WHERE oid = '16531';


ALTER TABLE public.t_array OWNER TO postgres;

--
-- Name: t_bytea; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16492'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16491'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16490'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16490'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16493'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16493'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16494'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16494'::pg_catalog.oid);

CREATE TABLE public.t_bytea (
    id integer NOT NULL,
    v_bytea bytea
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '761', relminmxid = '1'
WHERE oid = 'public.t_bytea'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '761', relminmxid = '1'
WHERE oid = '16493';


ALTER TABLE public.t_bytea OWNER TO postgres;

--
-- Name: t_composite; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16540'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16539'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16538'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16538'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16541'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16541'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16542'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16542'::pg_catalog.oid);

CREATE TABLE public.t_composite (
    id integer NOT NULL,
    v_point public.point2d
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '776', relminmxid = '1'
WHERE oid = 'public.t_composite'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '776', relminmxid = '1'
WHERE oid = '16541';


ALTER TABLE public.t_composite OWNER TO postgres;

--
-- Name: t_date; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16472'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16471'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16470'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16470'::pg_catalog.oid);

CREATE TABLE public.t_date (
    id integer NOT NULL,
    v_date date
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '753', relminmxid = '1'
WHERE oid = 'public.t_date'::pg_catalog.regclass;


ALTER TABLE public.t_date OWNER TO postgres;

--
-- Name: t_enum_domain; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16525'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16524'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16523'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16523'::pg_catalog.oid);

CREATE TABLE public.t_enum_domain (
    id integer NOT NULL,
    v_mood public.mood,
    v_domain public.derived_domain
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '771', relminmxid = '1'
WHERE oid = 'public.t_enum_domain'::pg_catalog.regclass;


ALTER TABLE public.t_enum_domain OWNER TO postgres;

--
-- Name: t_float; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16467'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16466'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16465'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16465'::pg_catalog.oid);

CREATE TABLE public.t_float (
    id integer NOT NULL,
    v_real real,
    v_double double precision
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '751', relminmxid = '1'
WHERE oid = 'public.t_float'::pg_catalog.regclass;


ALTER TABLE public.t_float OWNER TO postgres;

--
-- Name: t_int; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16455'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16454'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16453'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16453'::pg_catalog.oid);

CREATE TABLE public.t_int (
    id integer NOT NULL,
    v_smallint smallint,
    v_integer integer,
    v_bigint bigint
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '747', relminmxid = '1'
WHERE oid = 'public.t_int'::pg_catalog.regclass;


ALTER TABLE public.t_int OWNER TO postgres;

--
-- Name: t_interval; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16487'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16486'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16485'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16485'::pg_catalog.oid);

CREATE TABLE public.t_interval (
    id integer NOT NULL,
    v_interval interval
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '759', relminmxid = '1'
WHERE oid = 'public.t_interval'::pg_catalog.regclass;


ALTER TABLE public.t_interval OWNER TO postgres;

--
-- Name: t_json; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16511'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16510'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16509'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16509'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16512'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16512'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16513'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16513'::pg_catalog.oid);

CREATE TABLE public.t_json (
    id integer NOT NULL,
    v_json json,
    v_jsonb jsonb
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '767', relminmxid = '1'
WHERE oid = 'public.t_json'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '767', relminmxid = '1'
WHERE oid = '16512';


ALTER TABLE public.t_json OWNER TO postgres;

--
-- Name: t_net; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16518'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16517'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16516'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16516'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16519'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16519'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16520'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16520'::pg_catalog.oid);

CREATE TABLE public.t_net (
    id integer NOT NULL,
    v_inet inet,
    v_cidr cidr,
    v_macaddr macaddr,
    v_macaddr8 macaddr8
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '769', relminmxid = '1'
WHERE oid = 'public.t_net'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '769', relminmxid = '1'
WHERE oid = '16519';


ALTER TABLE public.t_net OWNER TO postgres;

--
-- Name: t_numeric; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16460'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16459'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16458'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16458'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16461'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16461'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16462'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16462'::pg_catalog.oid);

CREATE TABLE public.t_numeric (
    id integer NOT NULL,
    v_typed numeric(38,10),
    v_typed39 numeric(39,10),
    v_small numeric(10,2),
    v_untyped numeric
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '749', relminmxid = '1'
WHERE oid = 'public.t_numeric'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '749', relminmxid = '1'
WHERE oid = '16461';


ALTER TABLE public.t_numeric OWNER TO postgres;

--
-- Name: t_range; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16547'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16546'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16545'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16545'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16548'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16548'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16549'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16549'::pg_catalog.oid);

CREATE TABLE public.t_range (
    id integer NOT NULL,
    v_range int4range
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '778', relminmxid = '1'
WHERE oid = 'public.t_range'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '778', relminmxid = '1'
WHERE oid = '16548';


ALTER TABLE public.t_range OWNER TO postgres;

--
-- Name: t_text; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16504'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16503'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16502'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16502'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16505'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_relfilenode('16505'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16506'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16506'::pg_catalog.oid);

CREATE TABLE public.t_text (
    id integer NOT NULL,
    v_text text,
    v_varchar character varying(10),
    v_char character(10)
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '765', relminmxid = '1'
WHERE oid = 'public.t_text'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '765', relminmxid = '1'
WHERE oid = '16505';


ALTER TABLE public.t_text OWNER TO postgres;

--
-- Name: t_time; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16482'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16481'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16480'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16480'::pg_catalog.oid);

CREATE TABLE public.t_time (
    id integer NOT NULL,
    v_time time without time zone,
    v_timetz time with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '757', relminmxid = '1'
WHERE oid = 'public.t_time'::pg_catalog.regclass;


ALTER TABLE public.t_time OWNER TO postgres;

--
-- Name: t_timestamp; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16477'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16476'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16475'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16475'::pg_catalog.oid);

CREATE TABLE public.t_timestamp (
    id integer NOT NULL,
    v_ts timestamp without time zone,
    v_tstz timestamp with time zone
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '755', relminmxid = '1'
WHERE oid = 'public.t_timestamp'::pg_catalog.regclass;


ALTER TABLE public.t_timestamp OWNER TO postgres;

--
-- Name: t_uuid; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16499'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16498'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16497'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_heap_relfilenode('16497'::pg_catalog.oid);

CREATE TABLE public.t_uuid (
    id integer NOT NULL,
    v_uuid uuid
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '763', relminmxid = '1'
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
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16533'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16533'::pg_catalog.oid);

ALTER TABLE ONLY public.t_array
    ADD CONSTRAINT t_array_pkey PRIMARY KEY (id);


--
-- Name: t_bytea t_bytea_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16495'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16495'::pg_catalog.oid);

ALTER TABLE ONLY public.t_bytea
    ADD CONSTRAINT t_bytea_pkey PRIMARY KEY (id);


--
-- Name: t_composite t_composite_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16543'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16543'::pg_catalog.oid);

ALTER TABLE ONLY public.t_composite
    ADD CONSTRAINT t_composite_pkey PRIMARY KEY (id);


--
-- Name: t_date t_date_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16473'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16473'::pg_catalog.oid);

ALTER TABLE ONLY public.t_date
    ADD CONSTRAINT t_date_pkey PRIMARY KEY (id);


--
-- Name: t_enum_domain t_enum_domain_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16526'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16526'::pg_catalog.oid);

ALTER TABLE ONLY public.t_enum_domain
    ADD CONSTRAINT t_enum_domain_pkey PRIMARY KEY (id);


--
-- Name: t_float t_float_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16468'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16468'::pg_catalog.oid);

ALTER TABLE ONLY public.t_float
    ADD CONSTRAINT t_float_pkey PRIMARY KEY (id);


--
-- Name: t_int t_int_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16456'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16456'::pg_catalog.oid);

ALTER TABLE ONLY public.t_int
    ADD CONSTRAINT t_int_pkey PRIMARY KEY (id);


--
-- Name: t_interval t_interval_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16488'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16488'::pg_catalog.oid);

ALTER TABLE ONLY public.t_interval
    ADD CONSTRAINT t_interval_pkey PRIMARY KEY (id);


--
-- Name: t_json t_json_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16514'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16514'::pg_catalog.oid);

ALTER TABLE ONLY public.t_json
    ADD CONSTRAINT t_json_pkey PRIMARY KEY (id);


--
-- Name: t_net t_net_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16521'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16521'::pg_catalog.oid);

ALTER TABLE ONLY public.t_net
    ADD CONSTRAINT t_net_pkey PRIMARY KEY (id);


--
-- Name: t_numeric t_numeric_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16463'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16463'::pg_catalog.oid);

ALTER TABLE ONLY public.t_numeric
    ADD CONSTRAINT t_numeric_pkey PRIMARY KEY (id);


--
-- Name: t_range t_range_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16550'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16550'::pg_catalog.oid);

ALTER TABLE ONLY public.t_range
    ADD CONSTRAINT t_range_pkey PRIMARY KEY (id);


--
-- Name: t_text t_text_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16507'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16507'::pg_catalog.oid);

ALTER TABLE ONLY public.t_text
    ADD CONSTRAINT t_text_pkey PRIMARY KEY (id);


--
-- Name: t_time t_time_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16483'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16483'::pg_catalog.oid);

ALTER TABLE ONLY public.t_time
    ADD CONSTRAINT t_time_pkey PRIMARY KEY (id);


--
-- Name: t_timestamp t_timestamp_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16478'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16478'::pg_catalog.oid);

ALTER TABLE ONLY public.t_timestamp
    ADD CONSTRAINT t_timestamp_pkey PRIMARY KEY (id);


--
-- Name: t_uuid t_uuid_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids and relfilenodes
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16500'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_relfilenode('16500'::pg_catalog.oid);

ALTER TABLE ONLY public.t_uuid
    ADD CONSTRAINT t_uuid_pkey PRIMARY KEY (id);


--
-- PostgreSQL database dump complete
--

\unrestrict HOmXCk7GTQbednefXSsoBIj3t20lI6jG6gh5mp8YTLMv9f7K3ygKYssEaWnTVmZ

