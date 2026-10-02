--
-- PostgreSQL database dump
--

\restrict sFVMIJpdIhKRqs5a2vQ8jasuYQcFimJ04s82EohdP17zy0fiXqB5xEXfQVHGEd2

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
-- Name: emitters; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA emitters;


ALTER SCHEMA emitters OWNER TO postgres;

--
-- Name: file_fdw; Type: EXTENSION; Schema: -; Owner: -
--

-- For binary upgrade, create an empty extension and insert objects into it
DROP EXTENSION IF EXISTS file_fdw;
SELECT pg_catalog.binary_upgrade_create_empty_extension('file_fdw', 'public', true, '1.0', NULL, NULL, ARRAY[]::pg_catalog.text[]);


--
-- Name: EXTENSION file_fdw; Type: COMMENT; Schema: -; Owner: 
--

COMMENT ON EXTENSION file_fdw IS 'foreign-data wrapper for flat file access';


--
-- Name: bt_char; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16404'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16407'::pg_catalog.oid);

CREATE TYPE emitters.bt_char;


--
-- Name: bt_char_in(cstring); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_char_in(cstring) RETURNS emitters.bt_char
    LANGUAGE internal IMMUTABLE STRICT
    AS $$charin$$;


ALTER FUNCTION emitters.bt_char_in(cstring) OWNER TO postgres;

--
-- Name: bt_char_out(emitters.bt_char); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_char_out(emitters.bt_char) RETURNS cstring
    LANGUAGE internal IMMUTABLE STRICT
    AS $$charout$$;


ALTER FUNCTION emitters.bt_char_out(emitters.bt_char) OWNER TO postgres;

--
-- Name: bt_char; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16404'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16407'::pg_catalog.oid);

CREATE TYPE emitters.bt_char (
    INTERNALLENGTH = 1,
    INPUT = emitters.bt_char_in,
    OUTPUT = emitters.bt_char_out,
    ALIGNMENT = char,
    STORAGE = plain,
    PASSEDBYVALUE
);


ALTER TYPE emitters.bt_char OWNER TO postgres;

--
-- Name: bt_int2; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16408'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16411'::pg_catalog.oid);

CREATE TYPE emitters.bt_int2;


--
-- Name: bt_int2_in(cstring); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_int2_in(cstring) RETURNS emitters.bt_int2
    LANGUAGE internal IMMUTABLE STRICT
    AS $$int2in$$;


ALTER FUNCTION emitters.bt_int2_in(cstring) OWNER TO postgres;

--
-- Name: bt_int2_out(emitters.bt_int2); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_int2_out(emitters.bt_int2) RETURNS cstring
    LANGUAGE internal IMMUTABLE STRICT
    AS $$int2out$$;


ALTER FUNCTION emitters.bt_int2_out(emitters.bt_int2) OWNER TO postgres;

--
-- Name: bt_int2; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16408'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16411'::pg_catalog.oid);

CREATE TYPE emitters.bt_int2 (
    INTERNALLENGTH = 2,
    INPUT = emitters.bt_int2_in,
    OUTPUT = emitters.bt_int2_out,
    ALIGNMENT = int2,
    STORAGE = plain,
    PASSEDBYVALUE
);


ALTER TYPE emitters.bt_int2 OWNER TO postgres;

--
-- Name: bt_pair; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16416'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16419'::pg_catalog.oid);

CREATE TYPE emitters.bt_pair;


--
-- Name: bt_pair_in(cstring); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_pair_in(cstring) RETURNS emitters.bt_pair
    LANGUAGE internal IMMUTABLE STRICT
    AS $$point_in$$;


ALTER FUNCTION emitters.bt_pair_in(cstring) OWNER TO postgres;

--
-- Name: bt_pair_out(emitters.bt_pair); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_pair_out(emitters.bt_pair) RETURNS cstring
    LANGUAGE internal IMMUTABLE STRICT
    AS $$point_out$$;


ALTER FUNCTION emitters.bt_pair_out(emitters.bt_pair) OWNER TO postgres;

--
-- Name: bt_pair; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16416'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16419'::pg_catalog.oid);

CREATE TYPE emitters.bt_pair (
    INTERNALLENGTH = 16,
    INPUT = emitters.bt_pair_in,
    OUTPUT = emitters.bt_pair_out,
    SUBSCRIPT = raw_array_subscript_handler,
    ELEMENT = double precision,
    ALIGNMENT = double,
    STORAGE = plain
);


ALTER TYPE emitters.bt_pair OWNER TO postgres;

--
-- Name: bt_text_main; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16412'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16415'::pg_catalog.oid);

CREATE TYPE emitters.bt_text_main;


--
-- Name: bt_text_main_in(cstring); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_text_main_in(cstring) RETURNS emitters.bt_text_main
    LANGUAGE internal IMMUTABLE STRICT
    AS $$textin$$;


ALTER FUNCTION emitters.bt_text_main_in(cstring) OWNER TO postgres;

--
-- Name: bt_text_main_out(emitters.bt_text_main); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_text_main_out(emitters.bt_text_main) RETURNS cstring
    LANGUAGE internal IMMUTABLE STRICT
    AS $$textout$$;


ALTER FUNCTION emitters.bt_text_main_out(emitters.bt_text_main) OWNER TO postgres;

--
-- Name: bt_text_main; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16412'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16415'::pg_catalog.oid);

CREATE TYPE emitters.bt_text_main (
    INTERNALLENGTH = variable,
    INPUT = emitters.bt_text_main_in,
    OUTPUT = emitters.bt_text_main_out,
    ALIGNMENT = int4,
    STORAGE = main
);


ALTER TYPE emitters.bt_text_main OWNER TO postgres;

--
-- Name: bt_varchar; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16395'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16403'::pg_catalog.oid);

CREATE TYPE emitters.bt_varchar;


--
-- Name: bt_varchar_analyze(internal); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_varchar_analyze(internal) RETURNS boolean
    LANGUAGE internal STRICT
    AS $$ts_typanalyze$$;


ALTER FUNCTION emitters.bt_varchar_analyze(internal) OWNER TO postgres;

--
-- Name: bt_varchar_in(cstring, oid, integer); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_varchar_in(cstring, oid, integer) RETURNS emitters.bt_varchar
    LANGUAGE internal IMMUTABLE STRICT
    AS $$varcharin$$;


ALTER FUNCTION emitters.bt_varchar_in(cstring, oid, integer) OWNER TO postgres;

--
-- Name: bt_varchar_out(emitters.bt_varchar); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_varchar_out(emitters.bt_varchar) RETURNS cstring
    LANGUAGE internal IMMUTABLE STRICT
    AS $$varcharout$$;


ALTER FUNCTION emitters.bt_varchar_out(emitters.bt_varchar) OWNER TO postgres;

--
-- Name: bt_varchar_recv(internal, oid, integer); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_varchar_recv(internal, oid, integer) RETURNS emitters.bt_varchar
    LANGUAGE internal STABLE STRICT
    AS $$varcharrecv$$;


ALTER FUNCTION emitters.bt_varchar_recv(internal, oid, integer) OWNER TO postgres;

--
-- Name: bt_varchar_send(emitters.bt_varchar); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_varchar_send(emitters.bt_varchar) RETURNS bytea
    LANGUAGE internal STABLE STRICT
    AS $$varcharsend$$;


ALTER FUNCTION emitters.bt_varchar_send(emitters.bt_varchar) OWNER TO postgres;

--
-- Name: bt_varchar_typmod_in(cstring[]); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_varchar_typmod_in(cstring[]) RETURNS integer
    LANGUAGE internal IMMUTABLE STRICT
    AS $$varchartypmodin$$;


ALTER FUNCTION emitters.bt_varchar_typmod_in(cstring[]) OWNER TO postgres;

--
-- Name: bt_varchar_typmod_out(integer); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.bt_varchar_typmod_out(integer) RETURNS cstring
    LANGUAGE internal IMMUTABLE STRICT
    AS $$varchartypmodout$$;


ALTER FUNCTION emitters.bt_varchar_typmod_out(integer) OWNER TO postgres;

--
-- Name: bt_varchar; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16395'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16403'::pg_catalog.oid);

CREATE TYPE emitters.bt_varchar (
    INTERNALLENGTH = variable,
    INPUT = emitters.bt_varchar_in,
    OUTPUT = emitters.bt_varchar_out,
    RECEIVE = emitters.bt_varchar_recv,
    SEND = emitters.bt_varchar_send,
    TYPMOD_IN = emitters.bt_varchar_typmod_in,
    TYPMOD_OUT = emitters.bt_varchar_typmod_out,
    ANALYZE = emitters.bt_varchar_analyze,
    COLLATABLE = true,
    DEFAULT = 'none',
    CATEGORY = 'S',
    PREFERRED = true,
    DELIMITER = ';',
    ALIGNMENT = int4,
    STORAGE = external
);


ALTER TYPE emitters.bt_varchar OWNER TO postgres;

--
-- Name: mood; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16469'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16468'::pg_catalog.oid);

CREATE TYPE emitters.mood AS ENUM (
);

-- For binary upgrade, must preserve pg_enum oids
SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16470'::pg_catalog.oid);
ALTER TYPE emitters.mood ADD VALUE 'calm';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16472'::pg_catalog.oid);
ALTER TYPE emitters.mood ADD VALUE 'busy';



ALTER TYPE emitters.mood OWNER TO postgres;

--
-- Name: pair; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16475'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16474'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16473'::pg_catalog.oid);

CREATE TYPE emitters.pair AS (
	left_part integer,
	right_part text
);


ALTER TYPE emitters.pair OWNER TO postgres;

--
-- Name: person; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16538'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16537'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16536'::pg_catalog.oid);

CREATE TYPE emitters.person AS (
	name text,
	born date,
	height integer
);


ALTER TYPE emitters.person OWNER TO postgres;

--
-- Name: positive; Type: DOMAIN; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16466'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16465'::pg_catalog.oid);

CREATE DOMAIN emitters.positive AS integer
	CONSTRAINT positive_check CHECK ((VALUE > 0));


ALTER DOMAIN emitters.positive OWNER TO postgres;

--
-- Name: r_canon; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16427'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16429'::pg_catalog.oid);

CREATE TYPE emitters.r_canon;


--
-- Name: r_canon_canonical(emitters.r_canon); Type: FUNCTION; Schema: emitters; Owner: postgres
--

CREATE FUNCTION emitters.r_canon_canonical(emitters.r_canon) RETURNS emitters.r_canon
    LANGUAGE internal IMMUTABLE STRICT
    AS $$int4range_canonical$$;


ALTER FUNCTION emitters.r_canon_canonical(emitters.r_canon) OWNER TO postgres;

--
-- Name: r_canon; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16427'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16429'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16430'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16431'::pg_catalog.oid);

CREATE TYPE emitters.r_canon AS RANGE (
    subtype = integer,
    multirange_type_name = emitters.r_canon_multirange,
    canonical = emitters.r_canon_canonical
);


ALTER TYPE emitters.r_canon OWNER TO postgres;

--
-- Name: r_diff; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16441'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16438'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16439'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16440'::pg_catalog.oid);

CREATE TYPE emitters.r_diff AS RANGE (
    subtype = double precision,
    multirange_type_name = emitters.r_diff_multirange,
    subtype_diff = float8mi
);


ALTER TYPE emitters.r_diff OWNER TO postgres;

--
-- Name: r_pattern; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16451'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16448'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16449'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16450'::pg_catalog.oid);

CREATE TYPE emitters.r_pattern AS RANGE (
    subtype = text,
    multirange_type_name = emitters.r_pattern_multirange,
    subtype_opclass = pg_catalog.text_pattern_ops
);


ALTER TYPE emitters.r_pattern OWNER TO postgres;

--
-- Name: trio; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16485'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16484'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16483'::pg_catalog.oid);

CREATE TYPE emitters.trio AS (
	a integer,
	"........pg.dropped.2........" INTEGER /* dummy */,
	c date
);

-- For binary upgrade, recreate dropped column.
UPDATE pg_catalog.pg_attribute
SET attlen = -1, attalign = 'i', attbyval = false
WHERE attname = '........pg.dropped.2........'
  AND attrelid = 'emitters.trio'::pg_catalog.regclass;
ALTER TYPE emitters.trio DROP ATTRIBUTE "........pg.dropped.2........";


ALTER TYPE emitters.trio OWNER TO postgres;

--
-- Name: file_fdw_handler(); Type: FUNCTION; Schema: public; Owner: postgres
--

CREATE FUNCTION public.file_fdw_handler() RETURNS fdw_handler
    LANGUAGE c STRICT
    AS '$libdir/file_fdw', 'file_fdw_handler';

-- For binary upgrade, handle extension membership the hard way
ALTER EXTENSION file_fdw ADD FUNCTION public.file_fdw_handler();


ALTER FUNCTION public.file_fdw_handler() OWNER TO postgres;

--
-- Name: file_fdw_validator(text[], oid); Type: FUNCTION; Schema: public; Owner: postgres
--

CREATE FUNCTION public.file_fdw_validator(text[], oid) RETURNS void
    LANGUAGE c STRICT
    AS '$libdir/file_fdw', 'file_fdw_validator';

-- For binary upgrade, handle extension membership the hard way
ALTER EXTENSION file_fdw ADD FUNCTION public.file_fdw_validator(text[], oid);


ALTER FUNCTION public.file_fdw_validator(text[], oid) OWNER TO postgres;

--
-- Name: file_fdw; Type: FOREIGN DATA WRAPPER; Schema: -; Owner: postgres
--

CREATE FOREIGN DATA WRAPPER file_fdw HANDLER public.file_fdw_handler VALIDATOR public.file_fdw_validator;

-- For binary upgrade, handle extension membership the hard way
ALTER EXTENSION file_fdw ADD FOREIGN DATA WRAPPER file_fdw;


ALTER FOREIGN DATA WRAPPER file_fdw OWNER TO postgres;

--
-- Name: emitters_files; Type: SERVER; Schema: -; Owner: postgres
--

CREATE SERVER emitters_files FOREIGN DATA WRAPPER file_fdw;


ALTER SERVER emitters_files OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: base_values; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16422'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16421'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16420'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16423'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16424'::pg_catalog.oid);

CREATE TABLE emitters.base_values (
    id integer NOT NULL,
    v_varchar emitters.bt_varchar(8),
    v_char emitters.bt_char,
    v_int2 emitters.bt_int2,
    v_main emitters.bt_text_main,
    v_pair emitters.bt_pair
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '767', relminmxid = '1'
WHERE oid = 'emitters.base_values'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '767', relminmxid = '1'
WHERE oid = '16423';


ALTER TABLE emitters.base_values OWNER TO postgres;

--
-- Name: parent; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16526'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16525'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16524'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16528'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16529'::pg_catalog.oid);

CREATE TABLE emitters.parent (
    id integer NOT NULL,
    label text,
    born date,
    CONSTRAINT parent_id_positive CHECK ((id > 0))
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '804', relminmxid = '1'
WHERE oid = 'emitters.parent'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '804', relminmxid = '1'
WHERE oid = '16528';


ALTER TABLE emitters.parent OWNER TO postgres;

--
-- Name: child; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16532'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16531'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16530'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16534'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16535'::pg_catalog.oid);

CREATE TABLE emitters.child (
    id integer NOT NULL,
    label text NOT NULL,
    born date,
    extra numeric(6,2)
);

-- For binary upgrade, recreate inherited column.
UPDATE pg_catalog.pg_attribute
SET attislocal = false
WHERE attname = 'id'
  AND attrelid = 'emitters.child'::pg_catalog.regclass;

-- For binary upgrade, recreate inherited column.
UPDATE pg_catalog.pg_attribute
SET attislocal = false
WHERE attname = 'label'
  AND attrelid = 'emitters.child'::pg_catalog.regclass;

-- For binary upgrade, recreate inherited column.
UPDATE pg_catalog.pg_attribute
SET attislocal = false
WHERE attname = 'born'
  AND attrelid = 'emitters.child'::pg_catalog.regclass;

-- For binary upgrade, set up inherited constraint.
ALTER TABLE ONLY emitters.child ADD CONSTRAINT parent_id_positive CHECK ((id > 0));
UPDATE pg_catalog.pg_constraint
SET conislocal = false
WHERE contype = 'c' AND conname = 'parent_id_positive'
  AND conrelid = 'emitters.child'::pg_catalog.regclass;

-- For binary upgrade, set up inheritance this way.
ALTER TABLE ONLY emitters.child INHERIT emitters.parent;

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '805', relminmxid = '1'
WHERE oid = 'emitters.child'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '805', relminmxid = '1'
WHERE oid = '16534';


ALTER TABLE emitters.child OWNER TO postgres;

--
-- Name: domain_values; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16478'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16477'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16476'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16479'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16480'::pg_catalog.oid);

CREATE TABLE emitters.domain_values (
    id integer NOT NULL,
    v_positive emitters.positive,
    v_mood emitters.mood,
    v_pair emitters.pair
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '779', relminmxid = '1'
WHERE oid = 'emitters.domain_values'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '779', relminmxid = '1'
WHERE oid = '16479';


ALTER TABLE emitters.domain_values OWNER TO postgres;

--
-- Name: external; Type: FOREIGN TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16551'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16550'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16549'::pg_catalog.oid);

CREATE FOREIGN TABLE emitters.external (
    id integer,
    "........pg.dropped.2........" INTEGER /* dummy */,
    label text
)
SERVER emitters_files
OPTIONS (
    filename '/tmp/emitters_external.tsv'
);

-- For binary upgrade, recreate dropped column.
UPDATE pg_catalog.pg_attribute
SET attlen = -1, attalign = 'i', attbyval = false
WHERE attname = '........pg.dropped.2........'
  AND attrelid = 'emitters.external'::pg_catalog.regclass;
ALTER FOREIGN TABLE ONLY emitters.external DROP COLUMN "........pg.dropped.2........";
ALTER FOREIGN TABLE emitters.external ALTER COLUMN label OPTIONS (
    force_not_null 'true'
);


ALTER FOREIGN TABLE emitters.external OWNER TO postgres;

--
-- Name: grown; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16515'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16514'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16513'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16516'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16517'::pg_catalog.oid);

CREATE TABLE emitters.grown (
    id integer NOT NULL,
    "........pg.dropped.2........" INTEGER /* dummy */,
    b text,
    "........pg.dropped.4........" INTEGER /* dummy */,
    added date DEFAULT '2024-02-29'::date
);

-- set missing value.
SELECT pg_catalog.binary_upgrade_set_missing_value('emitters.grown'::pg_catalog.regclass,'added','{2024-02-29}');


-- For binary upgrade, recreate dropped column.
UPDATE pg_catalog.pg_attribute
SET attlen = -1, attalign = 'i', attbyval = false
WHERE attname = '........pg.dropped.2........'
  AND attrelid = 'emitters.grown'::pg_catalog.regclass;
ALTER TABLE ONLY emitters.grown DROP COLUMN "........pg.dropped.2........";

-- For binary upgrade, recreate dropped column.
UPDATE pg_catalog.pg_attribute
SET attlen = -1, attalign = 'i', attbyval = false
WHERE attname = '........pg.dropped.4........'
  AND attrelid = 'emitters.grown'::pg_catalog.regclass;
ALTER TABLE ONLY emitters.grown DROP COLUMN "........pg.dropped.4........";

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '796', relminmxid = '1'
WHERE oid = 'emitters.grown'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '796', relminmxid = '1'
WHERE oid = '16516';


ALTER TABLE emitters.grown OWNER TO postgres;

--
-- Name: people; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16541'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16540'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16539'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16542'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16543'::pg_catalog.oid);

CREATE TABLE emitters.people (
    name text NOT NULL,
    born date,
    height integer
);

-- For binary upgrade, set up typed tables this way.
ALTER TABLE ONLY emitters.people OF emitters.person;

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '810', relminmxid = '1'
WHERE oid = 'emitters.people'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '810', relminmxid = '1'
WHERE oid = '16542';


ALTER TABLE emitters.people OWNER TO postgres;

--
-- Name: tuned; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16495'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16494'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16493'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16496'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16497'::pg_catalog.oid);

CREATE TABLE emitters.tuned (
    id integer NOT NULL,
    note text,
    amount numeric(8,2)
)
WITH (fillfactor='70', toast.autovacuum_enabled='false');

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '785', relminmxid = '1'
WHERE oid = 'emitters.tuned'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '785', relminmxid = '1'
WHERE oid = '16496';
ALTER TABLE ONLY emitters.tuned ALTER COLUMN note SET COMPRESSION pglz;
ALTER TABLE ONLY emitters.tuned ALTER COLUMN amount SET STATISTICS 500;

ALTER TABLE ONLY emitters.tuned REPLICA IDENTITY FULL;

ALTER TABLE ONLY emitters.tuned FORCE ROW LEVEL SECURITY;


ALTER TABLE emitters.tuned OWNER TO postgres;

--
-- Name: positive_tuned; Type: VIEW; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16507'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16506'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16505'::pg_catalog.oid);

CREATE VIEW emitters.positive_tuned WITH (security_barrier='true') AS
 SELECT tuned.id,
    tuned.note
   FROM emitters.tuned
  WHERE (tuned.id > 0)
  WITH LOCAL CHECK OPTION;


ALTER TABLE emitters.positive_tuned OWNER TO postgres;

--
-- Name: range_values; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16460'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16459'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16458'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16461'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16462'::pg_catalog.oid);

CREATE TABLE emitters.range_values (
    id integer NOT NULL,
    v_canon emitters.r_canon,
    v_diff emitters.r_diff,
    v_pattern emitters.r_pattern
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '774', relminmxid = '1'
WHERE oid = 'emitters.range_values'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '774', relminmxid = '1'
WHERE oid = '16461';


ALTER TABLE emitters.range_values OWNER TO postgres;

--
-- Name: scratch; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16523'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16522'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16521'::pg_catalog.oid);

CREATE UNLOGGED TABLE emitters.scratch (
    id integer,
    at date,
    ok boolean
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '802', relminmxid = '1'
WHERE oid = 'emitters.scratch'::pg_catalog.regclass;


ALTER TABLE emitters.scratch OWNER TO postgres;

--
-- Name: trios; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16488'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16487'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16486'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16489'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16490'::pg_catalog.oid);

CREATE TABLE emitters.trios (
    id integer NOT NULL,
    v emitters.trio
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '783', relminmxid = '1'
WHERE oid = 'emitters.trios'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '783', relminmxid = '1'
WHERE oid = '16489';


ALTER TABLE emitters.trios OWNER TO postgres;

--
-- Name: tuned_totals; Type: MATERIALIZED VIEW; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16511'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16510'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16509'::pg_catalog.oid);

CREATE MATERIALIZED VIEW emitters.tuned_totals AS
 SELECT count(*) AS n
   FROM emitters.tuned
  WITH NO DATA;

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '795', relminmxid = '1'
WHERE oid = 'emitters.tuned_totals'::pg_catalog.regclass;

-- For binary upgrade, mark materialized view as populated
UPDATE pg_catalog.pg_class
SET relispopulated = 't'
WHERE oid = 'emitters.tuned_totals'::pg_catalog.regclass;


ALTER TABLE emitters.tuned_totals OWNER TO postgres;

--
-- Name: unidentified; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16502'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16501'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16500'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16503'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16504'::pg_catalog.oid);

CREATE TABLE emitters.unidentified (
    id integer,
    label text
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '791', relminmxid = '1'
WHERE oid = 'emitters.unidentified'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '791', relminmxid = '1'
WHERE oid = '16503';

ALTER TABLE ONLY emitters.unidentified REPLICA IDENTITY NOTHING;


ALTER TABLE emitters.unidentified OWNER TO postgres;

--
-- Data for Name: base_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.base_values (id, v_varchar, v_char, v_int2, v_main, v_pair) FROM stdin;
1	alpha	a	7	main text	(1.5,2)
2	\N	\N	\N	\N	\N
\.


--
-- Data for Name: child; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.child (id, label, born, extra) FROM stdin;
2	c	2024-01-02	1.25
3	d	\N	\N
\.


--
-- Data for Name: domain_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.domain_values (id, v_positive, v_mood, v_pair) FROM stdin;
1	3	busy	(1,one)
2	\N	\N	\N
\.


--
-- Data for Name: grown; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.grown (id, b, added) FROM stdin;
1	b	2024-02-29
2	b2	2024-02-29
3	b3	2024-02-29
\.


--
-- Data for Name: parent; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.parent (id, label, born) FROM stdin;
1	p	2024-01-01
\.


--
-- Data for Name: people; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.people (name, born, height) FROM stdin;
ann	1990-05-01	170
bob	\N	\N
\.


--
-- Data for Name: range_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.range_values (id, v_canon, v_diff, v_pattern) FROM stdin;
1	[1,6)	[1.5,2.5)	[a,m)
2	empty	(,0]	[n,)
3	\N	\N	\N
\.


--
-- Data for Name: scratch; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.scratch (id, at, ok) FROM stdin;
1	2024-03-01	t
2	\N	f
\.


--
-- Data for Name: trios; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.trios (id, v) FROM stdin;
1	(1,2024-01-01)
2	\N
\.


--
-- Data for Name: tuned; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.tuned (id, note, amount) FROM stdin;
1	one	1.50
2	\N	\N
\.


--
-- Data for Name: unidentified; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.unidentified (id, label) FROM stdin;
1	x
\.


--
-- Name: base_values base_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16425'::pg_catalog.oid);

ALTER TABLE ONLY emitters.base_values
    ADD CONSTRAINT base_values_pkey PRIMARY KEY (id);


--
-- Name: domain_values domain_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16481'::pg_catalog.oid);

ALTER TABLE ONLY emitters.domain_values
    ADD CONSTRAINT domain_values_pkey PRIMARY KEY (id);


--
-- Name: grown grown_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16518'::pg_catalog.oid);

ALTER TABLE ONLY emitters.grown
    ADD CONSTRAINT grown_pkey PRIMARY KEY (id);


--
-- Name: range_values range_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16463'::pg_catalog.oid);

ALTER TABLE ONLY emitters.range_values
    ADD CONSTRAINT range_values_pkey PRIMARY KEY (id);


--
-- Name: trios trios_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16491'::pg_catalog.oid);

ALTER TABLE ONLY emitters.trios
    ADD CONSTRAINT trios_pkey PRIMARY KEY (id);


--
-- Name: tuned tuned_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16498'::pg_catalog.oid);

ALTER TABLE ONLY emitters.tuned
    ADD CONSTRAINT tuned_pkey PRIMARY KEY (id);


--
-- Name: tuned; Type: ROW SECURITY; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.tuned ENABLE ROW LEVEL SECURITY;

--
-- Name: tuned_totals; Type: MATERIALIZED VIEW DATA; Schema: emitters; Owner: postgres
--

REFRESH MATERIALIZED VIEW emitters.tuned_totals;


--
-- PostgreSQL database dump complete
--

\unrestrict sFVMIJpdIhKRqs5a2vQ8jasuYQcFimJ04s82EohdP17zy0fiXqB5xEXfQVHGEd2

