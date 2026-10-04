--
-- PostgreSQL database cluster dump
--

\restrict wxS5ilNXJecpKeyywub768U6Cudr7Yi6bfRRlLNioSlP8C4yisd33KxitfRcsEr

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE emitters_grantor;
ALTER ROLE emitters_grantor WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;
CREATE ROLE emitters_member;
ALTER ROLE emitters_member WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;
CREATE ROLE emitters_other;
ALTER ROLE emitters_other WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;
CREATE ROLE postgres;
ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;

--
-- User Configurations
--


--
-- Role memberships
--

GRANT emitters_grantor TO emitters_member WITH ADMIN OPTION, INHERIT TRUE GRANTED BY postgres;
GRANT emitters_grantor TO emitters_other WITH INHERIT FALSE, SET FALSE GRANTED BY postgres;




--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict wxS5ilNXJecpKeyywub768U6Cudr7Yi6bfRRlLNioSlP8C4yisd33KxitfRcsEr

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

\restrict ZlgeAX8HDpojKv7bbYp2nqok9ysRkqWgWrvazcOvf40bkcWbqDJWwJj5rvxnLva

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
-- PostgreSQL database dump complete
--

\unrestrict ZlgeAX8HDpojKv7bbYp2nqok9ysRkqWgWrvazcOvf40bkcWbqDJWwJj5rvxnLva

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict bTN3HDi6HaFmBXdoHCdVivo9Yjbu9ii4tazXG4fHMBIH8dKY5krdXgEIOwlg749

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
-- Name: pgdt-emitters; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE "pgdt-emitters" WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE_PROVIDER = libc LOCALE = 'en_US.utf8';


ALTER DATABASE "pgdt-emitters" OWNER TO postgres;

\unrestrict bTN3HDi6HaFmBXdoHCdVivo9Yjbu9ii4tazXG4fHMBIH8dKY5krdXgEIOwlg749
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict bTN3HDi6HaFmBXdoHCdVivo9Yjbu9ii4tazXG4fHMBIH8dKY5krdXgEIOwlg749

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

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: named; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.named (
    id integer NOT NULL,
    label text,
    born date
);


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

ALTER TABLE ONLY public.named
    ADD CONSTRAINT named_pkey PRIMARY KEY (id);


--
-- PostgreSQL database dump complete
--

\unrestrict bTN3HDi6HaFmBXdoHCdVivo9Yjbu9ii4tazXG4fHMBIH8dKY5krdXgEIOwlg749

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict m4yQEtlhaM7pHhICL3DrnjRuBWZTzAmHBQvEarFwAJASWF1oLZD2tofYmBZrtl4

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
-- Name: pgdt_fixture; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE pgdt_fixture WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE_PROVIDER = libc LOCALE = 'en_US.utf8';


ALTER DATABASE pgdt_fixture OWNER TO postgres;

\unrestrict m4yQEtlhaM7pHhICL3DrnjRuBWZTzAmHBQvEarFwAJASWF1oLZD2tofYmBZrtl4
\connect pgdt_fixture
\restrict m4yQEtlhaM7pHhICL3DrnjRuBWZTzAmHBQvEarFwAJASWF1oLZD2tofYmBZrtl4

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
-- Name: emitters; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA emitters;


ALTER SCHEMA emitters OWNER TO postgres;

--
-- Name: c_builtin; Type: COLLATION; Schema: emitters; Owner: postgres
--

CREATE COLLATION emitters.c_builtin (provider = builtin, locale = 'C');


ALTER COLLATION emitters.c_builtin OWNER TO postgres;

--
-- Name: c_rules; Type: COLLATION; Schema: emitters; Owner: postgres
--

CREATE COLLATION emitters.c_rules (provider = icu, locale = 'und', rules = '&a < b');


ALTER COLLATION emitters.c_rules OWNER TO postgres;

--
-- Name: c_split; Type: COLLATION; Schema: emitters; Owner: postgres
--

CREATE COLLATION emitters.c_split (provider = libc, lc_collate = 'C', lc_ctype = 'POSIX');


ALTER COLLATION emitters.c_split OWNER TO postgres;

--
-- Name: file_fdw; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS file_fdw WITH SCHEMA public;


--
-- Name: EXTENSION file_fdw; Type: COMMENT; Schema: -; Owner: 
--

COMMENT ON EXTENSION file_fdw IS 'foreign-data wrapper for flat file access';


--
-- Name: bt_char; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--

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

CREATE TYPE emitters.bt_list AS (
	items emitters.bt_varchar[]
);


ALTER TYPE emitters.bt_list OWNER TO postgres;

--
-- Name: bt_pair; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--

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

CREATE TYPE emitters.mood AS ENUM (
    'calm',
    'busy'
);


ALTER TYPE emitters.mood OWNER TO postgres;

--
-- Name: pair; Type: TYPE; Schema: emitters; Owner: postgres
--

CREATE TYPE emitters.pair AS (
	left_part integer,
	right_part text
);


ALTER TYPE emitters.pair OWNER TO postgres;

--
-- Name: person; Type: TYPE; Schema: emitters; Owner: postgres
--

CREATE TYPE emitters.person AS (
	name text,
	born date,
	height integer
);


ALTER TYPE emitters.person OWNER TO postgres;

--
-- Name: positive; Type: DOMAIN; Schema: emitters; Owner: postgres
--

CREATE DOMAIN emitters.positive AS integer
	CONSTRAINT positive_check CHECK ((VALUE > 0));


ALTER DOMAIN emitters.positive OWNER TO postgres;

--
-- Name: r_canon; Type: SHELL TYPE; Schema: emitters; Owner: postgres
--

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

CREATE TYPE emitters.r_canon AS RANGE (
    subtype = integer,
    multirange_type_name = emitters.r_canon_multirange,
    canonical = emitters.r_canon_canonical
);


ALTER TYPE emitters.r_canon OWNER TO postgres;

--
-- Name: r_diff; Type: TYPE; Schema: emitters; Owner: postgres
--

CREATE TYPE emitters.r_diff AS RANGE (
    subtype = double precision,
    multirange_type_name = emitters.r_diff_multirange,
    subtype_diff = float8mi
);


ALTER TYPE emitters.r_diff OWNER TO postgres;

--
-- Name: r_pattern; Type: TYPE; Schema: emitters; Owner: postgres
--

CREATE TYPE emitters.r_pattern AS RANGE (
    subtype = text,
    multirange_type_name = emitters.r_pattern_multirange,
    subtype_opclass = pg_catalog.text_pattern_ops
);


ALTER TYPE emitters.r_pattern OWNER TO postgres;

--
-- Name: trio; Type: TYPE; Schema: emitters; Owner: postgres
--

CREATE TYPE emitters.trio AS (
	a integer,
	c date
);


ALTER TYPE emitters.trio OWNER TO postgres;

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

CREATE TABLE emitters.base_values (
    id integer NOT NULL,
    v_varchar emitters.bt_varchar(8),
    v_char emitters.bt_char,
    v_int2 emitters.bt_int2,
    v_main emitters.bt_text_main,
    v_pair emitters.bt_pair
);


ALTER TABLE emitters.base_values OWNER TO postgres;

--
-- Name: built; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.built (
    id integer,
    label text COLLATE emitters.c_builtin
);


ALTER TABLE emitters.built OWNER TO postgres;

--
-- Name: parent; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.parent (
    id integer NOT NULL,
    label text,
    born date,
    CONSTRAINT parent_id_positive CHECK ((id > 0))
);


ALTER TABLE emitters.parent OWNER TO postgres;

--
-- Name: child; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.child (
    extra numeric(6,2)
)
INHERITS (emitters.parent);
ALTER TABLE ONLY emitters.child ALTER COLUMN label SET NOT NULL;


ALTER TABLE emitters.child OWNER TO postgres;

--
-- Name: collated; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.collated (
    id integer,
    label text COLLATE emitters.c_split
);


ALTER TABLE emitters.collated OWNER TO postgres;

--
-- Name: delimited; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.delimited (
    id integer NOT NULL,
    v emitters.bt_list,
    a emitters.bt_varchar[]
);


ALTER TABLE emitters.delimited OWNER TO postgres;

--
-- Name: domain_values; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.domain_values (
    id integer NOT NULL,
    v_positive emitters.positive,
    v_mood emitters.mood,
    v_pair emitters.pair
);


ALTER TABLE emitters.domain_values OWNER TO postgres;

--
-- Name: external; Type: FOREIGN TABLE; Schema: emitters; Owner: postgres
--

CREATE FOREIGN TABLE emitters.external (
    id integer,
    label text
)
SERVER emitters_files
OPTIONS (
    filename '/tmp/emitters_external.tsv'
);
ALTER FOREIGN TABLE ONLY emitters.external ALTER COLUMN label OPTIONS (
    force_not_null 'true'
);


ALTER FOREIGN TABLE emitters.external OWNER TO postgres;

--
-- Name: grown; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.grown (
    id integer NOT NULL,
    b text,
    added date DEFAULT '2024-02-29'::date
);


ALTER TABLE emitters.grown OWNER TO postgres;

--
-- Name: identified; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.identified (
    id integer NOT NULL,
    label text
);


ALTER TABLE emitters.identified OWNER TO postgres;

--
-- Name: identified_id_seq; Type: SEQUENCE; Schema: emitters; Owner: postgres
--

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

CREATE TABLE emitters.keyed (
    id integer NOT NULL,
    code text,
    note text
);


ALTER TABLE emitters.keyed OWNER TO postgres;

--
-- Name: TABLE keyed; Type: COMMENT; Schema: emitters; Owner: postgres
--

COMMENT ON TABLE emitters.keyed IS 'keyed by C:\path';


--
-- Name: no_columns; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.no_columns (
);


ALTER TABLE emitters.no_columns OWNER TO postgres;

--
-- Name: people; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.people OF emitters.person (
    name NOT NULL
);


ALTER TABLE emitters.people OWNER TO postgres;

--
-- Name: tuned; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.tuned (
    id integer NOT NULL,
    note text,
    amount numeric(8,2)
)
WITH (fillfactor='70', toast.autovacuum_enabled='false');
ALTER TABLE ONLY emitters.tuned ALTER COLUMN note SET COMPRESSION pglz;
ALTER TABLE ONLY emitters.tuned ALTER COLUMN amount SET STATISTICS 500;

ALTER TABLE ONLY emitters.tuned REPLICA IDENTITY FULL;

ALTER TABLE ONLY emitters.tuned FORCE ROW LEVEL SECURITY;


ALTER TABLE emitters.tuned OWNER TO postgres;

--
-- Name: positive_tuned; Type: VIEW; Schema: emitters; Owner: postgres
--

CREATE VIEW emitters.positive_tuned WITH (security_barrier='true') AS
 SELECT id,
    note
   FROM emitters.tuned
  WHERE (id > 0)
  WITH LOCAL CHECK OPTION;


ALTER VIEW emitters.positive_tuned OWNER TO postgres;

--
-- Name: range_values; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.range_values (
    id integer NOT NULL,
    v_canon emitters.r_canon,
    v_diff emitters.r_diff,
    v_pattern emitters.r_pattern
);


ALTER TABLE emitters.range_values OWNER TO postgres;

--
-- Name: ruled; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.ruled (
    id integer,
    label text COLLATE emitters.c_rules
);


ALTER TABLE emitters.ruled OWNER TO postgres;

--
-- Name: scratch; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE UNLOGGED TABLE emitters.scratch (
    id integer,
    at date,
    ok boolean
);


ALTER TABLE emitters.scratch OWNER TO postgres;

--
-- Name: stamped; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.stamped (
    id integer NOT NULL,
    stamps timestamp with time zone[] DEFAULT ARRAY[now(), now()],
    now integer
);


ALTER TABLE emitters.stamped OWNER TO postgres;

--
-- Name: trios; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.trios (
    id integer NOT NULL,
    v emitters.trio
);


ALTER TABLE emitters.trios OWNER TO postgres;

--
-- Name: tuned_totals; Type: MATERIALIZED VIEW; Schema: emitters; Owner: postgres
--

CREATE MATERIALIZED VIEW emitters.tuned_totals AS
 SELECT count(*) AS n
   FROM emitters.tuned
  WITH NO DATA;


ALTER MATERIALIZED VIEW emitters.tuned_totals OWNER TO postgres;

--
-- Name: unidentified; Type: TABLE; Schema: emitters; Owner: postgres
--

CREATE TABLE emitters.unidentified (
    id integer,
    label text
);

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
-- Data for Name: built; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.built (id, label) FROM stdin;
1	b
2	a
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
-- Data for Name: ruled; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.ruled (id, label) FROM stdin;
1	b
2	a
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

ALTER TABLE ONLY emitters.base_values
    ADD CONSTRAINT base_values_pkey PRIMARY KEY (id);


--
-- Name: delimited delimited_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.delimited
    ADD CONSTRAINT delimited_pkey PRIMARY KEY (id);


--
-- Name: domain_values domain_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.domain_values
    ADD CONSTRAINT domain_values_pkey PRIMARY KEY (id);


--
-- Name: grown grown_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.grown
    ADD CONSTRAINT grown_pkey PRIMARY KEY (id);


--
-- Name: keyed keyed_code_key; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.keyed
    ADD CONSTRAINT keyed_code_key UNIQUE (code) INCLUDE (note) DEFERRABLE INITIALLY DEFERRED;


--
-- Name: keyed keyed_id_key; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.keyed
    ADD CONSTRAINT keyed_id_key UNIQUE (id);

ALTER TABLE ONLY emitters.keyed REPLICA IDENTITY USING INDEX keyed_id_key;


--
-- Name: keyed keyed_note_key; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.keyed
    ADD CONSTRAINT keyed_note_key UNIQUE NULLS NOT DISTINCT (note);


--
-- Name: range_values range_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.range_values
    ADD CONSTRAINT range_values_pkey PRIMARY KEY (id);


--
-- Name: stamped stamped_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.stamped
    ADD CONSTRAINT stamped_pkey PRIMARY KEY (id);


--
-- Name: trios trios_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

ALTER TABLE ONLY emitters.trios
    ADD CONSTRAINT trios_pkey PRIMARY KEY (id);


--
-- Name: tuned tuned_pkey; Type: CONSTRAINT; Schema: emitters; Owner: postgres
--

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

\unrestrict m4yQEtlhaM7pHhICL3DrnjRuBWZTzAmHBQvEarFwAJASWF1oLZD2tofYmBZrtl4

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict Xn0vFG91rDzYULnUzxhvO1y7bgSPC9EpoFxAkOXorDoDjhyaiLf01aQbBX5J7rX

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
-- PostgreSQL database dump complete
--

\unrestrict Xn0vFG91rDzYULnUzxhvO1y7bgSPC9EpoFxAkOXorDoDjhyaiLf01aQbBX5J7rX

--
-- PostgreSQL database cluster dump complete
--

