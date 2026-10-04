--
-- PostgreSQL database cluster dump
--

\restrict ic3Yjk32ZgvEupv6TWNlZPRIkjihbisiV2KNtQ80AJoOgVzFZ937MgcENkFgBN2

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict ic3Yjk32ZgvEupv6TWNlZPRIkjihbisiV2KNtQ80AJoOgVzFZ937MgcENkFgBN2

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

\restrict j6JA2xubfyyF5ckYexgBHKXryc81xkluMfm4d9hJgBFfyXN8oQNdopV2b5afo9t

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

\unrestrict j6JA2xubfyyF5ckYexgBHKXryc81xkluMfm4d9hJgBFfyXN8oQNdopV2b5afo9t

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict uG5z8lz0W0RJIlaT0Wj6OGGdfVniDaV3hcG1GRtSpyJeTGHjdUQZvA5vm4TwrqG

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

\unrestrict uG5z8lz0W0RJIlaT0Wj6OGGdfVniDaV3hcG1GRtSpyJeTGHjdUQZvA5vm4TwrqG
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict uG5z8lz0W0RJIlaT0Wj6OGGdfVniDaV3hcG1GRtSpyJeTGHjdUQZvA5vm4TwrqG

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

\unrestrict uG5z8lz0W0RJIlaT0Wj6OGGdfVniDaV3hcG1GRtSpyJeTGHjdUQZvA5vm4TwrqG

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict O6BeKqSMrBza72sVAIXG6eofMf2yoWm2zsEGTaycBVwruTLOJqvVzxfyWp0u6hI

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

\unrestrict O6BeKqSMrBza72sVAIXG6eofMf2yoWm2zsEGTaycBVwruTLOJqvVzxfyWp0u6hI
\connect pgdt_fixture
\restrict O6BeKqSMrBza72sVAIXG6eofMf2yoWm2zsEGTaycBVwruTLOJqvVzxfyWp0u6hI

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

\unrestrict O6BeKqSMrBza72sVAIXG6eofMf2yoWm2zsEGTaycBVwruTLOJqvVzxfyWp0u6hI

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict OKIBJ5z1svli1KqrPpR4vkfYAtHbuev4xpqwIS7dHwNvUC2fEPO1ZteMzVOwrvz

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

\unrestrict OKIBJ5z1svli1KqrPpR4vkfYAtHbuev4xpqwIS7dHwNvUC2fEPO1ZteMzVOwrvz

--
-- PostgreSQL database cluster dump complete
--

