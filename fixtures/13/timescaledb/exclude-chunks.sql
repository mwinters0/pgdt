--
-- PostgreSQL database dump
--

\restrict rjL0xfTngC040vSn5PF056KeKiTuevhGwaTZMaFrPv1nQC9vLQhZzrQnvuKjYzp

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
-- Data for Name: cache_inval_bgw_job; Type: TABLE DATA; Schema: _timescaledb_cache; Owner: postgres
--

COPY _timescaledb_cache.cache_inval_bgw_job  FROM stdin;
\.


--
-- Data for Name: cache_inval_extension; Type: TABLE DATA; Schema: _timescaledb_cache; Owner: postgres
--

COPY _timescaledb_cache.cache_inval_extension  FROM stdin;
\.


--
-- Data for Name: cache_inval_hypertable; Type: TABLE DATA; Schema: _timescaledb_cache; Owner: postgres
--

COPY _timescaledb_cache.cache_inval_hypertable  FROM stdin;
\.


--
-- Data for Name: hypertable; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.hypertable (id, schema_name, table_name, associated_schema_name, associated_table_prefix, num_dimensions, chunk_sizing_func_schema, chunk_sizing_func_name, chunk_target_size, compression_state, compressed_hypertable_id, replication_factor, status) FROM stdin;
2	_timescaledb_internal	_materialized_hypertable_2	_timescaledb_internal	_hyper_2	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	\N	0
3	_timescaledb_internal	_materialized_hypertable_3	_timescaledb_internal	_hyper_3	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	\N	0
4	_timescaledb_internal	_compressed_hypertable_4	_timescaledb_internal	_hyper_4	0	_timescaledb_functions	calculate_chunk_interval	0	2	\N	\N	0
1	public	metrics	_timescaledb_internal	_hyper_1	2	_timescaledb_functions	calculate_chunk_interval	0	1	4	\N	0
5	public	counters	_timescaledb_internal	_hyper_5	1	_timescaledb_functions	calculate_chunk_interval	0	0	\N	\N	0
\.


--
-- Data for Name: chunk; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk (id, hypertable_id, schema_name, table_name, compressed_chunk_id, dropped, status, osm_chunk, creation_time) FROM stdin;
4	1	_timescaledb_internal	_hyper_1_4_chunk	\N	f	0	f	2026-10-30 17:11:05.824561+00
8	1	_timescaledb_internal	_hyper_1_8_chunk	\N	f	0	f	2026-10-30 17:11:05.849041+00
9	2	_timescaledb_internal	_hyper_2_9_chunk	\N	f	0	f	2026-10-30 17:11:05.884564+00
10	2	_timescaledb_internal	_hyper_2_10_chunk	\N	f	0	f	2026-10-30 17:11:05.89084+00
11	3	_timescaledb_internal	_hyper_3_11_chunk	\N	f	0	f	2026-10-30 17:11:05.903612+00
12	3	_timescaledb_internal	_hyper_3_12_chunk	\N	f	0	f	2026-10-30 17:11:05.906795+00
1	1	_timescaledb_internal	_hyper_1_1_chunk	\N	t	0	f	2026-10-30 17:11:05.803049+00
5	1	_timescaledb_internal	_hyper_1_5_chunk	\N	t	0	f	2026-10-30 17:11:05.830743+00
13	4	_timescaledb_internal	compress_hyper_4_13_chunk	\N	f	0	f	2026-10-30 17:11:05.934676+00
2	1	_timescaledb_internal	_hyper_1_2_chunk	13	f	1	f	2026-10-30 17:11:05.810493+00
14	4	_timescaledb_internal	compress_hyper_4_14_chunk	\N	f	0	f	2026-10-30 17:11:05.947202+00
15	4	_timescaledb_internal	compress_hyper_4_15_chunk	\N	f	0	f	2026-10-30 17:11:05.959344+00
6	1	_timescaledb_internal	_hyper_1_6_chunk	15	f	1	f	2026-10-30 17:11:05.836934+00
16	4	_timescaledb_internal	compress_hyper_4_16_chunk	\N	f	0	f	2026-10-30 17:11:05.971436+00
3	1	_timescaledb_internal	_hyper_1_3_chunk	14	f	9	f	2026-10-30 17:11:05.818458+00
7	1	_timescaledb_internal	_hyper_1_7_chunk	16	f	9	f	2026-10-30 17:11:05.843115+00
17	5	_timescaledb_internal	_hyper_5_17_chunk	\N	f	0	f	2026-10-30 17:11:06.008238+00
18	5	_timescaledb_internal	_hyper_5_18_chunk	\N	f	0	f	2026-10-30 17:11:06.011497+00
19	5	_timescaledb_internal	_hyper_5_19_chunk	\N	f	0	f	2026-10-30 17:11:06.016462+00
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
\.


--
-- Data for Name: dimension_slice; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.dimension_slice (id, dimension_id, range_start, range_end) FROM stdin;
1	1	1703721600000000	1704326400000000
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
\.


--
-- Data for Name: chunk_constraint; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk_constraint (chunk_id, dimension_slice_id, constraint_name, hypertable_constraint_name) FROM stdin;
1	1	constraint_1	\N
1	2	constraint_2	\N
2	3	constraint_3	\N
2	2	constraint_2	\N
3	4	constraint_4	\N
3	2	constraint_2	\N
4	5	constraint_5	\N
4	2	constraint_2	\N
5	1	constraint_1	\N
5	6	constraint_6	\N
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
\.


--
-- Data for Name: chunk_data_node; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.chunk_data_node (chunk_id, node_chunk_id, node_name) FROM stdin;
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
13	compress_hyper_4_13_chunk__compressed_hypertable_4_device__ts_m	4	_compressed_hypertable_4_device__ts_meta_sequence_num_idx
14	compress_hyper_4_14_chunk__compressed_hypertable_4_device__ts_m	4	_compressed_hypertable_4_device__ts_meta_sequence_num_idx
15	compress_hyper_4_15_chunk__compressed_hypertable_4_device__ts_m	4	_compressed_hypertable_4_device__ts_meta_sequence_num_idx
16	compress_hyper_4_16_chunk__compressed_hypertable_4_device__ts_m	4	_compressed_hypertable_4_device__ts_meta_sequence_num_idx
17	_hyper_5_17_chunk_counters_tick_idx	5	counters_tick_idx
18	_hyper_5_18_chunk_counters_tick_idx	5	counters_tick_idx
19	_hyper_5_19_chunk_counters_tick_idx	5	counters_tick_idx
\.


--
-- Data for Name: compression_chunk_size; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.compression_chunk_size (chunk_id, compressed_chunk_id, uncompressed_heap_size, uncompressed_toast_size, uncompressed_index_size, compressed_heap_size, compressed_toast_size, compressed_index_size, numrows_pre_compression, numrows_post_compression, numrows_frozen_immediately) FROM stdin;
2	13	8192	0	32768	8192	8192	16384	14	1	1
3	14	8192	0	32768	8192	8192	16384	14	1	1
6	15	8192	0	32768	8192	8192	16384	28	2	2
7	16	8192	0	32768	8192	8192	16384	28	2	2
\.


--
-- Data for Name: continuous_agg; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.continuous_agg (mat_hypertable_id, raw_hypertable_id, parent_mat_hypertable_id, user_view_schema, user_view_name, partial_view_schema, partial_view_name, bucket_width, direct_view_schema, direct_view_name, materialized_only, finalized) FROM stdin;
2	1	\N	public	metrics_daily	_timescaledb_internal	_partial_view_2	86400000000	_timescaledb_internal	_direct_view_2	t	t
3	1	\N	public	metrics_weekly	_timescaledb_internal	_partial_view_3	604800000000	_timescaledb_internal	_direct_view_3	f	t
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

COPY _timescaledb_catalog.continuous_aggs_bucket_function (mat_hypertable_id, experimental, name, bucket_width, origin, timezone) FROM stdin;
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
-- Data for Name: dimension_partition; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.dimension_partition (dimension_id, range_start, data_nodes) FROM stdin;
2	-9223372036854775808	\N
2	1073741823	\N
\.


--
-- Data for Name: hypertable_compression; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.hypertable_compression (hypertable_id, attname, compression_algorithm_id, segmentby_column_index, orderby_column_index, orderby_asc, orderby_nullsfirst) FROM stdin;
1	time	4	\N	1	t	f
1	device	0	1	\N	\N	\N
1	value	3	\N	\N	\N	\N
\.


--
-- Data for Name: hypertable_data_node; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.hypertable_data_node (hypertable_id, node_hypertable_id, node_name, block_chunks) FROM stdin;
\.


--
-- Data for Name: metadata; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.metadata (key, value, include_in_telemetry) FROM stdin;
exported_uuid	8aca2329-ca13-45e1-8f4e-b923b231a774	t
\.


--
-- Data for Name: remote_txn; Type: TABLE DATA; Schema: _timescaledb_catalog; Owner: postgres
--

COPY _timescaledb_catalog.remote_txn (data_node_name, remote_transaction_id) FROM stdin;
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
-- Name: chunk_constraint_name; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.chunk_constraint_name', 1, false);


--
-- Name: chunk_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.chunk_id_seq', 19, true);


--
-- Name: continuous_agg_migrate_plan_step_step_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.continuous_agg_migrate_plan_step_step_id_seq', 1, false);


--
-- Name: dimension_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.dimension_id_seq', 5, true);


--
-- Name: dimension_slice_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.dimension_slice_id_seq', 13, true);


--
-- Name: hypertable_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_catalog; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_catalog.hypertable_id_seq', 5, true);


--
-- Name: bgw_job_id_seq; Type: SEQUENCE SET; Schema: _timescaledb_config; Owner: postgres
--

SELECT pg_catalog.setval('_timescaledb_config.bgw_job_id_seq', 1000, false);


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

\unrestrict rjL0xfTngC040vSn5PF056KeKiTuevhGwaTZMaFrPv1nQC9vLQhZzrQnvuKjYzp

