--
-- PostgreSQL database dump
--

\restrict z8HUF6AvZhKQSZzdOfGoe2uH9RJuXF1kST7U8onuUNSH7TragUO2iYNkZAe6lpH

-- Dumped from database version 17.11
-- Dumped by pg_dump version 17.11

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
100	1	created	2026-08-22 23:17:22.034049+00
101	2	\N	2026-08-22 23:17:22.034049+00
102	3	updated\twith a tab char	2026-08-22 23:17:22.034049+00
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
-- PostgreSQL database dump complete
--

\unrestrict z8HUF6AvZhKQSZzdOfGoe2uH9RJuXF1kST7U8onuUNSH7TragUO2iYNkZAe6lpH

