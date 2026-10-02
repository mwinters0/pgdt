--
-- PostgreSQL database dump
--

\restrict GobAJIn6tWjLTWMoQ39d0vWdE13ftSJFGTcwM8M3VbN3UTqs5iwjuw9UIWodEpb

-- Dumped from database version 17.11 (Debian 17.11-1.pgdg13+2)
-- Dumped by pg_dump version 17.11 (Debian 17.11-1.pgdg13+2)

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

SET default_tablespace = '';

--
-- Name: events; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events (
    id bigint NOT NULL,
    event_date date NOT NULL,
    payload text
)
PARTITION BY RANGE (event_date);


ALTER TABLE objects.events OWNER TO postgres;

SET default_table_access_method = heap;

--
-- Name: events_2024; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events_2024 (
    id bigint NOT NULL,
    event_date date NOT NULL,
    payload text
);


ALTER TABLE objects.events_2024 OWNER TO postgres;

--
-- Name: events_2024; Type: TABLE ATTACH; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.events ATTACH PARTITION objects.events_2024 FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');


--
-- Data for Name: events_2024; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.events_2024 (id, event_date, payload) FROM stdin;
1	2024-06-01	first
2	2024-12-31	second
\.


--
-- Name: events_event_date_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX events_event_date_idx ON ONLY objects.events USING btree (event_date);


--
-- Name: events_2024_event_date_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX events_2024_event_date_idx ON objects.events_2024 USING btree (event_date);


--
-- Name: events_2024_event_date_idx; Type: INDEX ATTACH; Schema: objects; Owner: postgres
--

ALTER INDEX objects.events_event_date_idx ATTACH PARTITION objects.events_2024_event_date_idx;


--
-- Name: TABLE events; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.events TO fixture_reader;


--
-- Name: TABLE events_2024; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.events_2024 TO fixture_reader;


--
-- PostgreSQL database dump complete
--

\unrestrict GobAJIn6tWjLTWMoQ39d0vWdE13ftSJFGTcwM8M3VbN3UTqs5iwjuw9UIWodEpb

