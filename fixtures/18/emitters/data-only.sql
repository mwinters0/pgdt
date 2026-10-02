--
-- PostgreSQL database dump
--

\restrict CmgBGb2RHYOVLM8gcyBezjaBstJMCBqdUmRjdPmpFqcwmxIzsRp1x3ZXWnhVkQK

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
-- Data for Name: base_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

SET SESSION AUTHORIZATION 'postgres';

ALTER TABLE emitters.base_values DISABLE TRIGGER ALL;

COPY emitters.base_values (id, v_varchar, v_char, v_int2, v_main, v_pair) FROM stdin;
1	alpha	a	7	main text	(1.5,2)
2	\N	\N	\N	\N	\N
\.


ALTER TABLE emitters.base_values ENABLE TRIGGER ALL;

--
-- Data for Name: child; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.child DISABLE TRIGGER ALL;

COPY emitters.child (id, label, born, extra) FROM stdin;
2	c	2024-01-02	1.25
3	d	\N	\N
\.


ALTER TABLE emitters.child ENABLE TRIGGER ALL;

--
-- Data for Name: domain_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.domain_values DISABLE TRIGGER ALL;

COPY emitters.domain_values (id, v_positive, v_mood, v_pair) FROM stdin;
1	3	busy	(1,one)
2	\N	\N	\N
\.


ALTER TABLE emitters.domain_values ENABLE TRIGGER ALL;

--
-- Data for Name: grown; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.grown DISABLE TRIGGER ALL;

COPY emitters.grown (id, b, added) FROM stdin;
1	b	2024-02-29
2	b2	2024-02-29
3	b3	2024-02-29
\.


ALTER TABLE emitters.grown ENABLE TRIGGER ALL;

--
-- Data for Name: parent; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.parent DISABLE TRIGGER ALL;

COPY emitters.parent (id, label, born) FROM stdin;
1	p	2024-01-01
\.


ALTER TABLE emitters.parent ENABLE TRIGGER ALL;

--
-- Data for Name: people; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.people DISABLE TRIGGER ALL;

COPY emitters.people (name, born, height) FROM stdin;
ann	1990-05-01	170
bob	\N	\N
\.


ALTER TABLE emitters.people ENABLE TRIGGER ALL;

--
-- Data for Name: range_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.range_values DISABLE TRIGGER ALL;

COPY emitters.range_values (id, v_canon, v_diff, v_pattern) FROM stdin;
1	[1,6)	[1.5,2.5)	[a,m)
2	empty	(,0]	[n,)
3	\N	\N	\N
\.


ALTER TABLE emitters.range_values ENABLE TRIGGER ALL;

--
-- Data for Name: scratch; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.scratch DISABLE TRIGGER ALL;

COPY emitters.scratch (id, at, ok) FROM stdin;
1	2024-03-01	t
2	\N	f
\.


ALTER TABLE emitters.scratch ENABLE TRIGGER ALL;

--
-- Data for Name: stamped; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.stamped DISABLE TRIGGER ALL;

COPY emitters.stamped (id, stamps, now) FROM stdin;
1	{"2024-01-01 00:00:00+00","2024-01-02 12:30:00+00"}	7
2	\N	\N
\.


ALTER TABLE emitters.stamped ENABLE TRIGGER ALL;

--
-- Data for Name: trios; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.trios DISABLE TRIGGER ALL;

COPY emitters.trios (id, v) FROM stdin;
1	(1,2024-01-01)
2	\N
\.


ALTER TABLE emitters.trios ENABLE TRIGGER ALL;

--
-- Data for Name: tuned; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.tuned DISABLE TRIGGER ALL;

COPY emitters.tuned (id, note, amount) FROM stdin;
1	one	1.50
2	\N	\N
\.


ALTER TABLE emitters.tuned ENABLE TRIGGER ALL;

--
-- Data for Name: unidentified; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.unidentified DISABLE TRIGGER ALL;

COPY emitters.unidentified (id, label) FROM stdin;
1	x
\.


ALTER TABLE emitters.unidentified ENABLE TRIGGER ALL;

--
-- PostgreSQL database dump complete
--

\unrestrict CmgBGb2RHYOVLM8gcyBezjaBstJMCBqdUmRjdPmpFqcwmxIzsRp1x3ZXWnhVkQK

