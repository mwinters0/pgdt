-- TimescaleDB fixture schema: every way a pinned TimescaleDB version stores a
-- hypertable's rows, each oracle-checked (`docs/design/decisions.md`,
-- "D130"). Loaded into
-- `timescale/timescaledb-ha` at each pin (`generate_fixtures.py`'s
-- `TIMESCALEDB_PINS`) and dumped by the routine `postgres:` image's `pg_dump`.
-- Written from TimescaleDB's Apache-licensed `sql/` and its documentation.
--
-- Every timestamp is fixed and the session is UTC, so a regeneration moves
-- nothing but what D69 names, the catalog's `creation_time`, and `metadata`'s
-- `install_timestamp` and `exported_uuid`.
--
-- `metrics` holds the shapes, one per seven-day time slice, each slice cut
-- in two by the hash dimension on `device`:
--
--   2023-12-28 .. 2024-01-04   dropped once its rows are materialized: a
--                              catalog row marked `dropped` at 2.13, none
--                              at a later pin (TS6)
--   2024-01-04 .. 2024-01-11   compressed
--   2024-01-11 .. 2024-01-18   compressed, then a row inserted: partially
--                              compressed (status bit 8)
--   2024-01-18 .. 2024-01-25   uncompressed
--
-- `counters` is an integer-time hypertable, uncompressed. The two continuous
-- aggregates over `metrics` are one materialized-only and one real-time.

SET TimeZone = 'UTC';

CREATE EXTENSION IF NOT EXISTS timescaledb;
-- The image's template database carries the toolkit, which nothing here is
-- about.
DROP EXTENSION IF EXISTS timescaledb_toolkit;

CREATE TABLE public.metrics (
    "time" timestamp with time zone NOT NULL,
    device integer NOT NULL,
    value double precision
);

-- Time slices of seven days, aligned to the epoch (a Thursday), and two
-- hash partitions on `device`.
SELECT create_hypertable('public.metrics', 'time', 'device', 2,
                         chunk_time_interval => interval '7 days');

-- Every twelve hours from 2023-12-29 to 2024-01-24, for devices 1-3.
INSERT INTO public.metrics
SELECT t, d, d * 100 + extract(epoch FROM t - timestamptz '2023-12-29') / 3600
FROM generate_series(timestamptz '2023-12-29 00:00', timestamptz '2024-01-24 12:00',
                     interval '12 hours') AS t,
     generate_series(1, 3) AS d;

CREATE MATERIALIZED VIEW public.metrics_daily
WITH (timescaledb.continuous, timescaledb.materialized_only = true) AS
SELECT time_bucket(interval '1 day', "time") AS day, device, count(*) AS n, max(value) AS top
FROM public.metrics
GROUP BY 1, 2
WITH NO DATA;

CREATE MATERIALIZED VIEW public.metrics_weekly
WITH (timescaledb.continuous, timescaledb.materialized_only = false) AS
SELECT time_bucket(interval '7 days', "time") AS week, count(*) AS n
FROM public.metrics
GROUP BY 1
WITH NO DATA;

CALL refresh_continuous_aggregate('public.metrics_daily', NULL, NULL);
CALL refresh_continuous_aggregate('public.metrics_weekly', NULL, NULL);

-- With a continuous aggregate on the hypertable, a dropped chunk keeps its
-- catalog row, marked `dropped`, through 2.14
-- (`docs/design/timescaledb-invariants.md`, "TS6").
SELECT drop_chunks('public.metrics', older_than => timestamptz '2024-01-04');

ALTER TABLE public.metrics SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'device',
    timescaledb.compress_orderby = 'time'
);

SELECT compress_chunk(c)
FROM show_chunks('public.metrics', older_than => timestamptz '2024-01-18') AS c;

-- Rows inserted into a compressed chunk stay in it: partially compressed.
INSERT INTO public.metrics VALUES
    ('2024-01-12 06:00+00', 1, -1),
    ('2024-01-12 06:00+00', 2, -2);

CREATE TABLE public.counters (
    tick bigint NOT NULL,
    n integer
);

SELECT create_hypertable('public.counters', 'tick', chunk_time_interval => 100);

INSERT INTO public.counters SELECT t, t / 10 FROM generate_series(0, 290, 10) AS t;
