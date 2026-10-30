-- Stray rows in a hypertable's root, which nothing refuses from 2.23, when
-- the `ts_insert_blocker` trigger went; loaded after
-- `fixture_schema_timescaledb.sql` at the pins from 2.27
-- (`generate_fixtures.py`'s `TIMESCALEDB_PINS`). In restoring mode an insert
-- is not routed to a chunk, as a restore's `COPY` into the root is not.

SET TimeZone = 'UTC';

SET timescaledb.restoring = on;
INSERT INTO public.metrics VALUES ('2024-01-20 00:00+00', 9, 900);
RESET timescaledb.restoring;
