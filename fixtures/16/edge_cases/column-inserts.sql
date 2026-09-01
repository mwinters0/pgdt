--
-- PostgreSQL database dump
--

\restrict Y5amT0UZQSZYrTQCJgPJXrP08QChN0t9WdaFyDDdvqRjVibVrvxWpMfXjHsL38F

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

INSERT INTO logs.events (event_id, widget_id, message, logged_at) VALUES (100, 1, 'created', '2026-08-31 23:53:24.691486+00');
INSERT INTO logs.events (event_id, widget_id, message, logged_at) VALUES (101, 2, NULL, '2026-08-31 23:53:24.691486+00');
INSERT INTO logs.events (event_id, widget_id, message, logged_at) VALUES (102, 3, 'updated	with a tab char', '2026-08-31 23:53:24.691486+00');


--
-- Data for Name: dropped_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.dropped_column (id, keep_me, also_keep) VALUES (1, 'x', true);
INSERT INTO public.dropped_column (id, keep_me, also_keep) VALUES (2, 'y', false);


--
-- Data for Name: empty_table; Type: TABLE DATA; Schema: public; Owner: postgres
--



--
-- Data for Name: escapes; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.escapes (codepoint, value) VALUES (1, '');
INSERT INTO public.escapes (codepoint, value) VALUES (2, '');
INSERT INTO public.escapes (codepoint, value) VALUES (3, '');
INSERT INTO public.escapes (codepoint, value) VALUES (4, '');
INSERT INTO public.escapes (codepoint, value) VALUES (5, '');
INSERT INTO public.escapes (codepoint, value) VALUES (6, '');
INSERT INTO public.escapes (codepoint, value) VALUES (7, '');
INSERT INTO public.escapes (codepoint, value) VALUES (8, '');
INSERT INTO public.escapes (codepoint, value) VALUES (9, '	');
INSERT INTO public.escapes (codepoint, value) VALUES (10, '
');
INSERT INTO public.escapes (codepoint, value) VALUES (11, '');
INSERT INTO public.escapes (codepoint, value) VALUES (12, '');
INSERT INTO public.escapes (codepoint, value) VALUES (13, '
');
INSERT INTO public.escapes (codepoint, value) VALUES (14, '');
INSERT INTO public.escapes (codepoint, value) VALUES (15, '');
INSERT INTO public.escapes (codepoint, value) VALUES (16, '');
INSERT INTO public.escapes (codepoint, value) VALUES (17, '');
INSERT INTO public.escapes (codepoint, value) VALUES (18, '');
INSERT INTO public.escapes (codepoint, value) VALUES (19, '');
INSERT INTO public.escapes (codepoint, value) VALUES (20, '');
INSERT INTO public.escapes (codepoint, value) VALUES (21, '');
INSERT INTO public.escapes (codepoint, value) VALUES (22, '');
INSERT INTO public.escapes (codepoint, value) VALUES (23, '');
INSERT INTO public.escapes (codepoint, value) VALUES (24, '');
INSERT INTO public.escapes (codepoint, value) VALUES (25, '');
INSERT INTO public.escapes (codepoint, value) VALUES (26, '');
INSERT INTO public.escapes (codepoint, value) VALUES (27, '');
INSERT INTO public.escapes (codepoint, value) VALUES (28, '');
INSERT INTO public.escapes (codepoint, value) VALUES (29, '');
INSERT INTO public.escapes (codepoint, value) VALUES (30, '');
INSERT INTO public.escapes (codepoint, value) VALUES (31, '');
INSERT INTO public.escapes (codepoint, value) VALUES (32, ' ');
INSERT INTO public.escapes (codepoint, value) VALUES (33, '!');
INSERT INTO public.escapes (codepoint, value) VALUES (34, '"');
INSERT INTO public.escapes (codepoint, value) VALUES (35, '#');
INSERT INTO public.escapes (codepoint, value) VALUES (36, '$');
INSERT INTO public.escapes (codepoint, value) VALUES (37, '%');
INSERT INTO public.escapes (codepoint, value) VALUES (38, '&');
INSERT INTO public.escapes (codepoint, value) VALUES (39, '''');
INSERT INTO public.escapes (codepoint, value) VALUES (40, '(');
INSERT INTO public.escapes (codepoint, value) VALUES (41, ')');
INSERT INTO public.escapes (codepoint, value) VALUES (42, '*');
INSERT INTO public.escapes (codepoint, value) VALUES (43, '+');
INSERT INTO public.escapes (codepoint, value) VALUES (44, ',');
INSERT INTO public.escapes (codepoint, value) VALUES (45, '-');
INSERT INTO public.escapes (codepoint, value) VALUES (46, '.');
INSERT INTO public.escapes (codepoint, value) VALUES (47, '/');
INSERT INTO public.escapes (codepoint, value) VALUES (48, '0');
INSERT INTO public.escapes (codepoint, value) VALUES (49, '1');
INSERT INTO public.escapes (codepoint, value) VALUES (50, '2');
INSERT INTO public.escapes (codepoint, value) VALUES (51, '3');
INSERT INTO public.escapes (codepoint, value) VALUES (52, '4');
INSERT INTO public.escapes (codepoint, value) VALUES (53, '5');
INSERT INTO public.escapes (codepoint, value) VALUES (54, '6');
INSERT INTO public.escapes (codepoint, value) VALUES (55, '7');
INSERT INTO public.escapes (codepoint, value) VALUES (56, '8');
INSERT INTO public.escapes (codepoint, value) VALUES (57, '9');
INSERT INTO public.escapes (codepoint, value) VALUES (58, ':');
INSERT INTO public.escapes (codepoint, value) VALUES (59, ';');
INSERT INTO public.escapes (codepoint, value) VALUES (60, '<');
INSERT INTO public.escapes (codepoint, value) VALUES (61, '=');
INSERT INTO public.escapes (codepoint, value) VALUES (62, '>');
INSERT INTO public.escapes (codepoint, value) VALUES (63, '?');
INSERT INTO public.escapes (codepoint, value) VALUES (64, '@');
INSERT INTO public.escapes (codepoint, value) VALUES (65, 'A');
INSERT INTO public.escapes (codepoint, value) VALUES (66, 'B');
INSERT INTO public.escapes (codepoint, value) VALUES (67, 'C');
INSERT INTO public.escapes (codepoint, value) VALUES (68, 'D');
INSERT INTO public.escapes (codepoint, value) VALUES (69, 'E');
INSERT INTO public.escapes (codepoint, value) VALUES (70, 'F');
INSERT INTO public.escapes (codepoint, value) VALUES (71, 'G');
INSERT INTO public.escapes (codepoint, value) VALUES (72, 'H');
INSERT INTO public.escapes (codepoint, value) VALUES (73, 'I');
INSERT INTO public.escapes (codepoint, value) VALUES (74, 'J');
INSERT INTO public.escapes (codepoint, value) VALUES (75, 'K');
INSERT INTO public.escapes (codepoint, value) VALUES (76, 'L');
INSERT INTO public.escapes (codepoint, value) VALUES (77, 'M');
INSERT INTO public.escapes (codepoint, value) VALUES (78, 'N');
INSERT INTO public.escapes (codepoint, value) VALUES (79, 'O');
INSERT INTO public.escapes (codepoint, value) VALUES (80, 'P');
INSERT INTO public.escapes (codepoint, value) VALUES (81, 'Q');
INSERT INTO public.escapes (codepoint, value) VALUES (82, 'R');
INSERT INTO public.escapes (codepoint, value) VALUES (83, 'S');
INSERT INTO public.escapes (codepoint, value) VALUES (84, 'T');
INSERT INTO public.escapes (codepoint, value) VALUES (85, 'U');
INSERT INTO public.escapes (codepoint, value) VALUES (86, 'V');
INSERT INTO public.escapes (codepoint, value) VALUES (87, 'W');
INSERT INTO public.escapes (codepoint, value) VALUES (88, 'X');
INSERT INTO public.escapes (codepoint, value) VALUES (89, 'Y');
INSERT INTO public.escapes (codepoint, value) VALUES (90, 'Z');
INSERT INTO public.escapes (codepoint, value) VALUES (91, '[');
INSERT INTO public.escapes (codepoint, value) VALUES (92, '\');
INSERT INTO public.escapes (codepoint, value) VALUES (93, ']');
INSERT INTO public.escapes (codepoint, value) VALUES (94, '^');
INSERT INTO public.escapes (codepoint, value) VALUES (95, '_');
INSERT INTO public.escapes (codepoint, value) VALUES (96, '`');
INSERT INTO public.escapes (codepoint, value) VALUES (97, 'a');
INSERT INTO public.escapes (codepoint, value) VALUES (98, 'b');
INSERT INTO public.escapes (codepoint, value) VALUES (99, 'c');
INSERT INTO public.escapes (codepoint, value) VALUES (100, 'd');
INSERT INTO public.escapes (codepoint, value) VALUES (101, 'e');
INSERT INTO public.escapes (codepoint, value) VALUES (102, 'f');
INSERT INTO public.escapes (codepoint, value) VALUES (103, 'g');
INSERT INTO public.escapes (codepoint, value) VALUES (104, 'h');
INSERT INTO public.escapes (codepoint, value) VALUES (105, 'i');
INSERT INTO public.escapes (codepoint, value) VALUES (106, 'j');
INSERT INTO public.escapes (codepoint, value) VALUES (107, 'k');
INSERT INTO public.escapes (codepoint, value) VALUES (108, 'l');
INSERT INTO public.escapes (codepoint, value) VALUES (109, 'm');
INSERT INTO public.escapes (codepoint, value) VALUES (110, 'n');
INSERT INTO public.escapes (codepoint, value) VALUES (111, 'o');
INSERT INTO public.escapes (codepoint, value) VALUES (112, 'p');
INSERT INTO public.escapes (codepoint, value) VALUES (113, 'q');
INSERT INTO public.escapes (codepoint, value) VALUES (114, 'r');
INSERT INTO public.escapes (codepoint, value) VALUES (115, 's');
INSERT INTO public.escapes (codepoint, value) VALUES (116, 't');
INSERT INTO public.escapes (codepoint, value) VALUES (117, 'u');
INSERT INTO public.escapes (codepoint, value) VALUES (118, 'v');
INSERT INTO public.escapes (codepoint, value) VALUES (119, 'w');
INSERT INTO public.escapes (codepoint, value) VALUES (120, 'x');
INSERT INTO public.escapes (codepoint, value) VALUES (121, 'y');
INSERT INTO public.escapes (codepoint, value) VALUES (122, 'z');
INSERT INTO public.escapes (codepoint, value) VALUES (123, '{');
INSERT INTO public.escapes (codepoint, value) VALUES (124, '|');
INSERT INTO public.escapes (codepoint, value) VALUES (125, '}');
INSERT INTO public.escapes (codepoint, value) VALUES (126, '~');
INSERT INTO public.escapes (codepoint, value) VALUES (127, '');
INSERT INTO public.escapes (codepoint, value) VALUES (233, 'é');
INSERT INTO public.escapes (codepoint, value) VALUES (1071, 'Я');
INSERT INTO public.escapes (codepoint, value) VALUES (12354, 'あ');
INSERT INTO public.escapes (codepoint, value) VALUES (8364, '€');
INSERT INTO public.escapes (codepoint, value) VALUES (128169, '💩');


--
-- Data for Name: generated_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.generated_column (id, a, b) VALUES (1, 2, 3);
INSERT INTO public.generated_column (id, a, b) VALUES (2, 10, -4);


--
-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES (1, 'alpha', 'a simple widget', true, '2024-01-01 00:00:00+00');
INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES (2, 'beta', NULL, false, '2024-01-02 00:00:00+00');
INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES (3, 'gamma', 'multi
line	description with a literal backslash \ and a quote '' inside', true, NULL);
INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES (4, 'delta', 'contains a COPY-like phrase: COPY public.widgets (id, name) TO stdout; -- not a real directive', true, '2024-01-04 00:00:00+00');
INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES (5, '', 'empty name above', NULL, '2024-01-05 00:00:00+00');


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

\unrestrict Y5amT0UZQSZYrTQCJgPJXrP08QChN0t9WdaFyDDdvqRjVibVrvxWpMfXjHsL38F

