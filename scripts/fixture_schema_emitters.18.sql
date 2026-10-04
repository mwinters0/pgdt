-- The `emitters` schema's v18 sidecar: DDL 13 to 17 refuse, loaded after
-- fixture_schema_emitters.sql at 18 and above
-- (generate_fixtures.schema_files). A temporal `UNIQUE` constraint, which
-- `dumpConstraint` writes `WITHOUT OVERLAPS` from 18.
CREATE TABLE emitters.booked (
    room int4range,
    during daterange,
    CONSTRAINT booked_key UNIQUE (room, during WITHOUT OVERLAPS)
);
INSERT INTO emitters.booked VALUES ('[1,2)', '[2024-01-01,2024-01-05)');
