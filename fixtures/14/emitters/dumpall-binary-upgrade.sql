--
-- PostgreSQL database cluster dump
--

\restrict 0fIlIWrOo36n1R37GnwR3k5yIv1TgPH5oxp3LuPradswYa3ESwcTsv1oudScWBp

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--


-- For binary upgrade, must preserve pg_authid.oid
SELECT pg_catalog.binary_upgrade_set_next_pg_authid_oid('16386'::pg_catalog.oid);

CREATE ROLE emitters_grantor;
ALTER ROLE emitters_grantor WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;

-- For binary upgrade, must preserve pg_authid.oid
SELECT pg_catalog.binary_upgrade_set_next_pg_authid_oid('16387'::pg_catalog.oid);

CREATE ROLE emitters_member;
ALTER ROLE emitters_member WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;

-- For binary upgrade, must preserve pg_authid.oid
SELECT pg_catalog.binary_upgrade_set_next_pg_authid_oid('16388'::pg_catalog.oid);

CREATE ROLE emitters_other;
ALTER ROLE emitters_other WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;

-- For binary upgrade, must preserve pg_authid.oid
SELECT pg_catalog.binary_upgrade_set_next_pg_authid_oid('10'::pg_catalog.oid);

ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;


--
-- Role memberships
--

GRANT emitters_grantor TO emitters_member WITH ADMIN OPTION GRANTED BY postgres;


--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict 0fIlIWrOo36n1R37GnwR3k5yIv1TgPH5oxp3LuPradswYa3ESwcTsv1oudScWBp

--
-- Databases
--

--
-- Database "template1" dump
--

\connect template1

--
-- PostgreSQL database dump
--

\restrict mNddTDZih3zWIBSJ7Is9LgIWW2sfVzgCyv4LcpBdPOFBkkUcNBcZ8b7FVP3tp32

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
-- PostgreSQL database dump complete
--

\unrestrict mNddTDZih3zWIBSJ7Is9LgIWW2sfVzgCyv4LcpBdPOFBkkUcNBcZ8b7FVP3tp32

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict Q17ReOk8BRE1eZaQRbLXbVdSD9N4GbVLXqgOktfKXgXDDts3xySbf7wEJuEaqmG

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
-- Name: pgdt-emitters; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE "pgdt-emitters" WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE = 'en_US.utf8';


ALTER DATABASE "pgdt-emitters" OWNER TO postgres;

\unrestrict Q17ReOk8BRE1eZaQRbLXbVdSD9N4GbVLXqgOktfKXgXDDts3xySbf7wEJuEaqmG
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict Q17ReOk8BRE1eZaQRbLXbVdSD9N4GbVLXqgOktfKXgXDDts3xySbf7wEJuEaqmG

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
-- Name: pgdt-emitters; Type: DATABASE PROPERTIES; Schema: -; Owner: postgres
--


-- For binary upgrade, set datfrozenxid and datminmxid.
UPDATE pg_catalog.pg_database
SET datfrozenxid = '727', datminmxid = '1'
WHERE datname = 'pgdt-emitters';


\unrestrict Q17ReOk8BRE1eZaQRbLXbVdSD9N4GbVLXqgOktfKXgXDDts3xySbf7wEJuEaqmG
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict Q17ReOk8BRE1eZaQRbLXbVdSD9N4GbVLXqgOktfKXgXDDts3xySbf7wEJuEaqmG

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
-- Name: pg_largeobject; Type: pg_largeobject; Schema: -; Owner: -
--


-- For binary upgrade, set pg_largeobject relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '727', relminmxid = '1'
WHERE oid = 2613;


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: named; Type: TABLE; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16392'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16391'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16390'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16393'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16394'::pg_catalog.oid);

CREATE TABLE public.named (
    id integer NOT NULL,
    label text,
    born date
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '743', relminmxid = '1'
WHERE oid = 'public.named'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '743', relminmxid = '1'
WHERE oid = '16393';


ALTER TABLE public.named OWNER TO postgres;

--
-- Data for Name: named; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.named (id, label, born) FROM stdin;
1	first	2024-04-01
2	\N	\N
\.


--
-- Name: named named_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16395'::pg_catalog.oid);

ALTER TABLE ONLY public.named
    ADD CONSTRAINT named_pkey PRIMARY KEY (id);


--
-- PostgreSQL database dump complete
--

\unrestrict Q17ReOk8BRE1eZaQRbLXbVdSD9N4GbVLXqgOktfKXgXDDts3xySbf7wEJuEaqmG

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict I9SNVmGxI4lbzgQ7bLr0eYK3nkM1x8FcxXFAKORTdXfCXboHrQU9gwZe1ZlhLJV

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
-- Name: pgdt_fixture; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE pgdt_fixture WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE = 'en_US.utf8';


ALTER DATABASE pgdt_fixture OWNER TO postgres;

\unrestrict I9SNVmGxI4lbzgQ7bLr0eYK3nkM1x8FcxXFAKORTdXfCXboHrQU9gwZe1ZlhLJV
\connect pgdt_fixture
\restrict I9SNVmGxI4lbzgQ7bLr0eYK3nkM1x8FcxXFAKORTdXfCXboHrQU9gwZe1ZlhLJV

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
-- Name: pgdt_fixture; Type: DATABASE PROPERTIES; Schema: -; Owner: postgres
--


-- For binary upgrade, set datfrozenxid and datminmxid.
UPDATE pg_catalog.pg_database
SET datfrozenxid = '727', datminmxid = '1'
WHERE datname = 'pgdt_fixture';


\unrestrict I9SNVmGxI4lbzgQ7bLr0eYK3nkM1x8FcxXFAKORTdXfCXboHrQU9gwZe1ZlhLJV
\connect pgdt_fixture
\restrict I9SNVmGxI4lbzgQ7bLr0eYK3nkM1x8FcxXFAKORTdXfCXboHrQU9gwZe1ZlhLJV

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
-- Name: pg_largeobject; Type: pg_largeobject; Schema: -; Owner: -
--


-- For binary upgrade, set pg_largeobject relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '727', relminmxid = '1'
WHERE oid = 2613;


--
-- Name: emitters; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA emitters;


ALTER SCHEMA emitters OWNER TO postgres;

--
-- Name: c_split; Type: COLLATION; Schema: emitters; Owner: postgres
--

CREATE COLLATION emitters.c_split (provider = libc, lc_collate = 'C', lc_ctype = 'POSIX');


ALTER COLLATION emitters.c_split OWNER TO postgres;

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16407'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16410'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16407'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16410'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16411'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16414'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16411'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16414'::pg_catalog.oid);

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
-- Name: bt_varchar; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16398'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16406'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16398'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16406'::pg_catalog.oid);

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
-- Name: bt_list; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16432'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16431'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16430'::pg_catalog.oid);

CREATE TYPE emitters.bt_list AS (
	items emitters.bt_varchar[]
);


ALTER TYPE emitters.bt_list OWNER TO postgres;

--
-- Name: bt_pair; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16419'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16422'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16419'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16422'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16415'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16418'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16415'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16418'::pg_catalog.oid);

CREATE TYPE emitters.bt_text_main (
    INTERNALLENGTH = variable,
    INPUT = emitters.bt_text_main_in,
    OUTPUT = emitters.bt_text_main_out,
    ALIGNMENT = int4,
    STORAGE = main
);


ALTER TYPE emitters.bt_text_main OWNER TO postgres;

--
-- Name: mood; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16482'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16481'::pg_catalog.oid);

CREATE TYPE emitters.mood AS ENUM (
);

-- For binary upgrade, must preserve pg_enum oids
SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16484'::pg_catalog.oid);
ALTER TYPE emitters.mood ADD VALUE 'calm';

SELECT pg_catalog.binary_upgrade_set_next_pg_enum_oid('16486'::pg_catalog.oid);
ALTER TYPE emitters.mood ADD VALUE 'busy';



ALTER TYPE emitters.mood OWNER TO postgres;

--
-- Name: pair; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16489'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16488'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16487'::pg_catalog.oid);

CREATE TYPE emitters.pair AS (
	left_part integer,
	right_part text
);


ALTER TYPE emitters.pair OWNER TO postgres;

--
-- Name: person; Type: TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16560'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16559'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16558'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16479'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16478'::pg_catalog.oid);

CREATE DOMAIN emitters.positive AS integer
	CONSTRAINT positive_check CHECK ((VALUE > 0));


ALTER DOMAIN emitters.positive OWNER TO postgres;

--
-- Name: r_canon; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16440'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16442'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16440'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16442'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16443'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16444'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16454'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16451'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16452'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16453'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16464'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16461'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_pg_type_oid('16462'::pg_catalog.oid);


-- For binary upgrade, must preserve multirange pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_multirange_array_pg_type_oid('16463'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16499'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16498'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16497'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16425'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16424'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16423'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16426'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16427'::pg_catalog.oid);

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
SET relfrozenxid = '771', relminmxid = '1'
WHERE oid = 'emitters.base_values'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '771', relminmxid = '1'
WHERE oid = '16426';


ALTER TABLE emitters.base_values OWNER TO postgres;

--
-- Name: parent; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16540'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16539'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16538'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16542'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16543'::pg_catalog.oid);

CREATE TABLE emitters.parent (
    id integer NOT NULL,
    label text,
    born date,
    CONSTRAINT parent_id_positive CHECK ((id > 0))
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '811', relminmxid = '1'
WHERE oid = 'emitters.parent'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '811', relminmxid = '1'
WHERE oid = '16542';


ALTER TABLE emitters.parent OWNER TO postgres;

--
-- Name: child; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16546'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16545'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16544'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16548'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16549'::pg_catalog.oid);

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
SET relfrozenxid = '812', relminmxid = '1'
WHERE oid = 'emitters.child'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '812', relminmxid = '1'
WHERE oid = '16548';


ALTER TABLE emitters.child OWNER TO postgres;

--
-- Name: collated; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16587'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16586'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16585'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16588'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16589'::pg_catalog.oid);

CREATE TABLE emitters.collated (
    id integer,
    label text COLLATE emitters.c_split
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '831', relminmxid = '1'
WHERE oid = 'emitters.collated'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '831', relminmxid = '1'
WHERE oid = '16588';


ALTER TABLE emitters.collated OWNER TO postgres;

--
-- Name: delimited; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16435'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16434'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16433'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16436'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16437'::pg_catalog.oid);

CREATE TABLE emitters.delimited (
    id integer NOT NULL,
    v emitters.bt_list,
    a emitters.bt_varchar[]
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '774', relminmxid = '1'
WHERE oid = 'emitters.delimited'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '774', relminmxid = '1'
WHERE oid = '16436';


ALTER TABLE emitters.delimited OWNER TO postgres;

--
-- Name: domain_values; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16492'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16491'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16490'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16493'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16494'::pg_catalog.oid);

CREATE TABLE emitters.domain_values (
    id integer NOT NULL,
    v_positive emitters.positive,
    v_mood emitters.mood,
    v_pair emitters.pair
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '786', relminmxid = '1'
WHERE oid = 'emitters.domain_values'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '786', relminmxid = '1'
WHERE oid = '16493';


ALTER TABLE emitters.domain_values OWNER TO postgres;

--
-- Name: external; Type: FOREIGN TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16573'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16572'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16571'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16529'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16528'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16527'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16530'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16531'::pg_catalog.oid);

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
SET relfrozenxid = '803', relminmxid = '1'
WHERE oid = 'emitters.grown'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '803', relminmxid = '1'
WHERE oid = '16530';


ALTER TABLE emitters.grown OWNER TO postgres;

--
-- Name: identified; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16596'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16595'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16594'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16597'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16598'::pg_catalog.oid);

CREATE TABLE emitters.identified (
    id integer NOT NULL,
    label text
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '836', relminmxid = '1'
WHERE oid = 'emitters.identified'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '836', relminmxid = '1'
WHERE oid = '16597';


ALTER TABLE emitters.identified OWNER TO postgres;

--
-- Name: identified_id_seq; Type: SEQUENCE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16593'::pg_catalog.oid);

ALTER TABLE emitters.identified ALTER COLUMN id ADD GENERATED ALWAYS AS IDENTITY (
    SEQUENCE NAME emitters.identified_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1
);


--
-- Name: keyed; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16576'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16575'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16574'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16577'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16578'::pg_catalog.oid);

CREATE TABLE emitters.keyed (
    id integer NOT NULL,
    code text,
    note text
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '826', relminmxid = '1'
WHERE oid = 'emitters.keyed'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '826', relminmxid = '1'
WHERE oid = '16577';


ALTER TABLE emitters.keyed OWNER TO postgres;

--
-- Name: TABLE keyed; Type: COMMENT; Schema: emitters; Owner: postgres
--

COMMENT ON TABLE emitters.keyed IS 'keyed by C:\path';


--
-- Name: no_columns; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16592'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16591'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16590'::pg_catalog.oid);

CREATE TABLE emitters.no_columns (
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '833', relminmxid = '1'
WHERE oid = 'emitters.no_columns'::pg_catalog.regclass;


ALTER TABLE emitters.no_columns OWNER TO postgres;

--
-- Name: people; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16563'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16562'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16561'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16564'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16565'::pg_catalog.oid);

CREATE TABLE emitters.people (
    name text NOT NULL,
    born date,
    height integer
);

-- For binary upgrade, set up typed tables this way.
ALTER TABLE ONLY emitters.people OF emitters.person;

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '819', relminmxid = '1'
WHERE oid = 'emitters.people'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '819', relminmxid = '1'
WHERE oid = '16564';


ALTER TABLE emitters.people OWNER TO postgres;

--
-- Name: tuned; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16509'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16508'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16507'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16510'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16511'::pg_catalog.oid);

CREATE TABLE emitters.tuned (
    id integer NOT NULL,
    note text,
    amount numeric(8,2)
)
WITH (fillfactor='70', toast.autovacuum_enabled='false');

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '792', relminmxid = '1'
WHERE oid = 'emitters.tuned'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '792', relminmxid = '1'
WHERE oid = '16510';
ALTER TABLE ONLY emitters.tuned ALTER COLUMN note SET COMPRESSION pglz;
ALTER TABLE ONLY emitters.tuned ALTER COLUMN amount SET STATISTICS 500;

ALTER TABLE ONLY emitters.tuned REPLICA IDENTITY FULL;

ALTER TABLE ONLY emitters.tuned FORCE ROW LEVEL SECURITY;


ALTER TABLE emitters.tuned OWNER TO postgres;

--
-- Name: positive_tuned; Type: VIEW; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16521'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16520'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16519'::pg_catalog.oid);

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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16473'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16472'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16471'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16474'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16475'::pg_catalog.oid);

CREATE TABLE emitters.range_values (
    id integer NOT NULL,
    v_canon emitters.r_canon,
    v_diff emitters.r_diff,
    v_pattern emitters.r_pattern
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '781', relminmxid = '1'
WHERE oid = 'emitters.range_values'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '781', relminmxid = '1'
WHERE oid = '16474';


ALTER TABLE emitters.range_values OWNER TO postgres;

--
-- Name: scratch; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16537'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16536'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16535'::pg_catalog.oid);

CREATE UNLOGGED TABLE emitters.scratch (
    id integer,
    at date,
    ok boolean
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '809', relminmxid = '1'
WHERE oid = 'emitters.scratch'::pg_catalog.regclass;


ALTER TABLE emitters.scratch OWNER TO postgres;

--
-- Name: stamped; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16552'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16551'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16550'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16554'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16555'::pg_catalog.oid);

CREATE TABLE emitters.stamped (
    id integer NOT NULL,
    stamps timestamp with time zone[] DEFAULT ARRAY[now(), now()],
    now integer
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '816', relminmxid = '1'
WHERE oid = 'emitters.stamped'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '816', relminmxid = '1'
WHERE oid = '16554';


ALTER TABLE emitters.stamped OWNER TO postgres;

--
-- Name: trios; Type: TABLE; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16502'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16501'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16500'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16503'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16504'::pg_catalog.oid);

CREATE TABLE emitters.trios (
    id integer NOT NULL,
    v emitters.trio
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '790', relminmxid = '1'
WHERE oid = 'emitters.trios'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '790', relminmxid = '1'
WHERE oid = '16503';


ALTER TABLE emitters.trios OWNER TO postgres;

--
-- Name: tuned_totals; Type: MATERIALIZED VIEW; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_type oid
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16525'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16524'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16523'::pg_catalog.oid);

CREATE MATERIALIZED VIEW emitters.tuned_totals AS
 SELECT count(*) AS n
   FROM emitters.tuned
  WITH NO DATA;

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '802', relminmxid = '1'
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
SELECT pg_catalog.binary_upgrade_set_next_pg_type_oid('16516'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_type array oid
SELECT pg_catalog.binary_upgrade_set_next_array_pg_type_oid('16515'::pg_catalog.oid);


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_heap_pg_class_oid('16514'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_toast_pg_class_oid('16517'::pg_catalog.oid);
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16518'::pg_catalog.oid);

CREATE TABLE emitters.unidentified (
    id integer,
    label text
);

-- For binary upgrade, set heap's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '798', relminmxid = '1'
WHERE oid = 'emitters.unidentified'::pg_catalog.regclass;

-- For binary upgrade, set toast's relfrozenxid and relminmxid
UPDATE pg_catalog.pg_class
SET relfrozenxid = '798', relminmxid = '1'
WHERE oid = '16517';

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
-- Data for Name: collated; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.collated (id, label) FROM stdin;
1	b
2	a
\.


--
-- Data for Name: delimited; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.delimited (id, v, a) FROM stdin;
1	("{""a b"";c,d;""e;f""}")	{"a b";c}
2	("{{x;""y z""};{"""";NULL}}")	\N
3	\N	\N
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
-- Data for Name: identified; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.identified (id, label) FROM stdin;
1	one
2	\N
\.


--
-- Data for Name: keyed; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.keyed (id, code, note) FROM stdin;
1	a	first
2	\N	\N
\.


--
-- Data for Name: no_columns; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.no_columns  FROM stdin;


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
-- Data for Name: stamped; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.stamped (id, stamps, now) FROM stdin;
1	{"2024-01-01 00:00:00+00","2024-01-02 12:30:00+00"}	7
2	\N	\N
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
-- Name: identified_id_seq; Type: SEQUENCE SET; Schema: emitters; Owner: postgres
--

SELECT pg_catalog.setval('emitters.identified_id_seq', 2, true);


--
-- Name: base_values base_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16428'::pg_catalog.oid);

ALTER TABLE ONLY emitters.base_values
    ADD CONSTRAINT base_values_pkey PRIMARY KEY (id);


--
-- Name: delimited delimited_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16438'::pg_catalog.oid);

ALTER TABLE ONLY emitters.delimited
    ADD CONSTRAINT delimited_pkey PRIMARY KEY (id);


--
-- Name: domain_values domain_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16495'::pg_catalog.oid);

ALTER TABLE ONLY emitters.domain_values
    ADD CONSTRAINT domain_values_pkey PRIMARY KEY (id);


--
-- Name: grown grown_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16532'::pg_catalog.oid);

ALTER TABLE ONLY emitters.grown
    ADD CONSTRAINT grown_pkey PRIMARY KEY (id);


--
-- Name: keyed keyed_code_key; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16581'::pg_catalog.oid);

ALTER TABLE ONLY emitters.keyed
    ADD CONSTRAINT keyed_code_key UNIQUE (code) INCLUDE (note) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: keyed keyed_id_key; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16579'::pg_catalog.oid);

ALTER TABLE ONLY emitters.keyed
    ADD CONSTRAINT keyed_id_key UNIQUE (id);

ALTER TABLE ONLY emitters.keyed REPLICA IDENTITY USING INDEX keyed_id_key;


--
-- Name: range_values range_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16476'::pg_catalog.oid);

ALTER TABLE ONLY emitters.range_values
    ADD CONSTRAINT range_values_pkey PRIMARY KEY (id);


--
-- Name: stamped stamped_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16556'::pg_catalog.oid);

ALTER TABLE ONLY emitters.stamped
    ADD CONSTRAINT stamped_pkey PRIMARY KEY (id);


--
-- Name: trios trios_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16505'::pg_catalog.oid);

ALTER TABLE ONLY emitters.trios
    ADD CONSTRAINT trios_pkey PRIMARY KEY (id);


--
-- Name: tuned tuned_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--


-- For binary upgrade, must preserve pg_class oids
SELECT pg_catalog.binary_upgrade_set_next_index_pg_class_oid('16512'::pg_catalog.oid);

ALTER TABLE ONLY emitters.tuned
    ADD CONSTRAINT tuned_pkey PRIMARY KEY (id);


--
-- Name: tuned; Type: ROW SECURITY; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.tuned ENABLE ROW LEVEL SECURITY;

--
-- Name: SCHEMA emitters; Type: ACL; Schema: -; Owner: postgres
--

GRANT USAGE ON SCHEMA emitters TO emitters_grantor;


--
-- Name: TABLE keyed; Type: ACL; Schema: emitters; Owner: postgres
--

GRANT SELECT ON TABLE emitters.keyed TO emitters_grantor WITH GRANT OPTION;
SET SESSION AUTHORIZATION emitters_grantor;
GRANT SELECT ON TABLE emitters.keyed TO emitters_member;
RESET SESSION AUTHORIZATION;


--
-- Name: tuned_totals; Type: MATERIALIZED VIEW DATA; Schema: emitters; Owner: postgres
--

REFRESH MATERIALIZED VIEW emitters.tuned_totals;


--
-- PostgreSQL database dump complete
--

\unrestrict I9SNVmGxI4lbzgQ7bLr0eYK3nkM1x8FcxXFAKORTdXfCXboHrQU9gwZe1ZlhLJV

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict QWfBhRGfX7aJcJ0m8aZXeanxkjzL4Vh9KRwBXLmi7kVyqDeq5gw00NW5WwUsJoh

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
-- PostgreSQL database dump complete
--

\unrestrict QWfBhRGfX7aJcJ0m8aZXeanxkjzL4Vh9KRwBXLmi7kVyqDeq5gw00NW5WwUsJoh

--
-- PostgreSQL database cluster dump complete
--

