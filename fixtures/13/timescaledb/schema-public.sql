--
-- PostgreSQL database dump
--

\restrict jIl8vfBcoH7d8n8JB6ETaDvp0ybse6e53LSbdhE23lKzyk2TESqOdYaOLLVacLz

-- Dumped from database version 13.13 (Ubuntu 13.13-1.pgdg22.04+1)
-- Dumped by pg_dump version 13.23 (Debian 13.23-1.pgdg13+1)

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
-- Name: public; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA public;


ALTER SCHEMA public OWNER TO postgres;

--
-- Name: SCHEMA public; Type: COMMENT; Schema: -; Owner: postgres
--

COMMENT ON SCHEMA public IS 'standard public schema';


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: metrics; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.metrics (
    "time" timestamp with time zone NOT NULL,
    device integer NOT NULL,
    value double precision
);


ALTER TABLE public.metrics OWNER TO postgres;

--
-- Name: counters; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.counters (
    tick bigint NOT NULL,
    n integer
);


ALTER TABLE public.counters OWNER TO postgres;

--
-- Name: metrics_daily; Type: VIEW; Schema: public; Owner: postgres
--

CREATE VIEW public.metrics_daily AS
 SELECT _materialized_hypertable_2.day,
    _materialized_hypertable_2.device,
    _materialized_hypertable_2.n,
    _materialized_hypertable_2.top
   FROM _timescaledb_internal._materialized_hypertable_2;


ALTER TABLE public.metrics_daily OWNER TO postgres;

--
-- Name: metrics_weekly; Type: VIEW; Schema: public; Owner: postgres
--

CREATE VIEW public.metrics_weekly AS
 SELECT _materialized_hypertable_3.week,
    _materialized_hypertable_3.n
   FROM _timescaledb_internal._materialized_hypertable_3
  WHERE (_materialized_hypertable_3.week < COALESCE(_timescaledb_functions.to_timestamp(_timescaledb_functions.cagg_watermark(3)), '-infinity'::timestamp with time zone))
UNION ALL
 SELECT public.time_bucket('7 days'::interval, metrics."time") AS week,
    count(*) AS n
   FROM public.metrics
  WHERE (metrics."time" >= COALESCE(_timescaledb_functions.to_timestamp(_timescaledb_functions.cagg_watermark(3)), '-infinity'::timestamp with time zone))
  GROUP BY (public.time_bucket('7 days'::interval, metrics."time"));


ALTER TABLE public.metrics_weekly OWNER TO postgres;

--
-- Data for Name: counters; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.counters (tick, n) FROM stdin;
\.


--
-- Data for Name: metrics; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.metrics ("time", device, value) FROM stdin;
\.


--
-- Name: counters_tick_idx; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX counters_tick_idx ON public.counters USING btree (tick DESC);


--
-- Name: metrics_device_time_idx; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX metrics_device_time_idx ON public.metrics USING btree (device, "time" DESC);


--
-- Name: metrics_time_idx; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX metrics_time_idx ON public.metrics USING btree ("time" DESC);


--
-- Name: metrics ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: public; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON public.metrics FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: counters ts_insert_blocker; Type: TRIGGER; Schema: public; Owner: postgres
--

CREATE TRIGGER ts_insert_blocker BEFORE INSERT ON public.counters FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.insert_blocker();


--
-- Name: metrics ts_insert_blocker; Type: TRIGGER; Schema: public; Owner: postgres
--

CREATE TRIGGER ts_insert_blocker BEFORE INSERT ON public.metrics FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.insert_blocker();


--
-- PostgreSQL database dump complete
--

\unrestrict jIl8vfBcoH7d8n8JB6ETaDvp0ybse6e53LSbdhE23lKzyk2TESqOdYaOLLVacLz

