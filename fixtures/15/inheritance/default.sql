--
-- PostgreSQL database dump
--

\restrict 3FFXdyZmTJn9WqeOlhcj63hZokcDBedfFJShQjWkMT9ZPA2tZhsBB6U58Pnl0tU

-- Dumped from database version 15.19 (Debian 15.19-1.pgdg13+2)
-- Dumped by pg_dump version 15.19 (Debian 15.19-1.pgdg13+2)

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
-- Name: kennel; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA kennel;


ALTER SCHEMA kennel OWNER TO postgres;

SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: animal; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.animal (
    id integer,
    name text
);


ALTER TABLE public.animal OWNER TO postgres;

--
-- Name: mammal; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.mammal (
    legs integer
)
INHERITS (public.animal);


ALTER TABLE public.mammal OWNER TO postgres;

--
-- Name: dog; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.dog (
    breed text
)
INHERITS (public.mammal);


ALTER TABLE public.dog OWNER TO postgres;

--
-- Name: puppy; Type: TABLE; Schema: kennel; Owner: postgres
--

CREATE TABLE kennel.puppy (
    weeks integer
)
INHERITS (public.dog);


ALTER TABLE kennel.puppy OWNER TO postgres;

--
-- Name: bird; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.bird (
    name text,
    wings integer
)
INHERITS (public.animal);


ALTER TABLE public.bird OWNER TO postgres;

--
-- Name: pet; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.pet (
    owner text
);


ALTER TABLE public.pet OWNER TO postgres;

--
-- Name: cat; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.cat (
)
INHERITS (public.mammal, public.pet);


ALTER TABLE public.cat OWNER TO postgres;

--
-- Name: fossil; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.fossil (
)
INHERITS (public.animal);


ALTER TABLE public.fossil OWNER TO postgres;

--
-- Name: reading; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.reading (
    year integer,
    region text,
    value integer
)
PARTITION BY RANGE (year);


ALTER TABLE public.reading OWNER TO postgres;

--
-- Name: reading_2020; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.reading_2020 (
    year integer,
    region text,
    value integer
)
PARTITION BY LIST (region);


ALTER TABLE public.reading_2020 OWNER TO postgres;

--
-- Name: reading_2020_north; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.reading_2020_north (
    year integer,
    region text,
    value integer
);


ALTER TABLE public.reading_2020_north OWNER TO postgres;

--
-- Name: reading_2020_south; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.reading_2020_south (
    value integer,
    region text,
    year integer
);


ALTER TABLE public.reading_2020_south OWNER TO postgres;

--
-- Name: reading_2021; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.reading_2021 (
    year integer,
    region text,
    value integer
);


ALTER TABLE public.reading_2021 OWNER TO postgres;

--
-- Name: reading_2020; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.reading ATTACH PARTITION public.reading_2020 FOR VALUES FROM (2020) TO (2021);


--
-- Name: reading_2020_north; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.reading_2020 ATTACH PARTITION public.reading_2020_north FOR VALUES IN ('north');


--
-- Name: reading_2020_south; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.reading_2020 ATTACH PARTITION public.reading_2020_south FOR VALUES IN ('south');


--
-- Name: reading_2021; Type: TABLE ATTACH; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.reading ATTACH PARTITION public.reading_2021 FOR VALUES FROM (2021) TO (2022);


--
-- Data for Name: puppy; Type: TABLE DATA; Schema: kennel; Owner: postgres
--

COPY kennel.puppy (id, name, legs, breed, weeks) FROM stdin;
5	bit	4	pug	8
\.


--
-- Data for Name: animal; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.animal (id, name) FROM stdin;
1	generic
\.


--
-- Data for Name: bird; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.bird (id, name, wings) FROM stdin;
7	tweety	2
\.


--
-- Data for Name: cat; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.cat (id, name, legs, owner) FROM stdin;
6	tom	4	ann
\.


--
-- Data for Name: dog; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.dog (id, name, legs, breed) FROM stdin;
3	rex	4	collie
4	fido	4	pug
\.


--
-- Data for Name: fossil; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.fossil (id, name) FROM stdin;
\.


--
-- Data for Name: mammal; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.mammal (id, name, legs) FROM stdin;
2	whale	0
\.


--
-- Data for Name: pet; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.pet (owner) FROM stdin;
\.


--
-- Data for Name: reading_2020_north; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.reading_2020_north (year, region, value) FROM stdin;
2020	north	1
\.


--
-- Data for Name: reading_2020_south; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.reading_2020_south (value, region, year) FROM stdin;
2	south	2020
3	south	2020
\.


--
-- Data for Name: reading_2021; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.reading_2021 (year, region, value) FROM stdin;
2021	north	4
\.


--
-- PostgreSQL database dump complete
--

\unrestrict 3FFXdyZmTJn9WqeOlhcj63hZokcDBedfFJShQjWkMT9ZPA2tZhsBB6U58Pnl0tU

