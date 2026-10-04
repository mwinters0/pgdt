-- The `emitters` schema's v17 sidecar: DDL 13 to 16 refuse, loaded after
-- fixture_schema_emitters.sql at 17 and above
-- (generate_fixtures.schema_files). A collation of the `builtin` provider,
-- which `dumpCollation` writes from 17; a column of it.
CREATE COLLATION emitters.c_builtin (provider = builtin, locale = 'C');
CREATE TABLE emitters.built (id integer, label text COLLATE emitters.c_builtin);
INSERT INTO emitters.built VALUES (1, 'b'), (2, 'a');
