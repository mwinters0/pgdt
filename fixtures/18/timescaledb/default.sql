--
-- PostgreSQL database dump
--

\restrict LeAVcxpaUYonxufjU5g2FqO5M2RAU1WLUWyzM15465W5gegITZQ3gi4QEykUrUc

-- Dumped from database version 18.6 (Ubuntu 18.6-1.pgdg22.04+2)
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
 SELECT public.time_bucket('1 day'::interval, "time") AS day,
    device,
    count(*) AS n,
    max(value) AS top
   FROM public.metrics
  GROUP BY (public.time_bucket('1 day'::interval, "time")), device;


ALTER VIEW _timescaledb_internal._direct_view_2 OWNER TO postgres;

--
-- Name: _direct_view_3; Type: VIEW; Schema: _timescaledb_internal; Owner: postgres
--

CREATE VIEW _timescaledb_internal._direct_view_3 AS
 SELECT public.time_bucket('7 days'::interval, "time") AS week,
    count(*) AS n
   FROM public.metrics
  GROUP BY (public.time_bucket('7 days'::interval, "time"));


ALTER VIEW _timescaledb_internal._direct_view_3 OWNER TO postgres;

--
-- Name: _hyper_1_2_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_2_chunk (
    CONSTRAINT constraint_3 CHECK ((("time" >= '2024-01-04 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-11 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_4 CHECK ((_timescaledb_functions.get_partition_hash(device) < 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_2_chunk OWNER TO postgres;

--
-- Name: _hyper_1_2_chunk_compressed; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_2_chunk_compressed (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    _ts_meta_v2_first_time timestamp with time zone,
    _ts_meta_v2_last_time timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN _ts_meta_v2_first_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN _ts_meta_v2_last_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_2_chunk_compressed ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal._hyper_1_2_chunk_compressed OWNER TO postgres;

--
-- Name: _hyper_1_3_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_3_chunk (
    CONSTRAINT constraint_5 CHECK ((("time" >= '2024-01-11 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-18 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_6 CHECK ((_timescaledb_functions.get_partition_hash(device) < 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_3_chunk OWNER TO postgres;

--
-- Name: _hyper_1_3_chunk_compressed; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_3_chunk_compressed (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    _ts_meta_v2_first_time timestamp with time zone,
    _ts_meta_v2_last_time timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN _ts_meta_v2_first_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN _ts_meta_v2_last_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_3_chunk_compressed ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal._hyper_1_3_chunk_compressed OWNER TO postgres;

--
-- Name: _hyper_1_4_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_4_chunk (
    CONSTRAINT constraint_7 CHECK ((("time" >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-25 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_8 CHECK ((_timescaledb_functions.get_partition_hash(device) < 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_4_chunk OWNER TO postgres;

--
-- Name: _hyper_1_6_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_6_chunk (
    CONSTRAINT constraint_11 CHECK ((("time" >= '2024-01-04 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-11 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_12 CHECK ((_timescaledb_functions.get_partition_hash(device) >= 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_6_chunk OWNER TO postgres;

--
-- Name: _hyper_1_6_chunk_compressed; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_6_chunk_compressed (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    _ts_meta_v2_first_time timestamp with time zone,
    _ts_meta_v2_last_time timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN _ts_meta_v2_first_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN _ts_meta_v2_last_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_6_chunk_compressed ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal._hyper_1_6_chunk_compressed OWNER TO postgres;

--
-- Name: _hyper_1_7_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_7_chunk (
    CONSTRAINT constraint_13 CHECK ((("time" >= '2024-01-11 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-18 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_14 CHECK ((_timescaledb_functions.get_partition_hash(device) >= 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_7_chunk OWNER TO postgres;

--
-- Name: _hyper_1_7_chunk_compressed; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_7_chunk_compressed (
    _ts_meta_count integer,
    device integer,
    _ts_meta_min_1 timestamp with time zone,
    _ts_meta_max_1 timestamp with time zone,
    _ts_meta_v2_first_time timestamp with time zone,
    _ts_meta_v2_last_time timestamp with time zone,
    "time" _timescaledb_internal.compressed_data,
    value _timescaledb_internal.compressed_data
)
WITH (toast_tuple_target='128');
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN _ts_meta_count SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN device SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN _ts_meta_min_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN _ts_meta_max_1 SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN _ts_meta_v2_first_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN _ts_meta_v2_last_time SET STATISTICS 1000;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN "time" SET STATISTICS 0;
ALTER TABLE ONLY _timescaledb_internal._hyper_1_7_chunk_compressed ALTER COLUMN value SET STATISTICS 0;


ALTER TABLE _timescaledb_internal._hyper_1_7_chunk_compressed OWNER TO postgres;

--
-- Name: _hyper_1_8_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_1_8_chunk (
    CONSTRAINT constraint_15 CHECK ((("time" >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND ("time" < '2024-01-25 00:00:00+00'::timestamp with time zone))),
    CONSTRAINT constraint_16 CHECK ((_timescaledb_functions.get_partition_hash(device) >= 1073741823))
)
INHERITS (public.metrics);


ALTER TABLE _timescaledb_internal._hyper_1_8_chunk OWNER TO postgres;

--
-- Name: _materialized_hypertable_2; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._materialized_hypertable_2 (
    day timestamp with time zone,
    device integer,
    n bigint,
    top double precision
);


ALTER TABLE _timescaledb_internal._materialized_hypertable_2 OWNER TO postgres;

--
-- Name: _hyper_2_10_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_2_10_chunk (
    CONSTRAINT constraint_18 CHECK (((day >= '2023-11-09 00:00:00+00'::timestamp with time zone) AND (day < '2024-01-18 00:00:00+00'::timestamp with time zone)))
)
INHERITS (_timescaledb_internal._materialized_hypertable_2);


ALTER TABLE _timescaledb_internal._hyper_2_10_chunk OWNER TO postgres;

--
-- Name: _hyper_2_9_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_2_9_chunk (
    CONSTRAINT constraint_17 CHECK (((day >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND (day < '2024-03-28 00:00:00+00'::timestamp with time zone)))
)
INHERITS (_timescaledb_internal._materialized_hypertable_2);


ALTER TABLE _timescaledb_internal._hyper_2_9_chunk OWNER TO postgres;

--
-- Name: _materialized_hypertable_3; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._materialized_hypertable_3 (
    week timestamp with time zone,
    n bigint
);


ALTER TABLE _timescaledb_internal._materialized_hypertable_3 OWNER TO postgres;

--
-- Name: _hyper_3_11_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_3_11_chunk (
    CONSTRAINT constraint_19 CHECK (((week >= '2023-11-09 00:00:00+00'::timestamp with time zone) AND (week < '2024-01-18 00:00:00+00'::timestamp with time zone)))
)
INHERITS (_timescaledb_internal._materialized_hypertable_3);


ALTER TABLE _timescaledb_internal._hyper_3_11_chunk OWNER TO postgres;

--
-- Name: _hyper_3_12_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_3_12_chunk (
    CONSTRAINT constraint_20 CHECK (((week >= '2024-01-18 00:00:00+00'::timestamp with time zone) AND (week < '2024-03-28 00:00:00+00'::timestamp with time zone)))
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
-- Name: _hyper_4_13_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_4_13_chunk (
    CONSTRAINT constraint_21 CHECK (((tick >= '0'::bigint) AND (tick < '100'::bigint)))
)
INHERITS (public.counters);


ALTER TABLE _timescaledb_internal._hyper_4_13_chunk OWNER TO postgres;

--
-- Name: _hyper_4_14_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_4_14_chunk (
    CONSTRAINT constraint_22 CHECK (((tick >= '100'::bigint) AND (tick < '200'::bigint)))
)
INHERITS (public.counters);


ALTER TABLE _timescaledb_internal._hyper_4_14_chunk OWNER TO postgres;

--
-- Name: _hyper_4_15_chunk; Type: TABLE; Schema: _timescaledb_internal; Owner: postgres
--

CREATE TABLE _timescaledb_internal._hyper_4_15_chunk (
    CONSTRAINT constraint_23 CHECK (((tick >= '200'::bigint) AND (tick < '300'::bigint)))
)
INHERITS (public.counters);


ALTER TABLE _timescaledb_internal._hyper_4_15_chunk OWNER TO postgres;

--
-- Name: _partial_view_2; Type: VIEW; Schema: _timescaledb_internal; Owner: postgres
--

CREATE VIEW _timescaledb_internal._partial_view_2 AS
 SELECT public.time_bucket('1 day'::interval, "time") AS day,
    device,
    count(*) AS n,
    max(value) AS top
   FROM public.metrics
  GROUP BY (public.time_bucket('1 day'::interval, "time")), device;


ALTER VIEW _timescaledb_internal._partial_view_2 OWNER TO postgres;

--
-- Name: _partial_view_3; Type: VIEW; Schema: _timescaledb_internal; Owner: postgres
--

CREATE VIEW _timescaledb_internal._partial_view_3 AS
 SELECT public.time_bucket('7 days'::interval, "time") AS week,
    count(*) AS n
   FROM public.metrics
  GROUP BY (public.time_bucket('7 days'::interval, "time"));


ALTER VIEW _timescaledb_internal._partial_view_3 OWNER TO postgres;

--
-- Name: metrics_daily; Type: VIEW; Schema: public; Owner: postgres
--

CREATE VIEW public.metrics_daily AS
 SELECT day,
    device,
    n,
    top
   FROM _timescaledb_internal._materialized_hypertable_2;


ALTER VIEW public.metrics_daily OWNER TO postgres;

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


ALTER VIEW public.metrics_weekly OWNER TO postgres;

--
-- Data for Name: hypertable; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.hypertable (id, schema_name, table_name, associated_schema_name, associated_table_prefix, num_dimensions, chunk_sizing_func_schema, chunk_sizing_func_name, chunk_target_size, compression_state, compressed_hypertable_id, status) FROM stdin;
2	_timescaledb_internal	_materialized_hypertable_2	_timescaledb_internal	_hyper_2	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	0
3	_timescaledb_internal	_materialized_hypertable_3	_timescaledb_internal	_hyper_3	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	0
1	public	metrics	_timescaledb_internal	_hyper_1	2	_timescaledb_functions	calculate_chunk_interval	0	0	\N	4
4	public	counters	_timescaledb_internal	_hyper_4	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	0
\.


--
-- Data for Name: bgw_job; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.bgw_job (id, application_name, schedule_interval, max_runtime, max_retries, retry_period, proc_schema, proc_name, owner, scheduled, fixed_schedule, initial_start, hypertable_id, config, check_schema, check_name, timezone) FROM stdin;
\.


--
-- Data for Name: chunk; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk (id, relid, hypertable_id, status, osm_chunk, creation_time) FROM stdin;
4	_timescaledb_internal._hyper_1_4_chunk	1	0	f	2026-10-30 17:12:04.000877+00
8	_timescaledb_internal._hyper_1_8_chunk	1	0	f	2026-10-30 17:12:04.00404+00
9	_timescaledb_internal._hyper_2_9_chunk	2	0	f	2026-10-30 17:12:04.029129+00
10	_timescaledb_internal._hyper_2_10_chunk	2	0	f	2026-10-30 17:12:04.029867+00
11	_timescaledb_internal._hyper_3_11_chunk	3	0	f	2026-10-30 17:12:04.065882+00
12	_timescaledb_internal._hyper_3_12_chunk	3	0	f	2026-10-30 17:12:04.066386+00
2	_timescaledb_internal._hyper_1_2_chunk	1	1	f	2026-10-30 17:12:03.999386+00
6	_timescaledb_internal._hyper_1_6_chunk	1	1	f	2026-10-30 17:12:04.002415+00
3	_timescaledb_internal._hyper_1_3_chunk	1	9	f	2026-10-30 17:12:04.000128+00
7	_timescaledb_internal._hyper_1_7_chunk	1	9	f	2026-10-30 17:12:04.003197+00
13	_timescaledb_internal._hyper_4_13_chunk	4	0	f	2026-10-30 17:12:04.094681+00
14	_timescaledb_internal._hyper_4_14_chunk	4	0	f	2026-10-30 17:12:04.095239+00
15	_timescaledb_internal._hyper_4_15_chunk	4	0	f	2026-10-30 17:12:04.095742+00
\.


--
-- Data for Name: chunk_column_stats; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk_column_stats (id, hypertable_id, chunk_id, column_name, range_start, range_end, valid) FROM stdin;
\.


--
-- Data for Name: compression_chunk_size; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.compression_chunk_size (chunk_id, compressed_chunk_id, uncompressed_heap_size, uncompressed_toast_size, uncompressed_index_size, compressed_heap_size, compressed_toast_size, compressed_index_size, numrows_pre_compression, numrows_post_compression, numrows_frozen_immediately) FROM stdin;
2	0	8192	0	32768	16384	8192	16384	14	1	1
3	0	8192	0	32768	16384	8192	16384	14	1	1
6	0	8192	0	32768	16384	8192	16384	28	2	2
7	0	8192	0	32768	16384	8192	16384	28	2	2
\.


--
-- Data for Name: compression_settings; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.compression_settings (relid, compress_relid, segmentby, orderby, orderby_desc, orderby_nullsfirst, index) FROM stdin;
public.metrics	\N	{device}	{time}	{f}	{f}	[{"type": "minmax", "column": "time", "source": "orderby"}, {"type": "firstlast", "column": "time", "source": "orderby"}]
_timescaledb_internal._hyper_1_2_chunk	_timescaledb_internal._hyper_1_2_chunk_compressed	{device}	{time}	{f}	{f}	[{"type": "minmax", "column": "time", "source": "orderby"}, {"type": "firstlast", "column": "time", "source": "orderby"}]
_timescaledb_internal._hyper_1_3_chunk	_timescaledb_internal._hyper_1_3_chunk_compressed	{device}	{time}	{f}	{f}	[{"type": "minmax", "column": "time", "source": "orderby"}, {"type": "firstlast", "column": "time", "source": "orderby"}]
_timescaledb_internal._hyper_1_6_chunk	_timescaledb_internal._hyper_1_6_chunk_compressed	{device}	{time}	{f}	{f}	[{"type": "minmax", "column": "time", "source": "orderby"}, {"type": "firstlast", "column": "time", "source": "orderby"}]
_timescaledb_internal._hyper_1_7_chunk	_timescaledb_internal._hyper_1_7_chunk_compressed	{device}	{time}	{f}	{f}	[{"type": "minmax", "column": "time", "source": "orderby"}, {"type": "firstlast", "column": "time", "source": "orderby"}]
\.


--
-- Data for Name: continuous_agg; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_agg (mat_hypertable_id, raw_hypertable_id, parent_mat_hypertable_id, user_view_schema, user_view_name, partial_view_schema, partial_view_name, direct_view_schema, direct_view_name, materialized_only, schema_change_timestamp, granular_refresh_enabled) FROM stdin;
2	1	\N	public	metrics_daily	_timescaledb_internal	_partial_view_2	_timescaledb_internal	_direct_view_2	t	\N	f
3	1	\N	public	metrics_weekly	_timescaledb_internal	_partial_view_3	_timescaledb_internal	_direct_view_3	f	\N	f
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

COPY _timescaledb_catalog.continuous_aggs_hypertable_invalidation_log (hypertable_id, lowest_modified_value, greatest_modified_value, seqnum) FROM stdin;
1	1703721600000000	1704326400000000	\N
1	1703721600000000	1704326400000000	\N
1	1705039200000000	1705039200000000	\N
1	1705039200000000	1705039200000000	\N
\.


--
-- Data for Name: continuous_aggs_invalidation_threshold; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_invalidation_threshold (hypertable_id, watermark) FROM stdin;
1	1706486400000000
\.


--
-- Data for Name: continuous_aggs_jobs_refresh_ranges; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_jobs_refresh_ranges (materialization_id, start_range, end_range, pid, job_id, created_at) FROM stdin;
\.


--
-- Data for Name: continuous_aggs_materialization_invalidation_log; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_materialization_invalidation_log (materialization_id, lowest_modified_value, greatest_modified_value, seqnum) FROM stdin;
2	1706140800000000	9223372036854775807	\N
2	-9223372036854775808	-210866803200000001	\N
3	-9223372036854775808	-210866803200000001	\N
3	1706486400000000	9223372036854775807	\N
\.


--
-- Data for Name: continuous_aggs_materialization_ranges; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_materialization_ranges (materialization_id, lowest_modified_value, greatest_modified_value) FROM stdin;
\.


--
-- Data for Name: continuous_aggs_tenant_tracking; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_tenant_tracking (hypertable_id, tenant_id, min_timestamp, max_timestamp, seqnum) FROM stdin;
\.


--
-- Data for Name: continuous_aggs_watermark; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_aggs_watermark (mat_hypertable_id, watermark) FROM stdin;
2	1706140800000000
3	1706486400000000
\.


--
-- Data for Name: dimension; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.dimension (id, hypertable_id, column_name, column_type, aligned, num_slices, partitioning_func_schema, partitioning_func, interval_length, compress_interval_length, integer_now_func_schema, integer_now_func) FROM stdin;
1	1	time	timestamp with time zone	t	\N	\N	\N	604800000000	\N	\N	\N
2	1	device	integer	f	2	_timescaledb_functions	get_partition_hash	\N	\N	\N	\N
3	2	day	timestamp with time zone	t	\N	\N	\N	6048000000000	\N	\N	\N
4	3	week	timestamp with time zone	t	\N	\N	\N	6048000000000	\N	\N	\N
5	4	tick	bigint	t	\N	\N	\N	100	\N	\N	\N
\.


--
-- Data for Name: dimension_slice; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.dimension_slice (id, chunk_id, dimension_id, range_start, range_end) FROM stdin;
3	2	1	1704326400000000	1704931200000000
4	2	2	-9223372036854775808	1073741823
5	3	1	1704931200000000	1705536000000000
6	3	2	-9223372036854775808	1073741823
7	4	1	1705536000000000	1706140800000000
8	4	2	-9223372036854775808	1073741823
11	6	1	1704326400000000	1704931200000000
12	6	2	1073741823	9223372036854775807
13	7	1	1704931200000000	1705536000000000
14	7	2	1073741823	9223372036854775807
15	8	1	1705536000000000	1706140800000000
16	8	2	1073741823	9223372036854775807
17	9	3	1705536000000000	1711584000000000
18	10	3	1699488000000000	1705536000000000
19	11	4	1699488000000000	1705536000000000
20	12	4	1705536000000000	1711584000000000
21	13	5	0	100
22	14	5	100	200
23	15	5	200	300
\.


--
-- Data for Name: hypertable_cagg_settings; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.hypertable_cagg_settings (hypertable_id, granular_refresh_column, granular_refresh_start_offset, granular_refresh_end_offset) FROM stdin;
\.


--
-- Data for Name: metadata; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.metadata (key, value, include_in_telemetry) FROM stdin;
install_timestamp	2026-10-30 17:12:02.137184+00	t
timescaledb_version	2.31.0	f
exported_uuid	dfe7ff30-ec07-466a-9f08-e25ad07102be	t
\.


--
-- Data for Name: tablespace; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.tablespace (id, hypertable_id, tablespace_name) FROM stdin;
\.


--
-- Data for Name: _hyper_1_2_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_2_chunk ("time", device, value) FROM stdin;
\.


--
-- Data for Name: _hyper_1_2_chunk_compressed; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_2_chunk_compressed (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, _ts_meta_v2_first_time, _ts_meta_v2_last_time, "time", value) FROM stdin;
14	1	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	BAAAArGU8GdQAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYiRc3sAAAAViED8HX/8AAAAAAAAAAA==	AwBAeQAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAAQnQAAAAEkAAAAAw049AEAAAAGAAAAAQAAAAAAAAAFAAAAAAxSDFAAAAACFmn6a/cAPYDdAAAAAAAF/KM=
\.


--
-- Data for Name: _hyper_1_3_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_3_chunk ("time", device, value) FROM stdin;
2024-01-12 06:00:00+00	1	-1
\.


--
-- Data for Name: _hyper_1_3_chunk_compressed; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_3_chunk_compressed (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, _ts_meta_v2_first_time, _ts_meta_v2_last_time, "time", value) FROM stdin;
14	1	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	BAAAArIhwUrwAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYz3+pgAAAAVjKeDOn/8AAAAAAAAAAA==	AwBAgcAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAAOCwAAAAEkAAAAA9Ag04EAAAAGAAAAAQAAAAAAAAAFAAAAAAg0lJEAAAACD/+Mryj6+wHnAAAAAAAAKe8=
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
-- Data for Name: _hyper_1_6_chunk_compressed; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_6_chunk_compressed (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, _ts_meta_v2_first_time, _ts_meta_v2_last_time, "time", value) FROM stdin;
14	2	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	BAAAArGU8GdQAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYiRc3sAAAAViED8HX/8AAAAAAAAAAA==	AwBAf0AAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAAAEwAAAAESAAAAAAAAw4EAAAADAAAAAQAAAAAAAAAFAAAAAAAAGJAAAAACGHxQzR/TX4DrAAAAAAAc0MU=
14	3	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	2024-01-04 00:00:00+00	2024-01-10 12:00:00+00	BAAAArGU8GdQAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYiRc3sAAAAViED8HX/8AAAAAAAAAAA==	AwBAgsAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAAATQAAAAEYAAAAAAAg48EAAAAEAAAAAQAAAAAAAAAFAAAAAAAFkHEAAAADCV/6p7wAOwHvQBgdAOA0AYAAAAAAAAAAAQ==
\.


--
-- Data for Name: _hyper_1_7_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_7_chunk ("time", device, value) FROM stdin;
2024-01-12 06:00:00+00	2	-2
\.


--
-- Data for Name: _hyper_1_7_chunk_compressed; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_1_7_chunk_compressed (_ts_meta_count, device, _ts_meta_min_1, _ts_meta_max_1, _ts_meta_v2_first_time, _ts_meta_v2_last_time, "time", value) FROM stdin;
14	2	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	BAAAArIhwUrwAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYz3+pgAAAAVjKeDOn/8AAAAAAAAAAA==	AwBAhOAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAAITwAAAAEkAAAAA049BEEAAAAGAAAAAQAAAAAAAAAEAAAAAABlQygAAAACBMX8o2n6a/eBAAAAAAAAAAA=
14	3	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	2024-01-11 00:00:00+00	2024-01-17 12:00:00+00	BAAAArIhwUrwAAAAAAoO67AAAAAADgAAAAMAAAAAAAAB7gAFYz3+pgAAAAVjKeDOn/8AAAAAAAAAAA==	AwBAiAAAAAAAAAAAAA4AAAABAAAAAAAAAAEAAAAAAAA//wAAAA4AAAABAAAAAAAAAAEAAAAAAAAgCwAAAAEYAAAAAAAw1AEAAAAEAAAAAQAAAAAAAAAFAAAAAAADmHIAAAACG1fFDNH9dgQZAAAAAAfRzQw=
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
2024-01-17 00:00:00+00	2	2	668
2024-01-17 00:00:00+00	1	2	568
2024-01-17 00:00:00+00	3	2	768
2024-01-16 00:00:00+00	3	2	744
2024-01-13 00:00:00+00	1	2	472
2024-01-15 00:00:00+00	3	2	720
2024-01-13 00:00:00+00	2	2	572
2024-01-11 00:00:00+00	1	2	424
2024-01-12 00:00:00+00	1	2	448
2024-01-16 00:00:00+00	1	2	544
2024-01-14 00:00:00+00	2	2	596
2024-01-13 00:00:00+00	3	2	672
2024-01-07 00:00:00+00	3	2	528
2024-01-07 00:00:00+00	2	2	428
2024-01-09 00:00:00+00	2	2	476
2024-01-10 00:00:00+00	1	2	400
2024-01-11 00:00:00+00	3	2	624
2024-01-09 00:00:00+00	3	2	576
2024-01-08 00:00:00+00	3	2	552
2024-01-14 00:00:00+00	1	2	496
2024-01-08 00:00:00+00	1	2	352
2024-01-12 00:00:00+00	3	2	648
2024-01-16 00:00:00+00	2	2	644
2024-01-10 00:00:00+00	3	2	600
2024-01-12 00:00:00+00	2	2	548
2024-01-07 00:00:00+00	1	2	328
2024-01-10 00:00:00+00	2	2	500
2024-01-11 00:00:00+00	2	2	524
2024-01-15 00:00:00+00	1	2	520
2024-01-09 00:00:00+00	1	2	376
2024-01-15 00:00:00+00	2	2	620
2024-01-08 00:00:00+00	2	2	452
2024-01-14 00:00:00+00	3	2	696
2024-01-03 00:00:00+00	3	2	432
2024-01-04 00:00:00+00	1	2	256
2024-01-04 00:00:00+00	3	2	456
2024-01-06 00:00:00+00	3	2	504
2024-01-02 00:00:00+00	3	2	408
2024-01-05 00:00:00+00	1	2	280
2024-01-03 00:00:00+00	1	2	232
2024-01-04 00:00:00+00	2	2	356
2023-12-30 00:00:00+00	2	2	236
2024-01-01 00:00:00+00	1	2	184
2024-01-01 00:00:00+00	3	2	384
2023-12-29 00:00:00+00	3	2	312
2024-01-06 00:00:00+00	1	2	304
2023-12-31 00:00:00+00	2	2	260
2024-01-01 00:00:00+00	2	2	284
2023-12-31 00:00:00+00	3	2	360
2024-01-03 00:00:00+00	2	2	332
2023-12-29 00:00:00+00	1	2	112
2023-12-31 00:00:00+00	1	2	160
2024-01-02 00:00:00+00	2	2	308
2024-01-05 00:00:00+00	3	2	480
2023-12-30 00:00:00+00	3	2	336
2024-01-02 00:00:00+00	1	2	208
2024-01-05 00:00:00+00	2	2	380
2023-12-30 00:00:00+00	1	2	136
2024-01-06 00:00:00+00	2	2	404
2023-12-29 00:00:00+00	2	2	212
\.


--
-- Data for Name: _hyper_2_9_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_2_9_chunk (day, device, n, top) FROM stdin;
2024-01-20 00:00:00+00	3	2	840
2024-01-21 00:00:00+00	1	2	664
2024-01-21 00:00:00+00	3	2	864
2024-01-22 00:00:00+00	1	2	688
2024-01-23 00:00:00+00	2	2	812
2024-01-19 00:00:00+00	2	2	716
2024-01-21 00:00:00+00	2	2	764
2024-01-20 00:00:00+00	1	2	640
2024-01-22 00:00:00+00	3	2	888
2024-01-23 00:00:00+00	3	2	912
2024-01-18 00:00:00+00	2	2	692
2024-01-24 00:00:00+00	1	2	736
2024-01-24 00:00:00+00	2	2	836
2024-01-19 00:00:00+00	1	2	616
2024-01-19 00:00:00+00	3	2	816
2024-01-23 00:00:00+00	1	2	712
2024-01-22 00:00:00+00	2	2	788
2024-01-18 00:00:00+00	1	2	592
2024-01-18 00:00:00+00	3	2	792
2024-01-20 00:00:00+00	2	2	740
2024-01-24 00:00:00+00	3	2	936
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
-- Data for Name: _hyper_4_13_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_4_13_chunk (tick, n) FROM stdin;
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
-- Data for Name: _hyper_4_14_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_4_14_chunk (tick, n) FROM stdin;
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
-- Data for Name: _hyper_4_15_chunk; Type: TABLE DATA; Schema: _timescaledb_internal; Owner: postgres
--

COPY _timescaledb_internal._hyper_4_15_chunk (tick, n) FROM stdin;
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
-- Data for Name: counters; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.counters (tick, n) FROM stdin;
\.


--
-- Data for Name: metrics; Type: TABLE DATA; Schema: public; Owner: postgres
--

COPY public.metrics ("time", device, value) FROM stdin;
2024-01-20 00:00:00+00	9	900
\.


--
-- Name: bgw_job_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.bgw_job_id_seq', 1000, false);


--
-- Name: chunk_column_stats_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.chunk_column_stats_id_seq', 1, false);


--
-- Name: chunk_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.chunk_id_seq', 15, true);


--
-- Name: dimension_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.dimension_id_seq', 5, true);


--
-- Name: dimension_slice_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.dimension_slice_id_seq', 23, true);


--
-- Name: hypertable_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.hypertable_id_seq', 4, true);


--
-- Name: _hyper_1_2_chunk_compressed_device__ts_meta_v2_first_time___idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_2_chunk_compressed_device__ts_meta_v2_first_time___idx ON _timescaledb_internal._hyper_1_2_chunk_compressed USING btree (device, _ts_meta_v2_first_time, _ts_meta_v2_last_time);


--
-- Name: _hyper_1_2_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_2_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_2_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_2_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_2_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_2_chunk USING btree ("time" DESC);


--
-- Name: _hyper_1_3_chunk_compressed_device__ts_meta_v2_first_time___idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_3_chunk_compressed_device__ts_meta_v2_first_time___idx ON _timescaledb_internal._hyper_1_3_chunk_compressed USING btree (device, _ts_meta_v2_first_time, _ts_meta_v2_last_time);


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
-- Name: _hyper_1_6_chunk_compressed_device__ts_meta_v2_first_time___idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_6_chunk_compressed_device__ts_meta_v2_first_time___idx ON _timescaledb_internal._hyper_1_6_chunk_compressed USING btree (device, _ts_meta_v2_first_time, _ts_meta_v2_last_time);


--
-- Name: _hyper_1_6_chunk_metrics_device_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_6_chunk_metrics_device_time_idx ON _timescaledb_internal._hyper_1_6_chunk USING btree (device, "time" DESC);


--
-- Name: _hyper_1_6_chunk_metrics_time_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_6_chunk_metrics_time_idx ON _timescaledb_internal._hyper_1_6_chunk USING btree ("time" DESC);


--
-- Name: _hyper_1_7_chunk_compressed_device__ts_meta_v2_first_time___idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_1_7_chunk_compressed_device__ts_meta_v2_first_time___idx ON _timescaledb_internal._hyper_1_7_chunk_compressed USING btree (device, _ts_meta_v2_first_time, _ts_meta_v2_last_time);


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
-- Name: _hyper_4_13_chunk_counters_tick_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_4_13_chunk_counters_tick_idx ON _timescaledb_internal._hyper_4_13_chunk USING btree (tick DESC);


--
-- Name: _hyper_4_14_chunk_counters_tick_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_4_14_chunk_counters_tick_idx ON _timescaledb_internal._hyper_4_14_chunk USING btree (tick DESC);


--
-- Name: _hyper_4_15_chunk_counters_tick_idx; Type: INDEX; Schema: _timescaledb_internal; Owner: postgres
--

CREATE INDEX _hyper_4_15_chunk_counters_tick_idx ON _timescaledb_internal._hyper_4_15_chunk USING btree (tick DESC);


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
-- PostgreSQL database dump complete
--

\unrestrict LeAVcxpaUYonxufjU5g2FqO5M2RAU1WLUWyzM15465W5gegITZQ3gi4QEykUrUc

