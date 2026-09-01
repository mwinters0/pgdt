--
-- PostgreSQL database dump
--

\restrict mLZr2SDvLJqJh0gCzzosDW5H8N0G3BShkrP41PGjXEFyXunKk6fLtZm8lXYPv1I

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

\unrestrict mLZr2SDvLJqJh0gCzzosDW5H8N0G3BShkrP41PGjXEFyXunKk6fLtZm8lXYPv1I

