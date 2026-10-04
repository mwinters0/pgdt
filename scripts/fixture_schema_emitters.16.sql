-- The `emitters` schema's v16 sidecar: DDL 13 to 15 refuse, loaded after
-- fixture_schema_emitters.sql at 16 and above
-- (generate_fixtures.schema_files). An ICU collation with tailoring `rules`,
-- which `dumpCollation` writes from 16; a column of it.
CREATE COLLATION emitters.c_rules (provider = icu, locale = 'und', rules = '&a < b');
CREATE TABLE emitters.ruled (id integer, label text COLLATE emitters.c_rules);
INSERT INTO emitters.ruled VALUES (1, 'b'), (2, 'a');
