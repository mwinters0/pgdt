-- The 2.21 pin's hypercore chunk, loaded after `fixture_schema_timescaledb.sql`
-- (`generate_fixtures.py`'s `TIMESCALEDB_PINS`): a chunk compressed into the
-- `hypercore` table access method (2.18-2.21), which `pg_dump` precedes with
-- `SET default_table_access_method = hypercore`.

SET TimeZone = 'UTC';

CREATE TABLE public.columnar (
    "time" timestamp with time zone NOT NULL,
    device integer NOT NULL,
    value double precision
);

SELECT create_hypertable('public.columnar', 'time', chunk_time_interval => interval '7 days');

INSERT INTO public.columnar
SELECT t, 1, extract(day FROM t) FROM generate_series(timestamptz '2024-01-04', timestamptz '2024-01-10',
                                                     interval '1 day') AS t;

ALTER TABLE public.columnar SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'device',
    timescaledb.compress_orderby = 'time'
);

SELECT compress_chunk(c, hypercore_use_access_method => true) FROM show_chunks('public.columnar') AS c;
