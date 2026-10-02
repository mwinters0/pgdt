--
-- PostgreSQL database dump
--

\restrict pgKOu4rqJ2M1r7ysVl0yT86FQw8lvm9ne5B1rjXdLQS1mrOWckWhCromrHHyOWj

-- Dumped from database version 16.15 (Debian 16.15-1.pgdg13+2)
-- Dumped by pg_dump version 16.15 (Debian 16.15-1.pgdg13+2)

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

INSERT INTO logs.events VALUES (100, 1, 'created', '2026-10-02 03:00:25.945631+00') ON CONFLICT DO NOTHING;
INSERT INTO logs.events VALUES (101, 2, NULL, '2026-10-02 03:00:25.945631+00') ON CONFLICT DO NOTHING;
INSERT INTO logs.events VALUES (102, 3, 'updated	with a tab char', '2026-10-02 03:00:25.945631+00') ON CONFLICT DO NOTHING;


--
-- Data for Name: dropped_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.dropped_column VALUES (1, 'x', true) ON CONFLICT DO NOTHING;
INSERT INTO public.dropped_column VALUES (2, 'y', false) ON CONFLICT DO NOTHING;


--
-- Data for Name: empty_table; Type: TABLE DATA; Schema: public; Owner: postgres
--



--
-- Data for Name: escapes; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.escapes VALUES (1, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (2, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (3, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (4, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (5, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (6, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (7, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (8, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (9, '	') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (10, '
') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (11, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (12, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (13, '
') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (14, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (15, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (16, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (17, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (18, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (19, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (20, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (21, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (22, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (23, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (24, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (25, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (26, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (27, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (28, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (29, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (30, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (31, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (32, ' ') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (33, '!') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (34, '"') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (35, '#') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (36, '$') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (37, '%') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (38, '&') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (39, '''') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (40, '(') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (41, ')') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (42, '*') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (43, '+') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (44, ',') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (45, '-') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (46, '.') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (47, '/') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (48, '0') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (49, '1') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (50, '2') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (51, '3') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (52, '4') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (53, '5') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (54, '6') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (55, '7') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (56, '8') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (57, '9') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (58, ':') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (59, ';') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (60, '<') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (61, '=') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (62, '>') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (63, '?') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (64, '@') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (65, 'A') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (66, 'B') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (67, 'C') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (68, 'D') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (69, 'E') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (70, 'F') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (71, 'G') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (72, 'H') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (73, 'I') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (74, 'J') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (75, 'K') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (76, 'L') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (77, 'M') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (78, 'N') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (79, 'O') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (80, 'P') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (81, 'Q') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (82, 'R') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (83, 'S') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (84, 'T') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (85, 'U') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (86, 'V') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (87, 'W') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (88, 'X') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (89, 'Y') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (90, 'Z') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (91, '[') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (92, '\') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (93, ']') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (94, '^') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (95, '_') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (96, '`') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (97, 'a') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (98, 'b') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (99, 'c') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (100, 'd') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (101, 'e') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (102, 'f') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (103, 'g') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (104, 'h') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (105, 'i') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (106, 'j') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (107, 'k') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (108, 'l') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (109, 'm') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (110, 'n') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (111, 'o') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (112, 'p') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (113, 'q') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (114, 'r') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (115, 's') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (116, 't') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (117, 'u') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (118, 'v') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (119, 'w') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (120, 'x') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (121, 'y') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (122, 'z') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (123, '{') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (124, '|') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (125, '}') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (126, '~') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (127, '') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (233, 'é') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (1071, 'Я') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (12354, 'あ') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (8364, '€') ON CONFLICT DO NOTHING;
INSERT INTO public.escapes VALUES (128169, '💩') ON CONFLICT DO NOTHING;


--
-- Data for Name: generated_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.generated_column VALUES (1, 2, 3, DEFAULT) ON CONFLICT DO NOTHING;
INSERT INTO public.generated_column VALUES (2, 10, -4, DEFAULT) ON CONFLICT DO NOTHING;


--
-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.widgets VALUES (1, 'alpha', 'a simple widget', true, '2024-01-01 00:00:00+00') ON CONFLICT DO NOTHING;
INSERT INTO public.widgets VALUES (2, 'beta', NULL, false, '2024-01-02 00:00:00+00') ON CONFLICT DO NOTHING;
INSERT INTO public.widgets VALUES (3, 'gamma', 'multi
line	description with a literal backslash \ and a quote '' inside', true, NULL) ON CONFLICT DO NOTHING;
INSERT INTO public.widgets VALUES (4, 'delta', 'contains a COPY-like phrase: COPY public.widgets (id, name) TO stdout; -- not a real directive', true, '2024-01-04 00:00:00+00') ON CONFLICT DO NOTHING;
INSERT INTO public.widgets VALUES (5, '', 'empty name above', NULL, '2024-01-05 00:00:00+00') ON CONFLICT DO NOTHING;


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

\unrestrict pgKOu4rqJ2M1r7ysVl0yT86FQw8lvm9ne5B1rjXdLQS1mrOWckWhCromrHHyOWj

