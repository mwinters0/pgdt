--
-- PostgreSQL database dump
--

\restrict N59hjCb0lfhfb0WHVA7xYpT0OHsdDctq8YyK2IDlXd0q19NEuSHS1ckoiN5V6QD

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
-- Name: empty_table; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.empty_table (
    id integer NOT NULL,
    value text
);


ALTER TABLE public.empty_table OWNER TO postgres;

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

INSERT INTO logs.events (event_id, widget_id, message, logged_at) VALUES (100, 1, 'created', '2026-08-21 23:10:52.408108+00');
INSERT INTO logs.events (event_id, widget_id, message, logged_at) VALUES (101, 2, NULL, '2026-08-21 23:10:52.408108+00');
INSERT INTO logs.events (event_id, widget_id, message, logged_at) VALUES (102, 3, 'updated	with a tab char', '2026-08-21 23:10:52.408108+00');


--
-- Data for Name: empty_table; Type: TABLE DATA; Schema: public; Owner: postgres
--



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
-- Name: empty_table empty_table_pkey; Type: CONSTRAINT; Schema: public; Owner: postgres
--

ALTER TABLE ONLY public.empty_table
    ADD CONSTRAINT empty_table_pkey PRIMARY KEY (id);


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

\unrestrict N59hjCb0lfhfb0WHVA7xYpT0OHsdDctq8YyK2IDlXd0q19NEuSHS1ckoiN5V6QD

