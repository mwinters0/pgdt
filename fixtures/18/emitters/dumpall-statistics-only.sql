--
-- PostgreSQL database cluster dump
--

\restrict Ti5MWylQaMbf3FmVcPjRjIlkhLEGG47QtyAnBqhn9WxaGyaUyjGKRMepvW02ukJ

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict Ti5MWylQaMbf3FmVcPjRjIlkhLEGG47QtyAnBqhn9WxaGyaUyjGKRMepvW02ukJ

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

\restrict T7TgbtmO28gtRxSlMZqsOvFRBiPMIDhRFGtVdzhHudBP4KsqL9zYqs7Nk96BA99

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

\unrestrict T7TgbtmO28gtRxSlMZqsOvFRBiPMIDhRFGtVdzhHudBP4KsqL9zYqs7Nk96BA99

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict 7VW8nLQE5k5LPvJwlEdefPoyoqQVyKHJ5Lj35tCeoa5s1ZHHpbclaudz6apZWZi

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

\unrestrict 7VW8nLQE5k5LPvJwlEdefPoyoqQVyKHJ5Lj35tCeoa5s1ZHHpbclaudz6apZWZi
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict 7VW8nLQE5k5LPvJwlEdefPoyoqQVyKHJ5Lj35tCeoa5s1ZHHpbclaudz6apZWZi

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

\unrestrict 7VW8nLQE5k5LPvJwlEdefPoyoqQVyKHJ5Lj35tCeoa5s1ZHHpbclaudz6apZWZi

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict wI84G6nwQVQKC5nQcvvehIXlu9qYzdm4etDfb7yxp6FeWbqZ71JyKRj4tHWpGfI

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

\unrestrict wI84G6nwQVQKC5nQcvvehIXlu9qYzdm4etDfb7yxp6FeWbqZ71JyKRj4tHWpGfI
\connect pgdt_fixture
\restrict wI84G6nwQVQKC5nQcvvehIXlu9qYzdm4etDfb7yxp6FeWbqZ71JyKRj4tHWpGfI

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
-- Statistics for Name: booked; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'booked',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: built; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'built',
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
-- Statistics for Name: collated; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'collated',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: delimited; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'delimited',
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
-- Statistics for Name: identified; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'identified',
	'relpages', '0'::integer,
	'reltuples', '-1'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: keyed; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'keyed',
	'relpages', '1'::integer,
	'reltuples', '2'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: no_columns; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'no_columns',
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
-- Statistics for Name: ruled; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'ruled',
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
-- Statistics for Name: booked_key; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'booked_key',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: delimited_pkey; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'delimited_pkey',
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
-- Statistics for Name: keyed_code_key; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'keyed_code_key',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: keyed_id_key; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'keyed_id_key',
	'relpages', '1'::integer,
	'reltuples', '0'::real,
	'relallvisible', '0'::integer,
	'relallfrozen', '0'::integer
);


--
-- Statistics for Name: keyed_note_key; Type: STATISTICS DATA; Schema: emitters; Owner: -
--

SELECT * FROM pg_catalog.pg_restore_relation_stats(
	'version', '180006'::integer,
	'schemaname', 'emitters',
	'relname', 'keyed_note_key',
	'relpages', '2'::integer,
	'reltuples', '2'::real,
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

\unrestrict wI84G6nwQVQKC5nQcvvehIXlu9qYzdm4etDfb7yxp6FeWbqZ71JyKRj4tHWpGfI

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict RqcVkLtB4klqmr1paqcW5OkcVz69SmOMYdiMg9hp2o1dFK9CNTiJZlA7KdvXxc9

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

\unrestrict RqcVkLtB4klqmr1paqcW5OkcVz69SmOMYdiMg9hp2o1dFK9CNTiJZlA7KdvXxc9

--
-- PostgreSQL database cluster dump complete
--

