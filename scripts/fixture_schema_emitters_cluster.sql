-- The `emitters` schema's cluster-global half, run in the `postgres` database
-- before fixture_schema_emitters.sql loads (scripts/generate_fixtures.py,
-- `CLUSTER_SCRIPTS`), so the schema's `pg_dumpall` flag sets reach the
-- emitters that write globals.
--
-- A tablespace with options and a comment: `dumpTablespaces`' `CREATE
-- TABLESPACE`, `LOCATION`, `ALTER TABLESPACE ... SET` and `COMMENT ON
-- TABLESPACE`, and under `--clean` `dropTablespaces`. Its directory is made by
-- the generator, owned by the server's OS user.
CREATE TABLESPACE emitters_ts LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost = 1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';

-- A database whose name holds a byte outside `[A-Za-z0-9_.]`, which
-- `appendPsqlMetaConnect` enters by `\encoding SQL_ASCII` and `\connect
-- -reuse-previous=on "dbname=..."` (`KD68`), and which `--clean` drops by name.
-- By I30 it follows `template1` and precedes `pgdt_fixture`, `-` sorting
-- below `_`, and it carries a table, so a segment attributed to the database
-- before it shows.
CREATE DATABASE "pgdt-emitters";
\connect "pgdt-emitters"
CREATE TABLE public.named (id integer PRIMARY KEY, label text, born date);
INSERT INTO public.named VALUES (1, 'first', '2024-04-01'), (2, NULL, NULL);
