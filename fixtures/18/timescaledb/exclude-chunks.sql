--
-- PostgreSQL database dump
--

\restrict vRvqodWoI7Pmwwk1bxiRKDNnXbFfu6EZtnaLJ8rsvamz5cn9fVlkoYNmv0nHzC4

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

\unrestrict vRvqodWoI7Pmwwk1bxiRKDNnXbFfu6EZtnaLJ8rsvamz5cn9fVlkoYNmv0nHzC4

