--
-- PostgreSQL database dump
--

\restrict SI0mYp9Q9Am4Pn1iR3ihA6B3dpj6IYjL1VDKDNK8XykaSxVzCqDyadTcJVb8BCS

-- Dumped from database version 13.23
-- Dumped by pg_dump version 13.23

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Data for Name: t_array; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_array (id, v_empty, v_with_null, v_null_array, v_text_special, v_enum_array) FROM stdin;
1	{}	{NULL}	\N	{"a,b","c{d}","e\\"f","g\\\\h"}	{sad,"has space","has,comma",has'quote}
2	{1,2,3}	{1,NULL,3}	{1,2}	{NULL,plain}	{NULL,ok}
\.


--
-- Data for Name: t_array_shape; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_array_shape (id, v_multidim, v_mixed_dim, v_lbound) FROM stdin;
1	{{1,2},{3,4}}	{1,2}	[0:2]={7,8,9}
2	\N	{{1,2},{3,4}}	[-1:0]={10,11}
3	{{5,6},{7,8}}	\N	\N
\.


--
-- Data for Name: t_array_spelling; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_array_spelling (id, v_bounded, v_bounded_2d, v_array_kw, v_array_kw_n) FROM stdin;
1	{1,2,3,4}	{5,6}	{7,8}	{9}
2	{}	\N	{NULL,10}	{}
3	\N	\N	\N	\N
\.


--
-- Data for Name: t_base_type; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_base_type (id, v_mybase, v_mybase_array) FROM stdin;
1	hello	{hello,"a,b"}
2	\N	\N
\.


--
-- Data for Name: t_bytea; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_bytea (id, v_bytea) FROM stdin;
1	\\x
2	\N
3	\\xdeadbeef00ff
4	\\x5c6261636b736c617368
\.


--
-- Data for Name: t_composite; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_composite (id, v_point, v_points, v_tagged, v_empty_comp) FROM stdin;
1	(1,"a,b""c")	{"(1,\\"a,b\\"\\"c\\")","(2,plain)"}	("a,b","{""x\\\\""y"",""p q"",NULL}")	()
2	\N	\N	\N	\N
3	(,"")	{NULL,"(3,)"}	("",{})	()
\.


--
-- Data for Name: t_date; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_date (id, v_date) FROM stdin;
1	infinity
2	-infinity
3	0001-01-01
4	9999-12-31
5	0044-01-01 BC
6	10000-01-01
7	\N
\.


--
-- Data for Name: t_delimiter; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_delimiter (id, v_box_domain, v_box_domain_array) FROM stdin;
1	(1,1),(0,0)	{(1,1),(0,0);(3,3),(2,2)}
2	\N	\N
\.


--
-- Data for Name: t_enum_domain; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_enum_domain (id, v_mood, v_domain, v_empty_enum) FROM stdin;
1	sad	5	\N
2	has space	0	\N
3	has,comma	-5	\N
4	has'quote	100	\N
\.


--
-- Data for Name: t_float; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_float (id, v_real, v_double) FROM stdin;
1	NaN	NaN
2	Infinity	Infinity
3	-Infinity	-Infinity
4	-0	-0
5	1.1754944e-38	2.2250738585072014e-308
6	3.1415927	3.14159265358979
7	\N	\N
\.


--
-- Data for Name: t_int; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_int (id, v_smallint, v_integer, v_bigint) FROM stdin;
1	-32768	-2147483648	-9223372036854775808
2	32767	2147483647	9223372036854775807
3	0	0	0
4	\N	\N	\N
\.


--
-- Data for Name: t_interval; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_interval (id, v_interval) FROM stdin;
1	1 year 2 mons 3 days 04:05:06
2	-1 days
3	00:00:00
4	01:30:00
5	\N
\.


--
-- Data for Name: t_json; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_json (id, v_json, v_jsonb) FROM stdin;
1	{"a":1,"b":[1,2,3]}	{"a": 1, "b": [1, 2, 3]}
2	null	null
3	\N	\N
\.


--
-- Data for Name: t_nested_array; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_nested_array (id, v_nested_array, v_arr_holder, v_pointdom, v_pointdom_array, v_boxed_point, v_myrange_array, v_rangedom) FROM stdin;
1	{"{1,2}","{3}"}	(L,"{""{1,2}""}")	(1,"a,b""c")	{"(1,\\"a,b\\"\\"c\\")","(2,plain)"}	(outer,"(3,""x y"")")	{"[1.5,10.5)",empty}	[2.5,3.5)
2	{"{}","{5,NULL}"}	("",)	(,"")	{NULL}	(,)	{NULL}	\N
3	\N	\N	\N	\N	\N	\N	\N
\.


--
-- Data for Name: t_net; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_net (id, v_inet, v_cidr, v_macaddr, v_macaddr8) FROM stdin;
1	192.168.1.1	192.168.1.0/24	08:00:2b:01:02:03	08:00:2b:01:02:03:04:05
2	::1	::/0	\N	\N
3	\N	\N	\N	\N
\.


--
-- Data for Name: t_numeric; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_numeric (id, v_typed, v_typed39, v_small, v_untyped) FROM stdin;
1	1234567890123456789012345678.1234567890	\N	\N	\N
2	\N	123456789.1234567890	\N	\N
3	\N	\N	NaN	NaN
4	0.0000000000	0.0000000000	0.00	0
5	-1.5000000000	-1.5000000000	-1.50	100.00
6	\N	\N	\N	12345.6789
7	\N	\N	\N	\N
\.


--
-- Data for Name: t_range; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_range (id, v_range) FROM stdin;
1	[1,10)
2	empty
3	(,5)
4	\N
\.


--
-- Data for Name: t_text; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_text (id, v_text, v_varchar, v_char) FROM stdin;
1			          
2	\N	\N	\N
3	hello	hello	hi        
\.


--
-- Data for Name: t_text_range; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_text_range (id, v_textrange) FROM stdin;
1	["a,b","c""d")
2	[" lead","trail ")
3	["",a)
4	(,z)
5	empty
6	\N
\.


--
-- Data for Name: t_time; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_time (id, v_time, v_timetz) FROM stdin;
1	24:00:00	24:00:00+00
2	00:00:00.000001	00:00:00.000001-05
3	\N	\N
\.


--
-- Data for Name: t_timestamp; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_timestamp (id, v_ts, v_tstz) FROM stdin;
1	infinity	infinity
2	-infinity	-infinity
3	2024-01-01 00:00:00	2023-12-31 18:30:00+00
4	2024-01-01 00:00:00.123456	2024-01-01 00:00:00.123456+00
5	0001-01-01 00:00:00	0001-01-01 00:00:00+00
6	0044-01-01 00:00:00 BC	0044-01-01 00:00:00+00 BC
7	294276-12-31 23:59:59.999999	294276-12-31 23:59:59.999999+00
8	\N	\N
\.


--
-- Data for Name: t_user_range; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_user_range (id, v_myrange) FROM stdin;
1	[1.5,10.5)
2	empty
3	\N
\.


--
-- Data for Name: t_uuid; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.t_uuid (id, v_uuid) FROM stdin;
1	00000000-0000-0000-0000-000000000000
2	a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11
3	\N
\.


--
-- PostgreSQL database dump complete
--

\unrestrict SI0mYp9Q9Am4Pn1iR3ihA6B3dpj6IYjL1VDKDNK8XykaSxVzCqDyadTcJVb8BCS

