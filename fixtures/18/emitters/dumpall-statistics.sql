--
-- PostgreSQL database cluster dump
--

\restrict NlEn039uwV4JGYpViPjLVffYmZni8W4hzQOlix88HcfoZPcogWFhn2aNqPMldVa

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict NlEn039uwV4JGYpViPjLVffYmZni8W4hzQOlix88HcfoZPcogWFhn2aNqPMldVa

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

\restrict W6YgW4zm5XvXkeUimSaZ5BV0JKWtr4AG7d7vvMAJLjDybI4HbICWVAbrKrLJnB5

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

\unrestrict W6YgW4zm5XvXkeUimSaZ5BV0JKWtr4AG7d7vvMAJLjDybI4HbICWVAbrKrLJnB5

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict MC6hRtaI8FDeLzmdhd88KFeaRyNUEN5MX5t4vI5kvC2qduCi3lrLbth9uUrDvry

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

\unrestrict MC6hRtaI8FDeLzmdhd88KFeaRyNUEN5MX5t4vI5kvC2qduCi3lrLbth9uUrDvry
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict MC6hRtaI8FDeLzmdhd88KFeaRyNUEN5MX5t4vI5kvC2qduCi3lrLbth9uUrDvry

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

\unrestrict MC6hRtaI8FDeLzmdhd88KFeaRyNUEN5MX5t4vI5kvC2qduCi3lrLbth9uUrDvry

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict OT5trghyRhz4uaaxIm9Iehv3Tf7PcOhrnY6I5w6gNdlM9ArG5I3X4hzBjeLSwNZ

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

\unrestrict OT5trghyRhz4uaaxIm9Iehv3Tf7PcOhrnY6I5w6gNdlM9ArG5I3X4hzBjeLSwNZ
\connect pgdt_fixture
\restrict OT5trghyRhz4uaaxIm9Iehv3Tf7PcOhrnY6I5w6gNdlM9ArG5I3X4hzBjeLSwNZ

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
-- Data for Name: booked; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

COPY emitters.booked (room, during) FROM stdin;
[1,2)	[2024-01-01,2024-01-05)
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

\unrestrict OT5trghyRhz4uaaxIm9Iehv3Tf7PcOhrnY6I5w6gNdlM9ArG5I3X4hzBjeLSwNZ

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict zi5jMJ0nl2B2iUDbGrbb9gjOSaaKiyjmRV7YCm4VcP8cSBHZQbOJmtWXB3cNtbc

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

\unrestrict zi5jMJ0nl2B2iUDbGrbb9gjOSaaKiyjmRV7YCm4VcP8cSBHZQbOJmtWXB3cNtbc

--
-- PostgreSQL database cluster dump complete
--

