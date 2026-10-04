-- The `emitters` schema's v15 sidecar: DDL 13 and 14 refuse, loaded after
-- fixture_schema_emitters.sql at 15 and above
-- (generate_fixtures.schema_files). `dumpConstraint`'s `NULLS NOT DISTINCT`.
ALTER TABLE emitters.keyed ADD CONSTRAINT keyed_note_key UNIQUE NULLS NOT DISTINCT (note);
