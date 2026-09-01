--
-- PostgreSQL database dump
--

\restrict EzajMFJOBqOvXtvzw9p3wihctvYOZA04bryMaSdF2c1661I70VAv7vQihwUPJhV

-- Dumped from database version 14.24 (Debian 14.24-1.pgdg13+2)
-- Dumped by pg_dump version 14.24 (Debian 14.24-1.pgdg13+2)

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
-- Name: mood; Type: TYPE; Schema: public; Owner: postgres
--

CREATE TYPE public.mood AS ENUM (
    'sad',
    'ok',
    'happy'
);


ALTER TYPE public.mood OWNER TO postgres;

SET default_tablespace = '';

--
-- Name: evt; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.evt (
    id integer,
    region text
)
PARTITION BY LIST (region);


ALTER TABLE public.evt OWNER TO postgres;

SET default_table_access_method = heap;

--
-- Name: evt_a; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.evt_a (
    id integer,
    region text
);


ALTER TABLE public.evt_a OWNER TO postgres;

--
-- Name: evt_m; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.evt_m (
    note text
);


ALTER TABLE public.evt_m OWNER TO postgres;

--
-- Name: evt_z; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.evt_z (
    id integer,
    region text
);


ALTER TABLE public.evt_z OWNER TO postgres;

--
-- Name: feel; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.feel (
    id integer,
    m public.mood
)
PARTITION BY HASH (m);


ALTER TABLE public.feel OWNER TO postgres;

--
-- Name: feel_a; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.feel_a (
    id integer,
    m public.mood
);


ALTER TABLE public.feel_a OWNER TO postgres;

--
-- Name: feel_m; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.feel_m (
    note text
);


ALTER TABLE public.feel_m OWNER TO postgres;

--
-- Name: feel_z; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.feel_z (
    id integer,
    m public.mood
);


ALTER TABLE public.feel_z OWNER TO postgres;

--
-- Name: spread; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.spread (
    id integer,
    m public.mood
)
PARTITION BY HASH (m);


ALTER TABLE public.spread OWNER TO postgres;

--
-- Name: spread_a; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.spread_a (
    id integer,
    m public.mood
);


ALTER TABLE public.spread_a OWNER TO postgres;

--
-- Name: spread_z; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.spread_z (
    id integer,
    m public.mood
);


ALTER TABLE public.spread_z OWNER TO postgres;

--
-- Name: evt_a; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.evt ATTACH PARTITION public.evt_a FOR VALUES IN ('a');


--
-- Name: evt_z; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.evt ATTACH PARTITION public.evt_z FOR VALUES IN ('z');


--
-- Name: feel_a; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.feel ATTACH PARTITION public.feel_a FOR VALUES WITH (modulus 2, remainder 0);


--
-- Name: feel_z; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.feel ATTACH PARTITION public.feel_z FOR VALUES WITH (modulus 2, remainder 1);


--
-- Name: spread_a; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.spread ATTACH PARTITION public.spread_a FOR VALUES WITH (modulus 2, remainder 0);


--
-- Name: spread_z; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.spread ATTACH PARTITION public.spread_z FOR VALUES WITH (modulus 2, remainder 1);


--
-- Data for Name: evt_a; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.evt_a (id, region) FROM stdin;
1	a
2	a
\.


--
-- Data for Name: evt_m; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.evt_m (note) FROM stdin;
unrelated
\.


--
-- Data for Name: evt_z; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.evt_z (id, region) FROM stdin;
3	z
\.


--
-- Data for Name: feel_a; Type: TABLE DATA; Schema: public; Owner: postgres
--

-- load via partition root public.feel

COPY public.feel (id, m) FROM stdin;
1	sad
2	ok
3	happy
\.


--
-- Data for Name: feel_m; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.feel_m (note) FROM stdin;
unrelated
\.


--
-- Data for Name: feel_z; Type: TABLE DATA; Schema: public; Owner: postgres
--

-- load via partition root public.feel

COPY public.feel (id, m) FROM stdin;
\.


--
-- Data for Name: spread_a; Type: TABLE DATA; Schema: public; Owner: postgres
--

-- load via partition root public.spread

COPY public.spread (id, m) FROM stdin;
1	sad
\.


--
-- Data for Name: spread_z; Type: TABLE DATA; Schema: public; Owner: postgres
--

-- load via partition root public.spread

COPY public.spread (id, m) FROM stdin;
\.


--
-- PostgreSQL database dump complete
--

\unrestrict EzajMFJOBqOvXtvzw9p3wihctvYOZA04bryMaSdF2c1661I70VAv7vQihwUPJhV

