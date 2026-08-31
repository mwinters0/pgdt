--
-- PostgreSQL database cluster dump
--

\restrict MYmauF7TZq3J9uZmhxeMqaE9kdFOSyuIAcHqhtkjIN4peQtSJq3TsgkSNd3s4Ik

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE postgres;
ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;

--
-- User Configurations
--








\unrestrict MYmauF7TZq3J9uZmhxeMqaE9kdFOSyuIAcHqhtkjIN4peQtSJq3TsgkSNd3s4Ik

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

\restrict O0lQiwPCvlEgU19VcwTkN1VeEX3YbeTM0pJRnkde8wzvUKoY3blrkToFgj6Z2Ex

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

\unrestrict O0lQiwPCvlEgU19VcwTkN1VeEX3YbeTM0pJRnkde8wzvUKoY3blrkToFgj6Z2Ex

--
-- Database "pgdq_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict VoEJMOyjTgDkY3Wykn2dgCS4KRPEtE3GKeUg6QM17bVjAyGuFjRcsfiRfGKiy3D

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
-- Name: pgdq_fixture; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE pgdq_fixture WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE_PROVIDER = libc LOCALE = 'en_US.utf8';


ALTER DATABASE pgdq_fixture OWNER TO postgres;

\unrestrict VoEJMOyjTgDkY3Wykn2dgCS4KRPEtE3GKeUg6QM17bVjAyGuFjRcsfiRfGKiy3D
\connect pgdq_fixture
\restrict VoEJMOyjTgDkY3Wykn2dgCS4KRPEtE3GKeUg6QM17bVjAyGuFjRcsfiRfGKiy3D

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
-- Name: logs; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA logs;


ALTER SCHEMA logs OWNER TO postgres;

--
-- Name: sample_fn(); Type: FUNCTION; Schema: public; Owner: postgres
--

CREATE FUNCTION public.sample_fn() RETURNS void
    LANGUAGE plpgsql
    AS $$
BEGIN
COPY public.widgets TO stdout;
COPY public.widgets FROM stdin WITH (FORMAT csv);
COPY public.widgets (id, name) FROM stdin;
1	adversarial
2	rows
\.
END;
$$;


ALTER FUNCTION public.sample_fn() OWNER TO postgres;

--
-- Name: tagged_fn(); Type: FUNCTION; Schema: public; Owner: postgres
--

CREATE FUNCTION public.tagged_fn() RETURNS text
    LANGUAGE sql
    AS $_$
    SELECT 'contains an inner $$ marker' AS note;
$_$;


ALTER FUNCTION public.tagged_fn() OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: events; Type: TABLE; Schema: logs; Owner: postgres
--

CREATE TABLE logs.events (
    event_id bigint NOT NULL,
    widget_id integer,
    message text,
    logged_at timestamp with time zone DEFAULT now()
);


ALTER TABLE logs.events OWNER TO postgres;

--
-- Name: dropped_column; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.dropped_column (
    id integer NOT NULL,
    keep_me text,
    also_keep boolean
);


ALTER TABLE public.dropped_column OWNER TO postgres;

--
-- Name: empty_table; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.empty_table (
    id integer NOT NULL,
    value text
);


ALTER TABLE public.empty_table OWNER TO postgres;

--
-- Name: escapes; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.escapes (
    codepoint integer NOT NULL,
    value text NOT NULL
);


ALTER TABLE public.escapes OWNER TO postgres;

--
-- Name: generated_column; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.generated_column (
    id integer NOT NULL,
    a integer,
    b integer,
    total integer GENERATED ALWAYS AS ((a + b)) STORED
);


ALTER TABLE public.generated_column OWNER TO postgres;

--
-- Name: widgets; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.widgets (
    id integer NOT NULL,
    name text,
    description text,
    is_active boolean,
    created_at timestamp with time zone
);


ALTER TABLE public.widgets OWNER TO postgres;

--
-- Data for Name: events; Type: TABLE DATA; Schema: logs; Owner: postgres
--

COPY logs.events (event_id, widget_id, message, logged_at) FROM stdin;
100	1	created	2026-08-31 22:50:18.293748+00
101	2	\N	2026-08-31 22:50:18.293748+00
102	3	updated\twith a tab char	2026-08-31 22:50:18.293748+00
\.


--
-- Data for Name: dropped_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.dropped_column (id, keep_me, also_keep) FROM stdin;
1	x	t
2	y	f
\.


--
-- Data for Name: empty_table; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.empty_table (id, value) FROM stdin;
\.


--
-- Data for Name: escapes; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.escapes (codepoint, value) FROM stdin;
1	
2	
3	
4	
5	
6	
7	
8	\b
9	\t
10	\n
11	\v
12	\f
13	\r
14	
15	
16	
17	
18	
19	
20	
21	
22	
23	
24	
25	
26	
27	
28	
29	
30	
31	
32	 
33	!
34	"
35	#
36	$
37	%
38	&
39	'
40	(
41	)
42	*
43	+
44	,
45	-
46	.
47	/
48	0
49	1
50	2
51	3
52	4
53	5
54	6
55	7
56	8
57	9
58	:
59	;
60	<
61	=
62	>
63	?
64	@
65	A
66	B
67	C
68	D
69	E
70	F
71	G
72	H
73	I
74	J
75	K
76	L
77	M
78	N
79	O
80	P
81	Q
82	R
83	S
84	T
85	U
86	V
87	W
88	X
89	Y
90	Z
91	[
92	\\
93	]
94	^
95	_
96	`
97	a
98	b
99	c
100	d
101	e
102	f
103	g
104	h
105	i
106	j
107	k
108	l
109	m
110	n
111	o
112	p
113	q
114	r
115	s
116	t
117	u
118	v
119	w
120	x
121	y
122	z
123	{
124	|
125	}
126	~
127	
233	é
1071	Я
12354	あ
8364	€
128169	💩
\.


--
-- Data for Name: generated_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.generated_column (id, a, b) FROM stdin;
1	2	3
2	10	-4
\.


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
-- Name: events events_pkey; Type: CONSTRAINT; Schema: logs; Owner: postgres
--

ALTER TABLE ONLY logs.events
    ADD CONSTRAINT events_pkey PRIMARY KEY (event_id);


--
-- Name: dropped_column dropped_column_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.dropped_column
    ADD CONSTRAINT dropped_column_pkey PRIMARY KEY (id);


--
-- Name: empty_table empty_table_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.empty_table
    ADD CONSTRAINT empty_table_pkey PRIMARY KEY (id);


--
-- Name: escapes escapes_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.escapes
    ADD CONSTRAINT escapes_pkey PRIMARY KEY (codepoint);


--
-- Name: generated_column generated_column_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.generated_column
    ADD CONSTRAINT generated_column_pkey PRIMARY KEY (id);


--
-- Name: widgets widgets_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.widgets
    ADD CONSTRAINT widgets_pkey PRIMARY KEY (id);


--
-- Name: events events_widget_id_fkey; Type: FK CONSTRAINT; Schema: logs; Owner: postgres
--

ALTER TABLE ONLY logs.events
    ADD CONSTRAINT events_widget_id_fkey FOREIGN KEY (widget_id) REFERENCES public.widgets(id);


--
-- PostgreSQL database dump complete
--

\unrestrict VoEJMOyjTgDkY3Wykn2dgCS4KRPEtE3GKeUg6QM17bVjAyGuFjRcsfiRfGKiy3D

--
-- Database "pgdq_tenant" dump
--

--
-- PostgreSQL database dump
--

\restrict ZFyIQciLvfI5GGMxE3Uq7N2cMNnPlO5RDTQ8o4zjQWOANx1bPHsVhssBmgDfesI

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
-- Name: pgdq_tenant; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE pgdq_tenant WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE_PROVIDER = libc LOCALE = 'en_US.utf8';


ALTER DATABASE pgdq_tenant OWNER TO postgres;

\unrestrict ZFyIQciLvfI5GGMxE3Uq7N2cMNnPlO5RDTQ8o4zjQWOANx1bPHsVhssBmgDfesI
\connect pgdq_tenant
\restrict ZFyIQciLvfI5GGMxE3Uq7N2cMNnPlO5RDTQ8o4zjQWOANx1bPHsVhssBmgDfesI

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
-- Name: tenant; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA tenant;


ALTER SCHEMA tenant OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: tenant_only; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.tenant_only (
    slug text NOT NULL,
    seats integer NOT NULL
);


ALTER TABLE public.tenant_only OWNER TO postgres;

--
-- Name: widgets; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.widgets (
    id bigint NOT NULL,
    name uuid,
    description bytea,
    is_active smallint,
    created_at date
);


ALTER TABLE public.widgets OWNER TO postgres;

--
-- Name: ledger; Type: TABLE; Schema: tenant; Owner: postgres
--

CREATE TABLE tenant.ledger (
    entry_id bigint NOT NULL,
    slug text,
    amount numeric(12,2),
    posted_at timestamp with time zone
);


ALTER TABLE tenant.ledger OWNER TO postgres;

--
-- Name: settings; Type: TABLE; Schema: tenant; Owner: postgres
--

CREATE TABLE tenant.settings (
    key text NOT NULL,
    value text
);


ALTER TABLE tenant.settings OWNER TO postgres;

--
-- Data for Name: tenant_only; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.tenant_only (slug, seats) FROM stdin;
acme	12
globex	3
\.


--
-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.widgets (id, name, description, is_active, created_at) FROM stdin;
9000000001	00000000-0000-0000-0000-000000000001	\\x00ff10	1	2025-03-01
9000000002	3f2504e0-4f89-11d3-9a0c-0305e82c3301	\N	0	2025-03-02
9000000003	\N	\\xdeadbeef	\N	\N
\.


--
-- Data for Name: ledger; Type: TABLE DATA; Schema: tenant; Owner: postgres
--

COPY tenant.ledger (entry_id, slug, amount, posted_at) FROM stdin;
1	acme	10.50	2025-03-01 12:00:00+00
2	acme	-2.25	2025-03-02 12:00:00+00
3	globex	99.99	\N
\.


--
-- Data for Name: settings; Type: TABLE DATA; Schema: tenant; Owner: postgres
--

COPY tenant.settings (key, value) FROM stdin;
locale	en_GB
retention_days	30
\.


--
-- Name: tenant_only tenant_only_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.tenant_only
    ADD CONSTRAINT tenant_only_pkey PRIMARY KEY (slug);


--
-- Name: widgets widgets_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.widgets
    ADD CONSTRAINT widgets_pkey PRIMARY KEY (id);


--
-- Name: ledger ledger_pkey; Type: CONSTRAINT; Schema: tenant; Owner: postgres
--

ALTER TABLE ONLY tenant.ledger
    ADD CONSTRAINT ledger_pkey PRIMARY KEY (entry_id);


--
-- Name: settings settings_pkey; Type: CONSTRAINT; Schema: tenant; Owner: postgres
--

ALTER TABLE ONLY tenant.settings
    ADD CONSTRAINT settings_pkey PRIMARY KEY (key);


--
-- Name: ledger ledger_slug_fkey; Type: FK CONSTRAINT; Schema: tenant; Owner: postgres
--

ALTER TABLE ONLY tenant.ledger
    ADD CONSTRAINT ledger_slug_fkey FOREIGN KEY (slug) REFERENCES public.tenant_only(slug);


--
-- PostgreSQL database dump complete
--

\unrestrict ZFyIQciLvfI5GGMxE3Uq7N2cMNnPlO5RDTQ8o4zjQWOANx1bPHsVhssBmgDfesI

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict LYdwlsN8QujvddRtgVHyRJckmJLpPlfufW91hzHuDVa0ezuCWEj7q9AvPNu4sU7

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

\unrestrict LYdwlsN8QujvddRtgVHyRJckmJLpPlfufW91hzHuDVa0ezuCWEj7q9AvPNu4sU7

--
-- PostgreSQL database cluster dump complete
--

