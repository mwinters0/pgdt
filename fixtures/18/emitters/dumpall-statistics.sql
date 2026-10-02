--
-- PostgreSQL database cluster dump
--

\restrict 6cwNnlyjKGnzeGu9zrmatJmrzZwRjN2gNiCNz1sbalpDrZTM0f5qgMj5aXHdebK

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict 6cwNnlyjKGnzeGu9zrmatJmrzZwRjN2gNiCNz1sbalpDrZTM0f5qgMj5aXHdebK

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

\restrict FqFd1stg8tmgsjOyW1z6PBRSd099j2dVLshXA1EbUG9ZdfBspiHhOk8fMRnSBOz

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

\unrestrict FqFd1stg8tmgsjOyW1z6PBRSd099j2dVLshXA1EbUG9ZdfBspiHhOk8fMRnSBOz

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict hY6lypp2g3stqPSk5mVIqIVoAfzt376HmfoNNCRAa6PwBzeCMV0FhGIWE9m8nj4

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

\unrestrict hY6lypp2g3stqPSk5mVIqIVoAfzt376HmfoNNCRAa6PwBzeCMV0FhGIWE9m8nj4
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict hY6lypp2g3stqPSk5mVIqIVoAfzt376HmfoNNCRAa6PwBzeCMV0FhGIWE9m8nj4

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
-- Data for Name: named; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.named (id, label, born) FROM stdin;
1	first	2024-04-01
2	\N	\N
\.


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

\unrestrict hY6lypp2g3stqPSk5mVIqIVoAfzt376HmfoNNCRAa6PwBzeCMV0FhGIWE9m8nj4

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict tyrhAPMY3PtWddKVoJMJpVp8uVlJfscUeKDhwEiMNOxlaYgD84O3TiNnAQncuyT

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

\unrestrict tyrhAPMY3PtWddKVoJMJpVp8uVlJfscUeKDhwEiMNOxlaYgD84O3TiNnAQncuyT
\connect pgdt_fixture
\restrict tyrhAPMY3PtWddKVoJMJpVp8uVlJfscUeKDhwEiMNOxlaYgD84O3TiNnAQncuyT

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
-- PostgreSQL database dump complete
--

\unrestrict tyrhAPMY3PtWddKVoJMJpVp8uVlJfscUeKDhwEiMNOxlaYgD84O3TiNnAQncuyT

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict p0MHin76eMHgTzNoeZd0cJ12CJmLAyrIgeaDtk7VRKShpOqBuFtSQVenHVCccUY

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

\unrestrict p0MHin76eMHgTzNoeZd0cJ12CJmLAyrIgeaDtk7VRKShpOqBuFtSQVenHVCccUY

--
-- PostgreSQL database cluster dump complete
--

