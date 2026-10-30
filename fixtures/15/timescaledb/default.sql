--
-- PostgreSQL database dump
--

\restrict CJAfn0fb2KL3svFFkntIHyzcueOPywV8LOxWKbFTAN3clPyz5zmMcz1HBswK22n

-- Dumped from database version 15.14 (Ubuntu 15.14-1.pgdg22.04+1)
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
-- Name: timescaledb; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS timescaledb WITH SCHEMA public;


--
-- Name: EXTENSION timescaledb; Type: COMMENT; Schema: -; Owner: 
--

COMMENT ON EXTENSION timescaledb IS 'Enables scalable inserts and complex queries for time-series data (Community Edition)';


SET default_tablespace = '';

SET default_table_access_method = heap;

--
-- Name: _compressed_hypertable_4; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._compressed_hypertable_4 (
);


ALTER TABLE _timescaledb_internal._compressed_hypertable_4 OWNER TO postgres;

--
-- Name: _compressed_hypertable_7; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._compressed_hypertable_7 (
);


ALTER TABLE _timescaledb_internal._compressed_hypertable_7 OWNER TO postgres;

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
-- Name: _direct_view_2; Type: VIEW; Schema: _timescaledb_internal; Owner: postgres
--

CREATE VIEW _timescaledb_internal._direct_view_2 AS
 SELECT public.time_bucket('1 day'::interval, metrics."time") AS day,
    metrics.device,
    count(*) AS n,
    max(metrics.value) AS top
   FROM public.metrics
  GROUP BY (public.time_bucket('1 day'::interval, metrics."time")), metrics.device;


ALTER TABLE _timescaledb_internal._direct_view_2 OWNER TO postgres;

--
-- Name: _direct_view_3; Type: VIEW; Schema: _timescaledb_internal; Owner: postgres
--

CREATE VIEW _timescaledb_internal._direct_view_3 AS
 SELECT public.time_bucket('7 days'::interval, metrics."time") AS week,
    count(*) AS n
   FROM public.metrics
  GROUP BY (public.time_bucket('7 days'::interval, metrics."time"));


ALTER TABLE _timescaledb_internal._direct_view_3 OWNER TO postgres;

--
-- Name: _hyper_1_2_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_2_chunk (
    CONSTRAINT constraint_2 CHECK ((_timescaledb_functions.get_partition_hash(device) < 1073741823)),
    CONSTRAINT constraint_3 CHECK ((("time" >= '2024-01-04 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-11 00:00:00+00'::timestamp with time zone)))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_2_chunk OWNER TO postgres;

--
-- Name: _hyper_1_3_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_3_chunk (
    CONSTRAINT constraint_2 CHECK ((_timescaledb_functions.get_partition_hash(device) < 1073741823)),
    CONSTRAINT constraint_4 CHECK ((("time" >= '2024-01-11 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-18 00:00:00+00'::timestamp with time zone)))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_3_chunk OWNER TO postgres;

--
-- Name: _hyper_1_4_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_4_chunk (
    CONSTRAINT constraint_2 CHECK ((_timescaledb_functions.get_partition_hash(device) < 1073741823)),
    CONSTRAINT constraint_5 CHECK ((("time" >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-25 00:00:00+00'::timestamp with time zone)))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_4_chunk OWNER TO postgres;

--
-- Name: _hyper_1_6_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_6_chunk (
    CONSTRAINT constraint_3 CHECK ((("time" >= '2024-01-04 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-11 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_6 CHECK ((_timescaledb_functions.get_partition_hash(device) >= 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_6_chunk OWNER TO postgres;

--
-- Name: _hyper_1_7_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_7_chunk (
    CONSTRAINT constraint_4 CHECK ((("time" >= '2024-01-11 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-18 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_6 CHECK ((_timescaledb_functions.get_partition_hash(device) >= 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_7_chunk OWNER TO postgres;

--
-- Name: _hyper_1_8_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_8_chunk (
    CONSTRAINT constraint_5 CHECK ((("time" >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-25 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_6 CHECK ((_timescaledb_functions.get_partition_hash(device) >= 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_8_chunk OWNER TO postgres;

--
-- Name: _materialized_hypertable_2; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._materialized_hypertable_2 (
    day timestamp with time zone NOT NULL,
    device integer,
    n bigint,
    top double precision
);


ALTER TABLE _timescaledb_internal._materialized_hypertable_2 OWNER TO postgres;

--
-- Name: _hyper_2_10_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_2_10_chunk (
    CONSTRAINT constraint_8 CHECK (((day >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND (day < '2024-03-28 00:00:00+00'::timestamp with time zone)))
)
INHERITS (_timescaledb_internal._materialized_hypertable_2);


ALTER TABLE _timescaledb_internal._hyper_2_10_chunk OWNER TO postgres;

--
-- Name: _hyper_2_9_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_2_9_chunk (
    CONSTRAINT constraint_7 CHECK (((day >= '2023-11-09 00:00:00+00'::timestamp with time zone) AND (day < '2024-01-18 00:00:00+00'::timestamp with time zone)))
)
INHERITS (_timescaledb_internal._materialized_hypertable_2);


ALTER TABLE _timescaledb_internal._hyper_2_9_chunk OWNER TO postgres;

--
-- Name: _materialized_hypertable_3; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._materialized_hypertable_3 (
    week timestamp with time zone NOT NULL,
    n bigint
);


ALTER TABLE _timescaledb_internal._materialized_hypertable_3 OWNER TO postgres;

--
-- Name: _hyper_3_11_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_3_11_chunk (
    CONSTRAINT constraint_9 CHECK (((week >= '2023-11-09 00:00:00+00'::timestamp with time zone) AND (week < '2024-01-18 00:00:00+00'::timestamp with time zone)))
)
INHERITS (_timescaledb_internal._materialized_hypertable_3);


ALTER TABLE _timescaledb_internal._hyper_3_11_chunk OWNER TO postgres;

--
-- Name: _hyper_3_12_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_3_12_chunk (
    CONSTRAINT constraint_10 CHECK (((week >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND (week < '2024-03-28 00:00:00+00'::timestamp with time zone)))
)
INHERITS (_timescaledb_internal._materialized_hypertable_3);


ALTER TABLE _timescaledb_internal._hyper_3_12_chunk OWNER TO postgres;

--
-- Name: counters; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.counters (
    tick bigint NOT NULL,
    n integer
);


ALTER TABLE public.counters OWNER TO postgres;

--
-- Name: _hyper_5_17_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_5_17_chunk (
    CONSTRAINT constraint_11 CHECK (((tick >= '0'::bigint) AND (tick < '100'::bigint)))
)
INHERITS (public.counters);


ALTER TABLE _timescaledb_internal._hyper_5_17_chunk OWNER TO postgres;

--
-- Name: _hyper_5_18_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_5_18_chunk (
    CONSTRAINT constraint_12 CHECK (((tick >= '100'::bigint) AND (tick < '200'::bigint)))
)
INHERITS (public.counters);


ALTER TABLE _timescaledb_internal._hyper_5_18_chunk OWNER TO postgres;

--
-- Name: _hyper_5_19_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_5_19_chunk (
    CONSTRAINT constraint_13 CHECK (((tick >= '200'::bigint) AND (tick < '300'::bigint)))
)
INHERITS (public.counters);


ALTER TABLE _timescaledb_internal._hyper_5_19_chunk OWNER TO postgres;

--
-- Name: columnar; Type: TABLE; Schema: public; Owner: postgres
--

CREATE TABLE public.columnar (
    "time" timestamp with time zone NOT NULL,
    device integer NOT NULL,
    value double precision
);


ALTER TABLE public.columnar OWNER TO postgres;

SET default_table_access_method = hypercore;

--
-- Name: _hyper_6_20_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_6_20_chunk (
    CONSTRAINT constraint_14 CHECK ((("time" >= '2024-01-04 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-11 00:00:00+00'::timestamp with time zone)))
)
INHERITS (public.columnar);


ALTER TABLE _timescaledb_internal._hyper_6_20_chunk OWNER TO postgres;

--
-- Name: _partial_view_2; Type: VIEW; Schema: _timescaledb_internal; Owner: postgres
--

CREATE VIEW _timescaledb_internal._partial_view_2 AS
 SELECT public.time_bucket('1 day'::interval, metrics."time") AS day,
    metrics.device,
    count(*) AS n,
    max(metrics.value) AS top
   FROM public.metrics
  GROUP BY (public.time_bucket('1 day'::interval, metrics."time")), metrics.device;


ALTER TABLE _timescaledb_internal._partial_view_2 OWNER TO postgres;

--
-- Name: _partial_view_3; Type: VIEW; Schema: _timescaledb_internal; Owner: postgres
--

CREATE VIEW _timescaledb_internal._partial_view_3 AS
 SELECT public.time_bucket('7 days'::interval, metrics."time") AS week,
    count(*) AS n
   FROM public.metrics
  GROUP BY (public.time_bucket('7 days'::interval, metrics."time"));


ALTER TABLE _timescaledb_internal._partial_view_3 OWNER TO postgres;

SET default_table_access_method = heap;

--
-- Name: compress_hyper_4_13_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal.compress_hyper_4_13_chunk (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_13_chunk ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_13_chunk ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_13_chunk ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_13_chunk ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_13_chunk ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_13_chunk ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal.compress_hyper_4_13_chunk OWNER TO postgres;

--
-- Name: compress_hyper_4_14_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal.compress_hyper_4_14_chunk (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_14_chunk ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_14_chunk ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_14_chunk ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_14_chunk ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_14_chunk ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_14_chunk ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal.compress_hyper_4_14_chunk OWNER TO postgres;

--
-- Name: compress_hyper_4_15_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal.compress_hyper_4_15_chunk (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_15_chunk ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_15_chunk ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_15_chunk ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_15_chunk ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_15_chunk ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_15_chunk ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal.compress_hyper_4_15_chunk OWNER TO postgres;

--
-- Name: compress_hyper_4_16_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal.compress_hyper_4_16_chunk (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_16_chunk ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_16_chunk ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_16_chunk ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_16_chunk ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_16_chunk ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_4_16_chunk ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal.compress_hyper_4_16_chunk OWNER TO postgres;

--
-- Name: compress_hyper_7_21_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal.compress_hyper_7_21_chunk (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128', autovacuum_enabled='0', toast.autovacuum_enabled='0');
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_7_21_chunk ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_7_21_chunk ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_7_21_chunk ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_7_21_chunk ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_7_21_chunk ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal.compress_hyper_7_21_chunk ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal.compress_hyper_7_21_chunk OWNER TO postgres;

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
-- Data for Name: hypertable; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.hypertable (id, schema_name, table_name, associated_schema_name, associated_table_prefix, num_dimensions, chunk_sizing_func_schema, chunk_sizing_func_name, chunk_target_size, compression_state, compressed_hypertable_id, status) FROM stdin;
2	_timescaledb_internal	_materialized_hypertable_2	_timescaledb_internal	_hyper_2	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	0
3	_timescaledb_internal	_materialized_hypertable_3	_timescaledb_internal	_hyper_3	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	0
4	_timescaledb_internal	_compressed_hypertable_4	_timescaledb_internal	_hyper_4	0	_timescaledb_functions	calculate_chunk_interval	0	2	\N	0
1	public	metrics	_timescaledb_internal	_hyper_1	2	_timescaledb_functions	calculate_chunk_interval	0	1	4	0
5	public	counters	_timescaledb_internal	_hyper_5	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	0
7	_timescaledb_internal	_compressed_hypertable_7	_timescaledb_internal	_hyper_7	0	_timescaledb_functions	calculate_chunk_interval	0	2	\N	0
6	public	columnar	_timescaledb_internal	_hyper_6	1	_timescaledb_functions	calculate_chunk_interval	0	1	7	0
\.


--
-- Data for Name: chunk; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk (id, hypertable_id, schema_name, table_name, compressed_chunk_id, dropped, status, osm_chunk, creation_time) FROM stdin;
4	1	_timescaledb_internal	_hyper_1_4_chunk	\N	f	0	f	2026-10-30 17:11:26.010805+00
8	1	_timescaledb_internal	_hyper_1_8_chunk	\N	f	0	f	2026-10-30 17:11:26.035312+00
9	2	_timescaledb_internal	_hyper_2_9_chunk	\N	f	0	f	2026-10-30 17:11:26.066818+00
10	2	_timescaledb_internal	_hyper_2_10_chunk	\N	f	0	f	2026-10-30 17:11:26.072981+00
11	3	_timescaledb_internal	_hyper_3_11_chunk	\N	f	0	f	2026-10-30 17:11:26.089994+00
12	3	_timescaledb_internal	_hyper_3_12_chunk	\N	f	0	f	2026-10-30 17:11:26.093338+00
13	4	_timescaledb_internal	compress_hyper_4_13_chunk	\N	f	0	f	2026-10-30 17:11:26.107317+00
2	1	_timescaledb_internal	_hyper_1_2_chunk	13	f	1	f	2026-10-30 17:11:25.996608+00
14	4	_timescaledb_internal	compress_hyper_4_14_chunk	\N	f	0	f	2026-10-30 17:11:26.119419+00
15	4	_timescaledb_internal	compress_hyper_4_15_chunk	\N	f	0	f	2026-10-30 17:11:26.131901+00
6	1	_timescaledb_internal	_hyper_1_6_chunk	15	f	1	f	2026-10-30 17:11:26.023195+00
16	4	_timescaledb_internal	compress_hyper_4_16_chunk	\N	f	0	f	2026-10-30 17:11:26.143582+00
3	1	_timescaledb_internal	_hyper_1_3_chunk	14	f	9	f	2026-10-30 17:11:26.004613+00
7	1	_timescaledb_internal	_hyper_1_7_chunk	16	f	9	f	2026-10-30 17:11:26.029185+00
17	5	_timescaledb_internal	_hyper_5_17_chunk	\N	f	0	f	2026-10-30 17:11:26.175038+00
18	5	_timescaledb_internal	_hyper_5_18_chunk	\N	f	0	f	2026-10-30 17:11:26.178349+00
19	5	_timescaledb_internal	_hyper_5_19_chunk	\N	f	0	f	2026-10-30 17:11:26.181482+00
21	7	_timescaledb_internal	compress_hyper_7_21_chunk	\N	f	0	f	2026-10-30 17:11:26.268495+00
20	6	_timescaledb_internal	_hyper_6_20_chunk	21	f	1	f	2026-10-30 17:11:26.25865+00
\.


--
-- Data for Name: chunk_column_stats; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk_column_stats (id, hypertable_id, chunk_id, column_name, range_start, range_end, valid) FROM stdin;
\.


--
-- Data for Name: dimension; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.dimension (id, hypertable_id, column_name, column_type, aligned, num_slices, partitioning_func_schema, partitioning_func, interval_length, compress_interval_length, integer_now_func_schema, integer_now_func) FROM stdin;
1	1	time	timestamp with time zone	t	\N	\N	\N	604800000000	\N	\N	\N
2	1	device	integer	f	2	_timescaledb_functions	get_partition_hash	\N	\N	\N	\N
3	2	day	timestamp with time zone	t	\N	\N	\N	6048000000000	\N	\N	\N
4	3	week	timestamp with time zone	t	\N	\N	\N	6048000000000	\N	\N	\N
5	5	tick	bigint	t	\N	\N	\N	100	\N	\N	\N
6	6	time	timestamp with time zone	t	\N	\N	\N	604800000000	\N	\N	\N
\.


--
-- Data for Name: dimension_slice; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.dimension_slice (id, dimension_id, range_start, range_end) FROM stdin;
2	2	-9223372036854775808	1073741823
3	1	1704326400000000	1704931200000000
4	1	1704931200000000	1705536000000000
5	1	1705536000000000	1706140800000000
6	2	1073741823	9223372036854775807
7	3	1699488000000000	1705536000000000
8	3	1705536000000000	1711584000000000
9	4	1699488000000000	1705536000000000
10	4	1705536000000000	1711584000000000
11	5	0	100
12	5	100	200
13	5	200	300
14	6	1704326400000000	1704931200000000
\.


--
-- Data for Name: chunk_constraint; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk_constraint (chunk_id, dimension_slice_id, constraint_name, hypertable_constraint_name) FROM stdin;
2	3	constraint_3	\N
2	2	constraint_2	\N
3	4	constraint_4	\N
3	2	constraint_2	\N
4	5	constraint_5	\N
4	2	constraint_2	\N
6	3	constraint_3	\N
6	6	constraint_6	\N
7	4	constraint_4	\N
7	6	constraint_6	\N
8	5	constraint_5	\N
8	6	constraint_6	\N
9	7	constraint_7	\N
10	8	constraint_8	\N
11	9	constraint_9	\N
12	10	constraint_10	\N
17	11	constraint_11	\N
18	12	constraint_12	\N
19	13	constraint_13	\N
20	14	constraint_14	\N
\.


--
-- Data for Name: chunk_index; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk_index (chunk_id, index_name, hypertable_id, hypertable_index_name) FROM stdin;
2	_hyper_1_2_chunk_metrics_time_idx	1	metrics_time_idx
2	_hyper_1_2_chunk_metrics_device_time_idx	1	metrics_device_time_idx
3	_hyper_1_3_chunk_metrics_time_idx	1	metrics_time_idx
3	_hyper_1_3_chunk_metrics_device_time_idx	1	metrics_device_time_idx
4	_hyper_1_4_chunk_metrics_time_idx	1	metrics_time_idx
4	_hyper_1_4_chunk_metrics_device_time_idx	1	metrics_device_time_idx
6	_hyper_1_6_chunk_metrics_time_idx	1	metrics_time_idx
6	_hyper_1_6_chunk_metrics_device_time_idx	1	metrics_device_time_idx
7	_hyper_1_7_chunk_metrics_time_idx	1	metrics_time_idx
7	_hyper_1_7_chunk_metrics_device_time_idx	1	metrics_device_time_idx
8	_hyper_1_8_chunk_metrics_time_idx	1	metrics_time_idx
8	_hyper_1_8_chunk_metrics_device_time_idx	1	metrics_device_time_idx
9	_hyper_2_9_chunk__materialized_hypertable_2_day_idx	2	_materialized_hypertable_2_day_idx
9	_hyper_2_9_chunk__materialized_hypertable_2_device_day_idx	2	_materialized_hypertable_2_device_day_idx
10	_hyper_2_10_chunk__materialized_hypertable_2_day_idx	2	_materialized_hypertable_2_day_idx
10	_hyper_2_10_chunk__materialized_hypertable_2_device_day_idx	2	_materialized_hypertable_2_device_day_idx
11	_hyper_3_11_chunk__materialized_hypertable_3_week_idx	3	_materialized_hypertable_3_week_idx
12	_hyper_3_12_chunk__materialized_hypertable_3_week_idx	3	_materialized_hypertable_3_week_idx
17	_hyper_5_17_chunk_counters_tick_idx	5	counters_tick_idx
18	_hyper_5_18_chunk_counters_tick_idx	5	counters_tick_idx
19	_hyper_5_19_chunk_counters_tick_idx	5	counters_tick_idx
20	_hyper_6_20_chunk_columnar_time_idx	6	columnar_time_idx
\.


--
-- Data for Name: compression_chunk_size; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.compression_chunk_size (chunk_id, compressed_chunk_id, uncompressed_heap_size, uncompressed_toast_size, uncompressed_index_size, compressed_heap_size, compressed_toast_size, compressed_index_size, numrows_pre_compression, numrows_post_compression, numrows_frozen_immediately) FROM stdin;
2	13	8192	0	32768	16384	8192	16384	14	1	1
3	14	8192	0	32768	16384	8192	16384	14	1	1
6	15	8192	0	32768	16384	8192	16384	28	2	2
7	16	8192	0	32768	16384	8192	16384	28	2	2
20	21	8192	0	16384	16384	8192	16384	7	1	1
\.


--
-- Data for Name: compression_settings; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.compression_settings (relid, compress_relid, segmentby, orderby, orderby_desc, orderby_nullsfirst) FROM stdin;
public.metrics	\N	{device}	{time}	{f}	{f}
_timescaledb_internal._hyper_1_2_chunk	_timescaledb_internal.compress_hyper_4_13_chunk	{device}	{time}	{f}	{f}
_timescaledb_internal._hyper_1_3_chunk	_timescaledb_internal.compress_hyper_4_14_chunk	{device}	{time}	{f}	{f}
_timescaledb_internal._hyper_1_6_chunk	_timescaledb_internal.compress_hyper_4_15_chunk	{device}	{time}	{f}	{f}
_timescaledb_internal._hyper_1_7_chunk	_timescaledb_internal.compress_hyper_4_16_chunk	{device}	{time}	{f}	{f}
public.columnar	\N	{device}	{time}	{f}	{f}
_timescaledb_internal._hyper_6_20_chunk	_timescaledb_internal.compress_hyper_7_21_chunk	{device}	{time}	{f}	{f}
\.


--
-- Data for Name: continuous_agg; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_agg (mat_hypertable_id, raw_hypertable_id, parent_mat_hypertable_id, user_view_schema, user_view_name, partial_view_schema, partial_view_name, direct_view_schema, direct_view_name, materialized_only, finalized) FROM stdin;
2	1	\N	public	metrics_daily	_timescaledb_internal	_partial_view_2	_timescaledb_internal	_direct_view_2	t	t
3	1	\N	public	metrics_weekly	_timescaledb_internal	_partial_view_3	_timescaledb_internal	_direct_view_3	f	t
\.


--
-- Data for Name: continuous_agg_migrate_plan; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_agg_migrate_plan (mat_hypertable_id, start_ts, end_ts, user_view_definition) FROM stdin;
\.


--
-- Data for Name: continuous_agg_migrate_plan_step; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_agg_migrate_plan_step (mat_hypertable_id, step_id, status, start_ts, end_ts, type, config) FROM stdin;
\.


--
-- Data for Name: continuous_aggs_bucket_function; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_bucket_function (mat_hypertable_id, bucket_func, bucket_width, bucket_origin, bucket_offset, bucket_timezone, bucket_fixed_width) FROM stdin;
2	public.time_bucket(interval,timestamp with time zone)	1 day	\N	\N	\N	t
3	public.time_bucket(interval,timestamp with time zone)	7 days	\N	\N	\N	t
\.


--
-- Data for Name: continuous_aggs_hypertable_invalidation_log; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_hypertable_invalidation_log (hypertable_id, lowest_modified_value, greatest_modified_value) FROM stdin;
1	1703721600000000	1704326400000000
1	1703721600000000	1704326400000000
1	1705039200000000	1705039200000000
\.


--
-- Data for Name: continuous_aggs_invalidation_threshold; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_invalidation_threshold (hypertable_id, watermark) FROM stdin;
1	1706486400000000
\.


--
-- Data for Name: continuous_aggs_materialization_invalidation_log; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_materialization_invalidation_log (materialization_id, lowest_modified_value, greatest_modified_value) FROM stdin;
2	-9223372036854775808	-210866803200000001
2	1706140800000000	9223372036854775807
3	-9223372036854775808	-210866803200000001
3	1706486400000000	9223372036854775807
\.


--
-- Data for Name: continuous_aggs_watermark; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_watermark (mat_hypertable_id, watermark) FROM stdin;
2	1706140800000000
3	1706486400000000
\.


--
-- Data for Name: metadata; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.metadata (key, value, include_in_telemetry) FROM stdin;
install_timestamp	2026-10-30 17:11:24.238733+00	t
timescaledb_version	2.21.4	f
exported_uuid	a14a7464-8e62-4e01-b6b8-71be123c3285	t
\.


--
-- Data for Name: tablespace; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.tablespace (id, hypertable_id, tablespace_name) FROM stdin;
\.


--
-- Data for Name: bgw_job; Type: TABLE DATA; Schema: _timescaledb_config; Owner: postgres
--

COPY _timescaledb_config.bgw_job (id, application_name, schedule_interval, max_runtime, max_retries, retry_period, proc_schema, proc_name, owner, scheduled, fixed_schedule, initial_start, hypertable_id, config, check_schema, check_name, timezone) FROM stdin;
\.


--
-- Data for Name: _compressed_hypertable_4; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._compressed_hypertable_4  FROM stdin;
\.


--
-- Data for Name: _compressed_hypertable_7; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._compressed_hypertable_7  FROM stdin;
\.


--
-- Data for Name: _hyper_1_2_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_2_chunk ("time", device, value) FROM stdin;
\.


--
-- Data for Name: _hyper_1_3_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_3_chunk ("time", device, value) FROM stdin;
2024-01-12 06:00:00+00	1	-1
\.


--
-- Data for Name: _hyper_1_4_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_4_chunk ("time", device, value) FROM stdin;
2024-01-18 00:00:00+00	1	580
2024-01-18 12:00:00+00	1	592
2024-01-19 00:00:00+00	1	604
2024-01-19 12:00:00+00	1	616
2024-01-20 00:00:00+00	1	628
2024-01-20 12:00:00+00	1	640
2024-01-21 00:00:00+00	1	652
2024-01-21 12:00:00+00	1	664
2024-01-22 00:00:00+00	1	676
2024-01-22 12:00:00+00	1	688
2024-01-23 00:00:00+00	1	700
2024-01-23 12:00:00+00	1	712
2024-01-24 00:00:00+00	1	724
2024-01-24 12:00:00+00	1	736
\.


--
-- Data for Name: _hyper_1_6_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_6_chunk ("time", device, value) FROM stdin;
\.


--
-- Data for Name: _hyper_1_7_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_7_chunk ("time", device, value) FROM stdin;
2024-01-12 06:00:00+00	2	-2
\.


--
-- Data for Name: _hyper_1_8_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_8_chunk ("time", device, value) FROM stdin;
2024-01-18 00:00:00+00	2	680
2024-01-18 12:00:00+00	2	692
2024-01-19 00:00:00+00	2	704
2024-01-19 12:00:00+00	2	716
2024-01-20 00:00:00+00	2	728
2024-01-20 12:00:00+00	2	740
2024-01-21 00:00:00+00	2	752
2024-01-21 12:00:00+00	2	764
2024-01-22 00:00:00+00	2	776
2024-01-22 12:00:00+00	2	788
2024-01-23 00:00:00+00	2	800
2024-01-23 12:00:00+00	2	812
2024-01-24 00:00:00+00	2	824
2024-01-24 12:00:00+00	2	836
2024-01-18 00:00:00+00	3	780
2024-01-18 12:00:00+00	3	792
2024-01-19 00:00:00+00	3	804
2024-01-19 12:00:00+00	3	816
2024-01-20 00:00:00+00	3	828
2024-01-20 12:00:00+00	3	840
2024-01-21 00:00:00+00	3	852
2024-01-21 12:00:00+00	3	864
2024-01-22 00:00:00+00	3	876
2024-01-22 12:00:00+00	3	888
2024-01-23 00:00:00+00	3	900
2024-01-23 12:00:00+00	3	912
2024-01-24 00:00:00+00	3	924
2024-01-24 12:00:00+00	3	936
\.


--
-- Data for Name: _hyper_2_10_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_2_10_chunk (day, device, n, top) FROM stdin;
2024-01-23 00:00:00+00	2	2	812
2024-01-19 00:00:00+00	2	2	716
2024-01-22 00:00:00+00	3	2	888
2024-01-23 00:00:00+00	3	2	912
2024-01-18 00:00:00+00	2	2	692
2024-01-22 00:00:00+00	1	2	688
2024-01-21 00:00:00+00	2	2	764
2024-01-24 00:00:00+00	2	2	836
2024-01-23 00:00:00+00	1	2	712
2024-01-22 00:00:00+00	2	2	788
2024-01-18 00:00:00+00	3	2	792
2024-01-24 00:00:00+00	3	2	936
2024-01-21 00:00:00+00	1	2	664
2024-01-24 00:00:00+00	1	2	736
2024-01-20 00:00:00+00	2	2	740
2024-01-20 00:00:00+00	3	2	840
2024-01-21 00:00:00+00	3	2	864
2024-01-20 00:00:00+00	1	2	640
2024-01-19 00:00:00+00	1	2	616
2024-01-19 00:00:00+00	3	2	816
2024-01-18 00:00:00+00	1	2	592
\.


--
-- Data for Name: _hyper_2_9_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_2_9_chunk (day, device, n, top) FROM stdin;
2024-01-15 00:00:00+00	2	2	620
2024-01-08 00:00:00+00	2	2	452
2024-01-14 00:00:00+00	3	2	696
2024-01-04 00:00:00+00	1	2	256
2024-01-16 00:00:00+00	3	2	744
2024-01-13 00:00:00+00	1	2	472
2024-01-03 00:00:00+00	1	2	232
2024-01-17 00:00:00+00	3	2	768
2024-01-12 00:00:00+00	1	2	448
2024-01-16 00:00:00+00	1	2	544
2024-01-14 00:00:00+00	2	2	596
2023-12-29 00:00:00+00	3	2	312
2024-01-01 00:00:00+00	2	2	284
2024-01-03 00:00:00+00	2	2	332
2023-12-29 00:00:00+00	1	2	112
2024-01-02 00:00:00+00	2	2	308
2024-01-07 00:00:00+00	3	2	528
2024-01-07 00:00:00+00	2	2	428
2024-01-09 00:00:00+00	3	2	576
2024-01-08 00:00:00+00	3	2	552
2024-01-03 00:00:00+00	3	2	432
2024-01-14 00:00:00+00	1	2	496
2024-01-08 00:00:00+00	1	2	352
2024-01-12 00:00:00+00	3	2	648
2024-01-06 00:00:00+00	3	2	504
2024-01-02 00:00:00+00	3	2	408
2024-01-05 00:00:00+00	1	2	280
2024-01-16 00:00:00+00	2	2	644
2024-01-10 00:00:00+00	3	2	600
2024-01-12 00:00:00+00	2	2	548
2023-12-30 00:00:00+00	2	2	236
2023-12-31 00:00:00+00	3	2	360
2024-01-07 00:00:00+00	1	2	328
2023-12-31 00:00:00+00	1	2	160
2024-01-11 00:00:00+00	2	2	524
2023-12-30 00:00:00+00	1	2	136
2024-01-06 00:00:00+00	2	2	404
2023-12-29 00:00:00+00	2	2	212
2024-01-17 00:00:00+00	2	2	668
2024-01-17 00:00:00+00	1	2	568
2024-01-13 00:00:00+00	2	2	572
2024-01-15 00:00:00+00	3	2	720
2024-01-11 00:00:00+00	1	2	424
2023-12-31 00:00:00+00	2	2	260
2024-01-13 00:00:00+00	3	2	672
2024-01-02 00:00:00+00	1	2	208
2024-01-05 00:00:00+00	2	2	380
2024-01-09 00:00:00+00	2	2	476
2024-01-10 00:00:00+00	1	2	400
2024-01-11 00:00:00+00	3	2	624
2024-01-04 00:00:00+00	3	2	456
2024-01-04 00:00:00+00	2	2	356
2024-01-01 00:00:00+00	1	2	184
2024-01-01 00:00:00+00	3	2	384
2024-01-06 00:00:00+00	1	2	304
2024-01-10 00:00:00+00	2	2	500
2024-01-05 00:00:00+00	3	2	480
2023-12-30 00:00:00+00	3	2	336
2024-01-15 00:00:00+00	1	2	520
2024-01-09 00:00:00+00	1	2	376
\.


--
-- Data for Name: _hyper_3_11_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_3_11_chunk (week, n) FROM stdin;
2024-01-15 00:00:00+00	42
2023-12-25 00:00:00+00	18
2024-01-08 00:00:00+00	42
2024-01-01 00:00:00+00	42
\.


--
-- Data for Name: _hyper_3_12_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_3_12_chunk (week, n) FROM stdin;
2024-01-22 00:00:00+00	18
\.


--
-- Data for Name: _hyper_5_17_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_5_17_chunk (tick, n) FROM stdin;
0	0
10	1
20	2
30	3
40	4
50	5
60	6
70	7
80	8
90	9
\.


--
-- Data for Name: _hyper_5_18_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_5_18_chunk (tick, n) FROM stdin;
100	10
110	11
120	12
130	13
140	14
150	15
160	16
170	17
180	18
190	19
\.


--
-- Data for Name: _hyper_5_19_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_5_19_chunk (tick, n) FROM stdin;
200	20
210	21
220	22
230	23
240	24
250	25
260	26
270	27
280	28
290	29
\.


--
-- Data for Name: _hyper_6_20_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_6_20_chunk ("time", device, value) FROM stdin;
\.


--
-- Data for Name: _materialized_hypertable_2; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._materialized_hypertable_2 (day, device, n, top) FROM stdin;
\.


--
-- Data for Name: _materialized_hypertable_3; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._materialized_hypertable_3 (week, n) FROM stdin;
\.


--
-- Data for Name: compress_hyper_4_13_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal.compress_hyper_4_13_chunk (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, "time", value) FROM stdin;
14	1	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	BAAAArGU8GdQAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYiRc3sAAAAViED8HX/8AAAAAAAAAAA==	AwBAeQAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAAIUA481A849AsEAAAAAAAPMPQAAAA4AAAACAAAAAAAAADUBiIMohkGI0AAAAAAAAAAeAAAAAT4v99/e/f2A3Q==
\.


--
-- Data for Name: compress_hyper_4_14_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal.compress_hyper_4_14_chunk (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, "time", value) FROM stdin;
14	1	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	BAAAArIhwUrwAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYz3+pgAAAAVjKeDOn/8AAAAAAAAAAA==	AwBAgcAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAAIUAhA849A084EAAAAAAAQRPQAAAA4AAAACAAAAAAAAACUCDSIZBiKMkQAAAAAAAAAOAAAAAUC+//9+/fsB5w==
\.


--
-- Data for Name: compress_hyper_4_15_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal.compress_hyper_4_15_chunk (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, "time", value) FROM stdin;
14	2	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	BAAAArGU8GdQAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYiRc3sAAAAViED8HX/8AAAAAAAAAAA==	AwBAf0AAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAAIU809A48xA84EAAAAAAAPOQAAAAA4AAAACAAAAAAAAADUBDKMRBmEMkAAAAAAAAAAcAAAAATwPvf33+9+A6w==
14	3	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	BAAAArGU8GdQAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYiRc3sAAAAViED8HX/8AAAAAAAAAAA==	AwBAgsAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAAIUA9FAg9A480EAAAAAAAQROQAAAA4AAAACAAAAAAAAACUCjIIaxiIMsQAAAAAAAAAOAAAAAgP797/2+/sB7wAAAAAAAAAF
\.


--
-- Data for Name: compress_hyper_4_16_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal.compress_hyper_4_16_chunk (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, "time", value) FROM stdin;
14	2	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	BAAAArIhwUrwAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYz3+pgAAAAVjKeDOn/8AAAAAAAAAAA==	AwBAhOAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAAIUBE9A5FA9BEEAAAAAAARQNQAAAA4AAAABAAAAAAAAAAQAI2MkNSNDKAAAAAEyAAO/339794E=
14	3	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	BAAAArIhwUrwAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYz3+pgAAAAVjKeDOn/8AAAAAAAAAAA==	AwBAiAAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAAIUFA5BE9A1FAEAAAAAAAMQPQAAAA4AAAACAAAAAAAAADUCCGUYiDMIcgAAAAAAAAA7AAAAAgH37399/vYEGQAAAAAAAAAB
\.


--
-- Data for Name: compress_hyper_7_21_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal.compress_hyper_7_21_chunk (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, "time", value) FROM stdin;
7	1	2024-01-04 00:00:00+00	2024-01-10 00:00:00+00	BAAAArGK4XugAAAAABQd12AAAAAABwAAAAMAAAAAAAAB7gAFYiRc3sAAAAVh/CEv//8AAAAAAAAAAA==	AwBAJAAAAAAAAAAAAAcAAAABAAAAAAAAAAEAAAAAAAAAfwAAAAcAAAABAAAAAAAAAAEAAAAAAAAAfwAAAAEqAAAA04o0w0EAAAAHAAAAAQAAAAAAAAAEAAAAAAIUEhsAAAABFgAAAAAAP/wB
\.


--
-- Data for Name: columnar; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.columnar ("time", device, value) FROM stdin;
\.


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
-- Name: chunk_column_stats_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.chunk_column_stats_id_seq', 1, false);


--
-- Name: chunk_constraint_name; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.chunk_constraint_name', 1, false);


--
-- Name: chunk_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.chunk_id_seq', 21, true);


--
-- Name: continuous_agg_migrate_plan_step_step_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.continuous_agg_migrate_plan_step_step_id_seq', 1, false);


--
-- Name: dimension_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.dimension_id_seq', 6, true);


--
-- Name: dimension_slice_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.dimension_slice_id_seq', 14, true);


--
-- Name: hypertable_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.hypertable_id_seq', 7, true);


--
-- Name: bgw_job_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_config; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_config.bgw_job_id_seq', 1000, false);


--
-- Name: _hyper_1_2_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_2_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_2_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_2_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_2_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_2_chunk USING btree ("time" DESC);


--
-- Name: _hyper_1_3_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_3_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_3_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_3_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_3_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_3_chunk USING btree ("time" DESC);


--
-- Name: _hyper_1_4_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_4_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_4_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_4_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_4_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_4_chunk USING btree ("time" DESC);


--
-- Name: _hyper_1_6_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_6_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_6_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_6_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_6_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_6_chunk USING btree ("time" DESC);


--
-- Name: _hyper_1_7_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_7_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_7_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_7_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_7_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_7_chunk USING btree ("time" DESC);


--
-- Name: _hyper_1_8_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_8_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_8_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_8_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_8_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_8_chunk USING btree ("time" DESC);


--
-- Name: _hyper_2_10_chunk__materialized_hypertable_2_day_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_2_10_chunk__materialized_hypertable_2_day_idx ON _timescaledb_internal._hyper_2_10_chunk USING btree (day DESC);


--
-- Name: _hyper_2_10_chunk__materialized_hypertable_2_device_day_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_2_10_chunk__materialized_hypertable_2_device_day_idx ON _timescaledb_internal._hyper_2_10_chunk USING btree (device, day DESC);


--
-- Name: _hyper_2_9_chunk__materialized_hypertable_2_day_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_2_9_chunk__materialized_hypertable_2_day_idx ON _timescaledb_internal._hyper_2_9_chunk USING btree (day DESC);


--
-- Name: _hyper_2_9_chunk__materialized_hypertable_2_device_day_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_2_9_chunk__materialized_hypertable_2_device_day_idx ON _timescaledb_internal._hyper_2_9_chunk USING btree (device, day DESC);


--
-- Name: _hyper_3_11_chunk__materialized_hypertable_3_week_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_3_11_chunk__materialized_hypertable_3_week_idx ON _timescaledb_internal._hyper_3_11_chunk USING btree (week DESC);


--
-- Name: _hyper_3_12_chunk__materialized_hypertable_3_week_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_3_12_chunk__materialized_hypertable_3_week_idx ON _timescaledb_internal._hyper_3_12_chunk USING btree (week DESC);


--
-- Name: _hyper_5_17_chunk_counters_tick_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_5_17_chunk_counters_tick_idx ON _timescaledb_internal._hyper_5_17_chunk USING btree (tick DESC);


--
-- Name: _hyper_5_18_chunk_counters_tick_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_5_18_chunk_counters_tick_idx ON _timescaledb_internal._hyper_5_18_chunk USING btree (tick DESC);


--
-- Name: _hyper_5_19_chunk_counters_tick_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_5_19_chunk_counters_tick_idx ON _timescaledb_internal._hyper_5_19_chunk USING btree (tick DESC);


--
-- Name: _hyper_6_20_chunk_columnar_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_6_20_chunk_columnar_time_idx ON _timescaledb_internal._hyper_6_20_chunk USING btree ("time" DESC);


--
-- Name: _materialized_hypertable_2_day_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _materialized_hypertable_2_day_idx ON _timescaledb_internal._materialized_hypertable_2 USING btree (day DESC);


--
-- Name: _materialized_hypertable_2_device_day_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _materialized_hypertable_2_device_day_idx ON _timescaledb_internal._materialized_hypertable_2 USING btree (device, day DESC);


--
-- Name: _materialized_hypertable_3_week_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _materialized_hypertable_3_week_idx ON _timescaledb_internal._materialized_hypertable_3 USING btree (week DESC);


--
-- Name: compress_hyper_4_13_chunk_device__ts_meta_min_1__ts_meta_ma_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX compress_hyper_4_13_chunk_device__ts_meta_min_1__ts_meta_ma_idx ON _timescaledb_internal.compress_hyper_4_13_chunk USING btree (device, _ts_meta_min_1, _ts_meta_max_1);


--
-- Name: compress_hyper_4_14_chunk_device__ts_meta_min_1__ts_meta_ma_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX compress_hyper_4_14_chunk_device__ts_meta_min_1__ts_meta_ma_idx ON _timescaledb_internal.compress_hyper_4_14_chunk USING btree (device, _ts_meta_min_1, _ts_meta_max_1);


--
-- Name: compress_hyper_4_15_chunk_device__ts_meta_min_1__ts_meta_ma_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX compress_hyper_4_15_chunk_device__ts_meta_min_1__ts_meta_ma_idx ON _timescaledb_internal.compress_hyper_4_15_chunk USING btree (device, _ts_meta_min_1, _ts_meta_max_1);


--
-- Name: compress_hyper_4_16_chunk_device__ts_meta_min_1__ts_meta_ma_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX compress_hyper_4_16_chunk_device__ts_meta_min_1__ts_meta_ma_idx ON _timescaledb_internal.compress_hyper_4_16_chunk USING btree (device, _ts_meta_min_1, _ts_meta_max_1);


--
-- Name: compress_hyper_7_21_chunk_device__ts_meta_min_1__ts_meta_ma_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX compress_hyper_7_21_chunk_device__ts_meta_min_1__ts_meta_ma_idx ON _timescaledb_internal.compress_hyper_7_21_chunk USING btree (device, _ts_meta_min_1, _ts_meta_max_1);


--
-- Name: compress_hyper_7_21_chunk_ts_hypercore_proxy_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX compress_hyper_7_21_chunk_ts_hypercore_proxy_idx ON _timescaledb_internal.compress_hyper_7_21_chunk USING hypercore_proxy (_ts_meta_count);


--
-- Name: INDEX compress_hyper_7_21_chunk_ts_hypercore_proxy_idx; Type: COMMENT; Schema: _timescaledb_internal; Owner: postgres
--

COMMENT ON INDEX _timescaledb_internal.compress_hyper_7_21_chunk_ts_hypercore_proxy_idx IS 'Hypercore vacuum proxy index';


--
-- Name: columnar_time_idx; Type: INDEX; Schema: public; Owner: postgres
--

CREATE INDEX columnar_time_idx ON public.columnar USING btree ("time" DESC);


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
-- Name: _hyper_1_2_chunk ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON _timescaledb_internal._hyper_1_2_chunk FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: _hyper_1_3_chunk ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON _timescaledb_internal._hyper_1_3_chunk FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: _hyper_1_4_chunk ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON _timescaledb_internal._hyper_1_4_chunk FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: _hyper_1_6_chunk ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON _timescaledb_internal._hyper_1_6_chunk FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: _hyper_1_7_chunk ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON _timescaledb_internal._hyper_1_7_chunk FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: _hyper_1_8_chunk ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON _timescaledb_internal._hyper_1_8_chunk FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: _compressed_hypertable_4 ts_insert_blocker; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_insert_blocker BEFORE INSERT ON _timescaledb_internal._compressed_hypertable_4 FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.insert_blocker();


--
-- Name: _compressed_hypertable_7 ts_insert_blocker; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_insert_blocker BEFORE INSERT ON _timescaledb_internal._compressed_hypertable_7 FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.insert_blocker();


--
-- Name: _materialized_hypertable_2 ts_insert_blocker; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_insert_blocker BEFORE INSERT ON _timescaledb_internal._materialized_hypertable_2 FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.insert_blocker();


--
-- Name: _materialized_hypertable_3 ts_insert_blocker; Type: TRIGGER; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TRIGGER ts_insert_blocker BEFORE INSERT ON _timescaledb_internal._materialized_hypertable_3 FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.insert_blocker();


--
-- Name: metrics ts_cagg_invalidation_trigger; Type: TRIGGER; Schema: public; Owner: postgres
--

CREATE TRIGGER ts_cagg_invalidation_trigger AFTER INSERT OR DELETE OR UPDATE ON public.metrics FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.continuous_agg_invalidation_trigger('1');


--
-- Name: columnar ts_insert_blocker; Type: TRIGGER; Schema: public; Owner: postgres
--

CREATE TRIGGER ts_insert_blocker BEFORE INSERT ON public.columnar FOR EACH ROW EXECUTE FUNCTION _timescaledb_functions.insert_blocker();


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

\unrestrict CJAfn0fb2KL3svFFkntIHyzcueOPywV8LOxWKbFTAN3clPyz5zmMcz1HBswK22n

