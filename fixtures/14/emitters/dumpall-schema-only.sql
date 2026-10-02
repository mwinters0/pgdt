--
-- PostgreSQL database cluster dump
--

\restrict pgdtfixture

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE "postgres";
ALTER ROLE "postgres" WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;




\unrestrict pgdtfixture

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

\restrict pgdtfixture

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

\unrestrict pgdtfixture

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict pgdtfixture

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
-- Name: pgdt-emitters; Type: DATABASE; Schema: -; Owner: -
--

CREATE DATABASE "pgdt-emitters" WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE = 'en_US.utf8';


\unrestrict pgdtfixture
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict pgdtfixture

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

SET default_table_access_method = "heap";

--
-- Name: named; Type: TABLE; Schema: public; Owner: -
--

CREATE TABLE "public"."named" (
    "id" integer NOT NULL,
    "label" "text",
    "born" "date"
);


--
-- Name: named named_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

ALTER TABLE ONLY "public"."named"
    ADD CONSTRAINT "named_pkey" PRIMARY KEY ("id");


--
-- PostgreSQL database dump complete
--

\unrestrict pgdtfixture

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict pgdtfixture

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
-- Name: pgdt_fixture; Type: DATABASE; Schema: -; Owner: -
--

CREATE DATABASE "pgdt_fixture" WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE = 'en_US.utf8';


\unrestrict pgdtfixture
\connect "pgdt_fixture"
\restrict pgdtfixture

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
-- Name: emitters; Type: SCHEMA; Schema: -; Owner: -
--

CREATE SCHEMA "emitters";


--
-- Name: file_fdw; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS "file_fdw" WITH SCHEMA "public";


--
-- Name: bt_char; Type: SHELL TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_char";


--
-- Name: bt_char_in("cstring"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_char_in"("cstring") RETURNS "emitters"."bt_char"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'charin';


--
-- Name: bt_char_out("emitters"."bt_char"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_char_out"("emitters"."bt_char") RETURNS "cstring"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'charout';


--
-- Name: bt_char; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_char" (
    INTERNALLENGTH = 1,
    INPUT = "emitters"."bt_char_in",
    OUTPUT = "emitters"."bt_char_out",
    ALIGNMENT = char,
    STORAGE = plain,
    PASSEDBYVALUE
);


--
-- Name: bt_int2; Type: SHELL TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_int2";


--
-- Name: bt_int2_in("cstring"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_int2_in"("cstring") RETURNS "emitters"."bt_int2"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'int2in';


--
-- Name: bt_int2_out("emitters"."bt_int2"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_int2_out"("emitters"."bt_int2") RETURNS "cstring"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'int2out';


--
-- Name: bt_int2; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_int2" (
    INTERNALLENGTH = 2,
    INPUT = "emitters"."bt_int2_in",
    OUTPUT = "emitters"."bt_int2_out",
    ALIGNMENT = int2,
    STORAGE = plain,
    PASSEDBYVALUE
);


--
-- Name: bt_pair; Type: SHELL TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_pair";


--
-- Name: bt_pair_in("cstring"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_pair_in"("cstring") RETURNS "emitters"."bt_pair"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'point_in';


--
-- Name: bt_pair_out("emitters"."bt_pair"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_pair_out"("emitters"."bt_pair") RETURNS "cstring"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'point_out';


--
-- Name: bt_pair; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_pair" (
    INTERNALLENGTH = 16,
    INPUT = "emitters"."bt_pair_in",
    OUTPUT = "emitters"."bt_pair_out",
    SUBSCRIPT = "raw_array_subscript_handler",
    ELEMENT = double precision,
    ALIGNMENT = double,
    STORAGE = plain
);


--
-- Name: bt_text_main; Type: SHELL TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_text_main";


--
-- Name: bt_text_main_in("cstring"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_text_main_in"("cstring") RETURNS "emitters"."bt_text_main"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'textin';


--
-- Name: bt_text_main_out("emitters"."bt_text_main"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_text_main_out"("emitters"."bt_text_main") RETURNS "cstring"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'textout';


--
-- Name: bt_text_main; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_text_main" (
    INTERNALLENGTH = variable,
    INPUT = "emitters"."bt_text_main_in",
    OUTPUT = "emitters"."bt_text_main_out",
    ALIGNMENT = int4,
    STORAGE = main
);


--
-- Name: bt_varchar; Type: SHELL TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_varchar";


--
-- Name: bt_varchar_analyze("internal"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_varchar_analyze"("internal") RETURNS boolean
    LANGUAGE "internal" STRICT
    AS 'ts_typanalyze';


--
-- Name: bt_varchar_in("cstring", "oid", integer); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_varchar_in"("cstring", "oid", integer) RETURNS "emitters"."bt_varchar"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'varcharin';


--
-- Name: bt_varchar_out("emitters"."bt_varchar"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_varchar_out"("emitters"."bt_varchar") RETURNS "cstring"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'varcharout';


--
-- Name: bt_varchar_recv("internal", "oid", integer); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_varchar_recv"("internal", "oid", integer) RETURNS "emitters"."bt_varchar"
    LANGUAGE "internal" STABLE STRICT
    AS 'varcharrecv';


--
-- Name: bt_varchar_send("emitters"."bt_varchar"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_varchar_send"("emitters"."bt_varchar") RETURNS "bytea"
    LANGUAGE "internal" STABLE STRICT
    AS 'varcharsend';


--
-- Name: bt_varchar_typmod_in("cstring"[]); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_varchar_typmod_in"("cstring"[]) RETURNS integer
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'varchartypmodin';


--
-- Name: bt_varchar_typmod_out(integer); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."bt_varchar_typmod_out"(integer) RETURNS "cstring"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'varchartypmodout';


--
-- Name: bt_varchar; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."bt_varchar" (
    INTERNALLENGTH = variable,
    INPUT = "emitters"."bt_varchar_in",
    OUTPUT = "emitters"."bt_varchar_out",
    RECEIVE = "emitters"."bt_varchar_recv",
    SEND = "emitters"."bt_varchar_send",
    TYPMOD_IN = "emitters"."bt_varchar_typmod_in",
    TYPMOD_OUT = "emitters"."bt_varchar_typmod_out",
    ANALYZE = "emitters"."bt_varchar_analyze",
    COLLATABLE = true,
    DEFAULT = 'none',
    CATEGORY = 'S',
    PREFERRED = true,
    DELIMITER = ';',
    ALIGNMENT = int4,
    STORAGE = external
);


--
-- Name: mood; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."mood" AS ENUM (
    'calm',
    'busy'
);


--
-- Name: pair; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."pair" AS (
	"left_part" integer,
	"right_part" "text"
);


--
-- Name: person; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."person" AS (
	"name" "text",
	"born" "date",
	"height" integer
);


--
-- Name: positive; Type: DOMAIN; Schema: emitters; Owner: -
--

CREATE DOMAIN "emitters"."positive" AS integer
	CONSTRAINT "positive_check" CHECK ((VALUE > 0));


--
-- Name: r_canon; Type: SHELL TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."r_canon";


--
-- Name: r_canon_canonical("emitters"."r_canon"); Type: FUNCTION; Schema: emitters; Owner: -
--

CREATE FUNCTION "emitters"."r_canon_canonical"("emitters"."r_canon") RETURNS "emitters"."r_canon"
    LANGUAGE "internal" IMMUTABLE STRICT
    AS 'int4range_canonical';


--
-- Name: r_canon; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."r_canon" AS RANGE (
    subtype = integer,
    multirange_type_name = "emitters"."r_canon_multirange",
    canonical = "emitters"."r_canon_canonical"
);


--
-- Name: r_diff; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."r_diff" AS RANGE (
    subtype = double precision,
    multirange_type_name = "emitters"."r_diff_multirange",
    subtype_diff = "float8mi"
);


--
-- Name: r_pattern; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."r_pattern" AS RANGE (
    subtype = "text",
    multirange_type_name = "emitters"."r_pattern_multirange",
    subtype_opclass = "pg_catalog"."text_pattern_ops"
);


--
-- Name: trio; Type: TYPE; Schema: emitters; Owner: -
--

CREATE TYPE "emitters"."trio" AS (
	"a" integer,
	"c" "date"
);


--
-- Name: emitters_files; Type: SERVER; Schema: -; Owner: -
--

CREATE SERVER "emitters_files" FOREIGN DATA WRAPPER "file_fdw";


SET default_table_access_method = "heap";

--
-- Name: base_values; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."base_values" (
    "id" integer NOT NULL,
    "v_varchar" "emitters"."bt_varchar"(8),
    "v_char" "emitters"."bt_char",
    "v_int2" "emitters"."bt_int2",
    "v_main" "emitters"."bt_text_main",
    "v_pair" "emitters"."bt_pair"
);


--
-- Name: parent; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."parent" (
    "id" integer NOT NULL,
    "label" "text",
    "born" "date",
    CONSTRAINT "parent_id_positive" CHECK (("id" > 0))
);


--
-- Name: child; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."child" (
    "extra" numeric(6,2)
)
INHERITS ("emitters"."parent");
ALTER TABLE ONLY "emitters"."child" ALTER COLUMN "label" SET NOT NULL;


--
-- Name: domain_values; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."domain_values" (
    "id" integer NOT NULL,
    "v_positive" "emitters"."positive",
    "v_mood" "emitters"."mood",
    "v_pair" "emitters"."pair"
);


--
-- Name: external; Type: FOREIGN TABLE; Schema: emitters; Owner: -
--

CREATE FOREIGN TABLE "emitters"."external" (
    "id" integer,
    "label" "text"
)
SERVER "emitters_files"
OPTIONS (
    "filename" '/tmp/emitters_external.tsv'
);
ALTER FOREIGN TABLE "emitters"."external" ALTER COLUMN "label" OPTIONS (
    "force_not_null" 'true'
);


--
-- Name: grown; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."grown" (
    "id" integer NOT NULL,
    "b" "text",
    "added" "date" DEFAULT '2024-02-29'::"date"
);


--
-- Name: people; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."people" OF "emitters"."person" (
    "name" NOT NULL
);


--
-- Name: tuned; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."tuned" (
    "id" integer NOT NULL,
    "note" "text",
    "amount" numeric(8,2)
)
WITH ("fillfactor"='70', toast."autovacuum_enabled"='false');
ALTER TABLE ONLY "emitters"."tuned" ALTER COLUMN "note" SET COMPRESSION pglz;
ALTER TABLE ONLY "emitters"."tuned" ALTER COLUMN "amount" SET STATISTICS 500;

ALTER TABLE ONLY "emitters"."tuned" REPLICA IDENTITY FULL;

ALTER TABLE ONLY "emitters"."tuned" FORCE ROW LEVEL SECURITY;


--
-- Name: positive_tuned; Type: VIEW; Schema: emitters; Owner: -
--

CREATE VIEW "emitters"."positive_tuned" WITH ("security_barrier"='true') AS
 SELECT "tuned"."id",
    "tuned"."note"
   FROM "emitters"."tuned"
  WHERE ("tuned"."id" > 0)
  WITH LOCAL CHECK OPTION;


--
-- Name: range_values; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."range_values" (
    "id" integer NOT NULL,
    "v_canon" "emitters"."r_canon",
    "v_diff" "emitters"."r_diff",
    "v_pattern" "emitters"."r_pattern"
);


--
-- Name: scratch; Type: TABLE; Schema: emitters; Owner: -
--

CREATE UNLOGGED TABLE "emitters"."scratch" (
    "id" integer,
    "at" "date",
    "ok" boolean
);


--
-- Name: stamped; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."stamped" (
    "id" integer NOT NULL,
    "stamps" timestamp with time zone[] DEFAULT ARRAY["now"(), "now"()],
    "now" integer
);


--
-- Name: trios; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."trios" (
    "id" integer NOT NULL,
    "v" "emitters"."trio"
);


--
-- Name: tuned_totals; Type: MATERIALIZED VIEW; Schema: emitters; Owner: -
--

CREATE MATERIALIZED VIEW "emitters"."tuned_totals" AS
 SELECT "count"(*) AS "n"
   FROM "emitters"."tuned"
  WITH NO DATA;


--
-- Name: unidentified; Type: TABLE; Schema: emitters; Owner: -
--

CREATE TABLE "emitters"."unidentified" (
    "id" integer,
    "label" "text"
);

ALTER TABLE ONLY "emitters"."unidentified" REPLICA IDENTITY NOTHING;


--
-- Name: base_values base_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: -
--

ALTER TABLE ONLY "emitters"."base_values"
    ADD CONSTRAINT "base_values_pkey" PRIMARY KEY ("id");


--
-- Name: domain_values domain_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: -
--

ALTER TABLE ONLY "emitters"."domain_values"
    ADD CONSTRAINT "domain_values_pkey" PRIMARY KEY ("id");


--
-- Name: grown grown_pkey; Type: CONSTRAINT; Schema: emitters; Owner: -
--

ALTER TABLE ONLY "emitters"."grown"
    ADD CONSTRAINT "grown_pkey" PRIMARY KEY ("id");


--
-- Name: range_values range_values_pkey; Type: CONSTRAINT; Schema: emitters; Owner: -
--

ALTER TABLE ONLY "emitters"."range_values"
    ADD CONSTRAINT "range_values_pkey" PRIMARY KEY ("id");


--
-- Name: stamped stamped_pkey; Type: CONSTRAINT; Schema: emitters; Owner: -
--

ALTER TABLE ONLY "emitters"."stamped"
    ADD CONSTRAINT "stamped_pkey" PRIMARY KEY ("id");


--
-- Name: trios trios_pkey; Type: CONSTRAINT; Schema: emitters; Owner: -
--

ALTER TABLE ONLY "emitters"."trios"
    ADD CONSTRAINT "trios_pkey" PRIMARY KEY ("id");


--
-- Name: tuned tuned_pkey; Type: CONSTRAINT; Schema: emitters; Owner: -
--

ALTER TABLE ONLY "emitters"."tuned"
    ADD CONSTRAINT "tuned_pkey" PRIMARY KEY ("id");


--
-- Name: tuned; Type: ROW SECURITY; Schema: emitters; Owner: -
--

ALTER TABLE "emitters"."tuned" ENABLE ROW LEVEL SECURITY;

--
-- PostgreSQL database dump complete
--

\unrestrict pgdtfixture

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict pgdtfixture

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

\unrestrict pgdtfixture

--
-- PostgreSQL database cluster dump complete
--

