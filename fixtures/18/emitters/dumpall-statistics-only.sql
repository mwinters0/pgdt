--
-- PostgreSQL database cluster dump
--

\restrict EBfWYKkLrX60kFKdda3GjTANE65KXIh5fQL4O0mVEn7bpYq2tZvsvYqK0VVaHbM

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict EBfWYKkLrX60kFKdda3GjTANE65KXIh5fQL4O0mVEn7bpYq2tZvsvYqK0VVaHbM

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

\restrict x1jRwKk3YKE9IrRHO6bYZf2gcWm7T30EgTQlAkwcQfua266f7oll7o8E3Q4a6DE

-- Dumped from database version 18.6 (Debian 18.6-1.pgdg13+2)
-- Dumped by pg_dump version 18.6 (Debian 18.6-1.pgdg13+2)

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

\unrestrict x1jRwKk3YKE9IrRHO6bYZf2gcWm7T30EgTQlAkwcQfua266f7oll7o8E3Q4a6DE

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict if6N1jl9waWiWVYHL6ug66eSrG0uZhwIo51iZWRmpxQB2f07qeZiWtDIynTZaYE

-- Dumped from database version 18.6 (Debian 18.6-1.pgdg13+2)
-- Dumped by pg_dump version 18.6 (Debian 18.6-1.pgdg13+2)

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

\unrestrict if6N1jl9waWiWVYHL6ug66eSrG0uZhwIo51iZWRmpxQB2f07qeZiWtDIynTZaYE
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict if6N1jl9waWiWVYHL6ug66eSrG0uZhwIo51iZWRmpxQB2f07qeZiWtDIynTZaYE

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
-- Statistics for Name: named; Type: STATISTICS DATA; Schema: public; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'public',
	'relname', 'named',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: named_pkey; Type: STATISTICS DATA; Schema: public; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'public',
	'relname', 'named_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- PostgreSQL database dump complete
--

\unrestrict if6N1jl9waWiWVYHL6ug66eSrG0uZhwIo51iZWRmpxQB2f07qeZiWtDIynTZaYE

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict a9gA7lb6vVzzKF8MbwPHZ5WJRgfURS8FaFgqEqWE8JfPZ8DdqahIsdod6UTJUqN

-- Dumped from database version 18.6 (Debian 18.6-1.pgdg13+2)
-- Dumped by pg_dump version 18.6 (Debian 18.6-1.pgdg13+2)

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

\unrestrict a9gA7lb6vVzzKF8MbwPHZ5WJRgfURS8FaFgqEqWE8JfPZ8DdqahIsdod6UTJUqN
\connect pgdt_fixture
\restrict a9gA7lb6vVzzKF8MbwPHZ5WJRgfURS8FaFgqEqWE8JfPZ8DdqahIsdod6UTJUqN

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
-- Statistics for Name: base_values; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'base_values',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: child; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'child',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: domain_values; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'domain_values',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: external; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'external',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: grown; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'grown',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: parent; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'parent',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: people; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'people',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: range_values; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'range_values',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: scratch; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'scratch',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: stamped; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'stamped',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: trios; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'trios',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: tuned; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'tuned',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: tuned_totals; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'tuned_totals',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: unidentified; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'unidentified',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: base_values_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'base_values_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: domain_values_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'domain_values_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: grown_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'grown_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: range_values_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'range_values_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: stamped_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'stamped_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: trios_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'trios_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: tuned_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'tuned_pkey',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- PostgreSQL database dump complete
--

\unrestrict a9gA7lb6vVzzKF8MbwPHZ5WJRgfURS8FaFgqEqWE8JfPZ8DdqahIsdod6UTJUqN

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict MYej2toiE2lnAW50cHczu9dStnsuevOfnCrpy4FuX2nxPDW5uLdedujQfGvspe4

-- Dumped from database version 18.6 (Debian 18.6-1.pgdg13+2)
-- Dumped by pg_dump version 18.6 (Debian 18.6-1.pgdg13+2)

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

\unrestrict MYej2toiE2lnAW50cHczu9dStnsuevOfnCrpy4FuX2nxPDW5uLdedujQfGvspe4

--
-- PostgreSQL database cluster dump complete
--

