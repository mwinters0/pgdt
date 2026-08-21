--
-- PostgreSQL database dump
--

\restrict ci2zNfMaGH8YZHsNvi1K7ijEuiL6EhEDo6oYBrLf1uZulTZDJzav3UP6nsnqJrC

-- Dumped from database version 18.6
-- Dumped by pg_dump version 18.6

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
-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.widgets (id, name, description, is_active, created_at) FROM stdin;
1	alpha	a simple widget	t	2024-01-01 00:00:00+00
2	beta	\N	f	2024-01-02 00:00:00+00
3	gamma	multi\nline\tdescription with a literal backslash \\ and a quote ' inside	t	\N
4	delta	contains a COPY-like phrase: COPY public.widgets (id, name) TO stdout; -- not a real directive	t	2024-01-04 00:00:00+00
5		empty name above	\N	2024-01-05 00:00:00+00
\.


--
-- Data for Name: events; Type: TABLE DATA; Schema: logs; Owner: postgres
--

COPY logs.events (event_id, widget_id, message, logged_at) FROM stdin;
100	1	created	2026-08-21 23:10:58.502966+00
101	2	\N	2026-08-21 23:10:58.502966+00
102	3	updated\twith a tab char	2026-08-21 23:10:58.502966+00
\.


--
-- Data for Name: empty_table; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.empty_table (id, value) FROM stdin;
\.


--
-- PostgreSQL database dump complete
--

\unrestrict ci2zNfMaGH8YZHsNvi1K7ijEuiL6EhEDo6oYBrLf1uZulTZDJzav3UP6nsnqJrC

