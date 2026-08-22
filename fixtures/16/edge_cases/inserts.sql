--
-- PostgreSQL database dump
--

\restrict 2mqEQ7Umn1NZ0d5zRpxAJf5z8YBkAiEK3WeBTbJLhCU4qUyPrvTv98s1kdNoUgj

-- Dumped from database version 16.15
-- Dumped by pg_dump version 16.15

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

INSERT INTO logs.events VALUES (100, 1, 'created', '2026-08-22 07:37:36.406539+00');
INSERT INTO logs.events VALUES (101, 2, NULL, '2026-08-22 07:37:36.406539+00');
INSERT INTO logs.events VALUES (102, 3, 'updated	with a tab char', '2026-08-22 07:37:36.406539+00');


--
-- Data for Name: dropped_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.dropped_column VALUES (1, 'x', true);
INSERT INTO public.dropped_column VALUES (2, 'y', false);


--
-- Data for Name: empty_table; Type: TABLE DATA; Schema: public; Owner: postgres
--



--
-- Data for Name: escapes; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.escapes VALUES (1, '');
INSERT INTO public.escapes VALUES (2, '');
INSERT INTO public.escapes VALUES (3, '');
INSERT INTO public.escapes VALUES (4, '');
INSERT INTO public.escapes VALUES (5, '');
INSERT INTO public.escapes VALUES (6, '');
INSERT INTO public.escapes VALUES (7, '');
INSERT INTO public.escapes VALUES (8, '');
INSERT INTO public.escapes VALUES (9, '	');
INSERT INTO public.escapes VALUES (10, '
');
INSERT INTO public.escapes VALUES (11, '');
INSERT INTO public.escapes VALUES (12, '');
INSERT INTO public.escapes VALUES (13, '
');
INSERT INTO public.escapes VALUES (14, '');
INSERT INTO public.escapes VALUES (15, '');
INSERT INTO public.escapes VALUES (16, '');
INSERT INTO public.escapes VALUES (17, '');
INSERT INTO public.escapes VALUES (18, '');
INSERT INTO public.escapes VALUES (19, '');
INSERT INTO public.escapes VALUES (20, '');
INSERT INTO public.escapes VALUES (21, '');
INSERT INTO public.escapes VALUES (22, '');
INSERT INTO public.escapes VALUES (23, '');
INSERT INTO public.escapes VALUES (24, '');
INSERT INTO public.escapes VALUES (25, '');
INSERT INTO public.escapes VALUES (26, '');
INSERT INTO public.escapes VALUES (27, '');
INSERT INTO public.escapes VALUES (28, '');
INSERT INTO public.escapes VALUES (29, '');
INSERT INTO public.escapes VALUES (30, '');
INSERT INTO public.escapes VALUES (31, '');
INSERT INTO public.escapes VALUES (32, ' ');
INSERT INTO public.escapes VALUES (33, '!');
INSERT INTO public.escapes VALUES (34, '"');
INSERT INTO public.escapes VALUES (35, '#');
INSERT INTO public.escapes VALUES (36, '$');
INSERT INTO public.escapes VALUES (37, '%');
INSERT INTO public.escapes VALUES (38, '&');
INSERT INTO public.escapes VALUES (39, '''');
INSERT INTO public.escapes VALUES (40, '(');
INSERT INTO public.escapes VALUES (41, ')');
INSERT INTO public.escapes VALUES (42, '*');
INSERT INTO public.escapes VALUES (43, '+');
INSERT INTO public.escapes VALUES (44, ',');
INSERT INTO public.escapes VALUES (45, '-');
INSERT INTO public.escapes VALUES (46, '.');
INSERT INTO public.escapes VALUES (47, '/');
INSERT INTO public.escapes VALUES (48, '0');
INSERT INTO public.escapes VALUES (49, '1');
INSERT INTO public.escapes VALUES (50, '2');
INSERT INTO public.escapes VALUES (51, '3');
INSERT INTO public.escapes VALUES (52, '4');
INSERT INTO public.escapes VALUES (53, '5');
INSERT INTO public.escapes VALUES (54, '6');
INSERT INTO public.escapes VALUES (55, '7');
INSERT INTO public.escapes VALUES (56, '8');
INSERT INTO public.escapes VALUES (57, '9');
INSERT INTO public.escapes VALUES (58, ':');
INSERT INTO public.escapes VALUES (59, ';');
INSERT INTO public.escapes VALUES (60, '<');
INSERT INTO public.escapes VALUES (61, '=');
INSERT INTO public.escapes VALUES (62, '>');
INSERT INTO public.escapes VALUES (63, '?');
INSERT INTO public.escapes VALUES (64, '@');
INSERT INTO public.escapes VALUES (65, 'A');
INSERT INTO public.escapes VALUES (66, 'B');
INSERT INTO public.escapes VALUES (67, 'C');
INSERT INTO public.escapes VALUES (68, 'D');
INSERT INTO public.escapes VALUES (69, 'E');
INSERT INTO public.escapes VALUES (70, 'F');
INSERT INTO public.escapes VALUES (71, 'G');
INSERT INTO public.escapes VALUES (72, 'H');
INSERT INTO public.escapes VALUES (73, 'I');
INSERT INTO public.escapes VALUES (74, 'J');
INSERT INTO public.escapes VALUES (75, 'K');
INSERT INTO public.escapes VALUES (76, 'L');
INSERT INTO public.escapes VALUES (77, 'M');
INSERT INTO public.escapes VALUES (78, 'N');
INSERT INTO public.escapes VALUES (79, 'O');
INSERT INTO public.escapes VALUES (80, 'P');
INSERT INTO public.escapes VALUES (81, 'Q');
INSERT INTO public.escapes VALUES (82, 'R');
INSERT INTO public.escapes VALUES (83, 'S');
INSERT INTO public.escapes VALUES (84, 'T');
INSERT INTO public.escapes VALUES (85, 'U');
INSERT INTO public.escapes VALUES (86, 'V');
INSERT INTO public.escapes VALUES (87, 'W');
INSERT INTO public.escapes VALUES (88, 'X');
INSERT INTO public.escapes VALUES (89, 'Y');
INSERT INTO public.escapes VALUES (90, 'Z');
INSERT INTO public.escapes VALUES (91, '[');
INSERT INTO public.escapes VALUES (92, '\');
INSERT INTO public.escapes VALUES (93, ']');
INSERT INTO public.escapes VALUES (94, '^');
INSERT INTO public.escapes VALUES (95, '_');
INSERT INTO public.escapes VALUES (96, '`');
INSERT INTO public.escapes VALUES (97, 'a');
INSERT INTO public.escapes VALUES (98, 'b');
INSERT INTO public.escapes VALUES (99, 'c');
INSERT INTO public.escapes VALUES (100, 'd');
INSERT INTO public.escapes VALUES (101, 'e');
INSERT INTO public.escapes VALUES (102, 'f');
INSERT INTO public.escapes VALUES (103, 'g');
INSERT INTO public.escapes VALUES (104, 'h');
INSERT INTO public.escapes VALUES (105, 'i');
INSERT INTO public.escapes VALUES (106, 'j');
INSERT INTO public.escapes VALUES (107, 'k');
INSERT INTO public.escapes VALUES (108, 'l');
INSERT INTO public.escapes VALUES (109, 'm');
INSERT INTO public.escapes VALUES (110, 'n');
INSERT INTO public.escapes VALUES (111, 'o');
INSERT INTO public.escapes VALUES (112, 'p');
INSERT INTO public.escapes VALUES (113, 'q');
INSERT INTO public.escapes VALUES (114, 'r');
INSERT INTO public.escapes VALUES (115, 's');
INSERT INTO public.escapes VALUES (116, 't');
INSERT INTO public.escapes VALUES (117, 'u');
INSERT INTO public.escapes VALUES (118, 'v');
INSERT INTO public.escapes VALUES (119, 'w');
INSERT INTO public.escapes VALUES (120, 'x');
INSERT INTO public.escapes VALUES (121, 'y');
INSERT INTO public.escapes VALUES (122, 'z');
INSERT INTO public.escapes VALUES (123, '{');
INSERT INTO public.escapes VALUES (124, '|');
INSERT INTO public.escapes VALUES (125, '}');
INSERT INTO public.escapes VALUES (126, '~');
INSERT INTO public.escapes VALUES (127, '');
INSERT INTO public.escapes VALUES (233, 'é');
INSERT INTO public.escapes VALUES (1071, 'Я');
INSERT INTO public.escapes VALUES (12354, 'あ');
INSERT INTO public.escapes VALUES (8364, '€');
INSERT INTO public.escapes VALUES (128169, '💩');


--
-- Data for Name: generated_column; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.generated_column VALUES (1, 2, 3, DEFAULT);
INSERT INTO public.generated_column VALUES (2, 10, -4, DEFAULT);


--
-- Data for Name: widgets; Type: TABLE DATA; Schema: public; Owner: postgres
--

INSERT INTO public.widgets VALUES (1, 'alpha', 'a simple widget', true, '2024-01-01 00:00:00+00');
INSERT INTO public.widgets VALUES (2, 'beta', NULL, false, '2024-01-02 00:00:00+00');
INSERT INTO public.widgets VALUES (3, 'gamma', 'multi
line	description with a literal backslash \ and a quote '' inside', true, NULL);
INSERT INTO public.widgets VALUES (4, 'delta', 'contains a COPY-like phrase: COPY public.widgets (id, name) TO stdout; -- not a real directive', true, '2024-01-04 00:00:00+00');
INSERT INTO public.widgets VALUES (5, '', 'empty name above', NULL, '2024-01-05 00:00:00+00');


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

\unrestrict 2mqEQ7Umn1NZ0d5zRpxAJf5z8YBkAiEK3WeBTbJLhCU4qUyPrvTv98s1kdNoUgj

