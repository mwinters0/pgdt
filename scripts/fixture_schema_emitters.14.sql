-- The `emitters` schema's v14 sidecar: DDL 13 refuses, loaded after
-- fixture_schema_emitters.sql at 14 and above
-- (generate_fixtures.schema_files). A column's stated compression, which
-- `dumpTableSchema` writes as its own `ALTER ... SET COMPRESSION`; `pglz`, the
-- method every build has.
ALTER TABLE emitters.tuned ALTER COLUMN note SET COMPRESSION pglz;
