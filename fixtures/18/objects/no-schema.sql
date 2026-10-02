--
-- PostgreSQL database dump
--

\restrict OH3ifHQjmr1YxKyWVEgr0emf3CP80KBopM7hyy4MeJpTRFZb0vTasQ3GN6LCdQh

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
-- Data for Name: events_2024; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.events_2024 (id, event_date, payload) FROM stdin;
1	2024-06-01	first
2	2024-12-31	second
\.


--
-- Data for Name: widgets; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widgets (id, label, created_at) FROM stdin;
1	alpha	2026-10-02 04:19:18.468135+00
2	beta	2026-10-02 04:19:18.468135+00
\.


--
-- Data for Name: orders; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.orders (id, widget_id, quantity) FROM stdin;
1	1	3
2	2	1
3	1	7
\.


--
-- Data for Name: price$$list; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects."price$$list" (id, note) FROM stdin;
1	costs $x$ here
2	costs $x$ here
\.


--
-- Data for Name: secrets; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.secrets (owner_role, payload) FROM stdin;
\.


--
-- Data for Name: tablespaced_table; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.tablespaced_table (id) FROM stdin;
\.


--
-- Data for Name: widget_audit; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widget_audit (widget_id, changed_at) FROM stdin;
\.


--
-- Data for Name: widgets_deleted_log; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widgets_deleted_log (id, deleted_at) FROM stdin;
\.


--
-- Name: orders_id_seq; Type: SEQUENCE SET; Schema: objects; Owner: postgres
--

SELECT pg_catalog.setval('objects.orders_id_seq', 3, true);


--
-- Name: standalone_seq; Type: SEQUENCE SET; Schema: objects; Owner: postgres
--

SELECT pg_catalog.setval('objects.standalone_seq', 100, true);


--
-- Name: 16525; Type: BLOB METADATA; Schema: -; Owner: postgres
--

SELECT pg_catalog.lo_create('16525');

ALTER LARGE OBJECT 16525 OWNER TO postgres;

--
-- Name: LARGE OBJECT 16525; Type: COMMENT; Schema: -; Owner: postgres
--

COMMENT ON LARGE OBJECT 16525 IS 'first large object';


--
-- Name: 16526; Type: BLOB METADATA; Schema: -; Owner: postgres
--

SELECT pg_catalog.lo_create('16526');

ALTER LARGE OBJECT 16526 OWNER TO postgres;

--
-- Data for Name: 16525; Type: BLOBS; Schema: -; Owner: postgres
--

BEGIN;

SELECT pg_catalog.lo_open('16525', 131072);
SELECT pg_catalog.lowrite(0, '\x48656c6c6f2c204c4f21');
SELECT pg_catalog.lo_close(0);

COMMIT;

--
-- Data for Name: 16526; Type: BLOBS; Schema: -; Owner: postgres
--

BEGIN;

SELECT pg_catalog.lo_open('16526', 131072);
SELECT pg_catalog.lowrite(0, '\x00010203040506070809');
SELECT pg_catalog.lo_close(0);

COMMIT;

--
-- Name: LARGE OBJECT 16525; Type: ACL; Schema: -; Owner: postgres
--

GRANT SELECT ON LARGE OBJECT 16525 TO fixture_reader;


--
-- PostgreSQL database dump complete
--

\unrestrict OH3ifHQjmr1YxKyWVEgr0emf3CP80KBopM7hyy4MeJpTRFZb0vTasQ3GN6LCdQh

