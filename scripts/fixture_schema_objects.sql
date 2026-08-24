-- DDL-object-inventory fixture schema: one object per TOC `Type:` kind that
-- neither scripts/fixture_schema_edge_cases.sql nor scripts/fixture_schema_types.sql
-- produces, plus a large object (a shape no pg_dump flag conjures on its own).
--
-- Backs docs/design/roadmap-phase3-object-inventory.md, slice 3.1. The TOC
-- vocabulary (~63 `Type:` values) is enumerable from `pg_dump` source --
-- see that doc's "Scanning" section for the exact grep. Across the two
-- existing schemas, real pg_dump output produces only: TABLE, TABLE DATA,
-- CONSTRAINT, FK CONSTRAINT, FUNCTION, SCHEMA, TYPE, DOMAIN, SHELL TYPE.
-- Everything below is chosen to fill that gap.
--
-- Deliberately NOT attempted here, with why:
--   - SECURITY LABEL: requires a security-label provider (e.g. sepgsql)
--     registered via shared_preload_libraries; not present in a stock
--     postgres:*-alpine image.
--   - ACCESS METHOD, OPERATOR, OPERATOR CLASS, OPERATOR FAMILY, TRANSFORM,
--     TEXT SEARCH PARSER, TEXT SEARCH TEMPLATE: all require a C-level
--     handler function, so a real one only ever arrives via an extension
--     (e.g. bloom, hstore) rather than plain SQL. Pulling in an extension
--     just to exercise a TOC label this schema doesn't otherwise need is
--     out of proportion to what it buys. (TEXT SEARCH CONFIGURATION and
--     TEXT SEARCH DICTIONARY don't need this -- both are buildable from the
--     always-present `simple` template, no extension required.)
--   - DATABASE, DATABASE PROPERTIES, ENCODING, SEARCHPATH, STDSTRINGS:
--     dump-level metadata already exercised by edge_cases/create.sql and
--     the dumpall fixture (docs/design/pg-dump-compatibility.md) -- not a
--     DDL object this phase's inventory is about.
--   - STATISTICS DATA: only emitted under --statistics (PG18+ -- see
--     generate_fixtures.py's version-conditional "stats" flag set for this
--     schema, objects/stats.sql, v18 only).
--
-- FOREIGN DATA WRAPPER / SERVER / USER MAPPING use postgres_fdw, which
-- object creation never actually dials out for -- only querying through it
-- would.

-- A role for GRANT/ALTER DEFAULT PRIVILEGES/large-object ACL to reference,
-- mirroring koji's `backup` role -- reachable only through post-data GRANTs,
-- never a CREATE ROLE statement, since roles are cluster-global and a
-- single-database pg_dump never emits one. See
-- docs/status/history/2026-08-23.md ("koji's role and privilege references").
CREATE ROLE fixture_reader NOLOGIN;

-- A non-default tablespace, for the TOC header's `; Tablespace: <name>`
-- suffix (I18) and the `SET default_tablespace = ...;` framing (I19) --
-- neither reachable through any pg_dump flag, since both need a real
-- filesystem location. generate_fixtures.py's prepare_tablespace_dir
-- mkdir/chowns the directory this LOCATION points at, in this schema's own
-- container, before this script runs.
CREATE TABLESPACE fixture_ts LOCATION '/var/lib/postgresql/fixture_tablespace';

CREATE SCHEMA IF NOT EXISTS objects;

-- ACL, DEFAULT: a table with a non-serial default and a direct GRANT.
CREATE TABLE objects.widgets (
    id integer PRIMARY KEY,
    label text DEFAULT 'unnamed',
    created_at timestamptz DEFAULT now()
);
GRANT SELECT ON objects.widgets TO fixture_reader;
GRANT SELECT ON objects.widgets TO PUBLIC;

-- SEQUENCE, SEQUENCE OWNED BY, SEQUENCE SET: `serial` (not
-- GENERATED ALWAYS AS IDENTITY) is what makes pg_dump emit a standalone
-- "SEQUENCE OWNED BY" TOC entry -- an identity column's sequence is owned
-- too, but pg_dump special-cases it (`is_identity_sequence`) and folds the
-- ownership into the identity clause instead, emitting no separate entry.
-- Inserting rows advances the sequence, so pg_dump also emits a
-- SEQUENCE SET restoring its position.
CREATE TABLE objects.orders (
    id serial PRIMARY KEY,
    widget_id integer REFERENCES objects.widgets(id),
    quantity integer NOT NULL DEFAULT 1
);
INSERT INTO objects.widgets (id, label) VALUES (1, 'alpha'), (2, 'beta');
INSERT INTO objects.orders (widget_id, quantity) VALUES (1, 3), (2, 1), (1, 7);

-- A standalone sequence, not owned by any column.
CREATE SEQUENCE objects.standalone_seq START 100 INCREMENT 5;
SELECT nextval('objects.standalone_seq');

-- INDEX: an explicit, non-PK index.
CREATE INDEX widgets_label_idx ON objects.widgets (label);

-- VIEW, MATERIALIZED VIEW (+ its MATERIALIZED VIEW DATA, since it is
-- populated at creation time).
CREATE VIEW objects.widget_orders AS
    SELECT w.id, w.label, o.quantity
    FROM objects.widgets w
    JOIN objects.orders o ON o.widget_id = w.id;

CREATE MATERIALIZED VIEW objects.widget_totals AS
    SELECT widget_id, sum(quantity) AS total_quantity
    FROM objects.orders
    GROUP BY widget_id
    WITH DATA;

-- DEFAULT: an ordinary table's column default is always folded into its
-- CREATE TABLE (pg_dump's dumpAttrDef only ever emits it separately for a
-- suppressed column -- a --binary-upgrade dropped-column shape the
-- edge_cases schema already covers -- or, as here, a view column, which
-- can never have an inline default of its own).
ALTER TABLE objects.widget_orders ALTER COLUMN quantity SET DEFAULT 0;

-- TRIGGER: a trivial audit trigger.
CREATE TABLE objects.widget_audit (
    widget_id integer,
    changed_at timestamptz DEFAULT now()
);

CREATE FUNCTION objects.log_widget_change() RETURNS trigger
    LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO objects.widget_audit (widget_id) VALUES (NEW.id);
    RETURN NEW;
END;
$$;

CREATE TRIGGER widgets_audit_trigger
    AFTER INSERT OR UPDATE ON objects.widgets
    FOR EACH ROW EXECUTE FUNCTION objects.log_widget_change();

-- RULE: a rule is distinct from a trigger (query rewrite vs. row callback).
CREATE TABLE objects.widgets_deleted_log (
    id integer,
    deleted_at timestamptz DEFAULT now()
);

CREATE RULE widgets_log_delete AS ON DELETE TO objects.widgets
    DO ALSO INSERT INTO objects.widgets_deleted_log (id) VALUES (OLD.id);

-- POLICY, ROW SECURITY: row-level security on its own table so it doesn't
-- interfere with the other fixture reads above.
CREATE TABLE objects.secrets (
    owner_role text NOT NULL,
    payload text NOT NULL
);
ALTER TABLE objects.secrets ENABLE ROW LEVEL SECURITY;
CREATE POLICY secrets_owner_only ON objects.secrets
    USING (owner_role = current_user);

-- DEFAULT ACL: applies to objects created later by `postgres` in this
-- schema, not retroactively -- placement doesn't matter for what pg_dump
-- emits, since it's a standing default, not a per-object grant.
ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA objects
    GRANT SELECT ON TABLES TO fixture_reader;

-- COMMENT: comments on more than one object kind, since pg_dump emits one
-- COMMENT entry per commented object regardless of kind.
COMMENT ON TABLE objects.widgets IS 'canonical widget catalog';
COMMENT ON COLUMN objects.widgets.label IS 'human-readable widget name';

-- PUBLICATION, PUBLICATION TABLE, PUBLICATION TABLES IN SCHEMA: two
-- publications so both sub-entry shapes appear (FOR TABLE vs. FOR TABLES
-- IN SCHEMA); FOR ALL TABLES would produce neither as its own sub-entry.
-- `FOR TABLES IN SCHEMA` is PG15+ (added alongside the TOC type of the same
-- name); this schema is loaded against every routine version back to 13, so
-- the older branch falls back to a second FOR TABLE publication instead --
-- still real PUBLICATION TABLE coverage, just not the schema-level shape.
CREATE PUBLICATION objects_pub_table FOR TABLE objects.widgets;
SELECT current_setting('server_version_num')::int >= 150000 AS pub_schema_supported \gset
\if :pub_schema_supported
CREATE PUBLICATION objects_pub_schema FOR TABLES IN SCHEMA objects;
\else
CREATE PUBLICATION objects_pub_schema FOR TABLE objects.orders;
\endif

-- SUBSCRIPTION: connect = false means CREATE SUBSCRIPTION never dials out,
-- so this never depends on a reachable publisher. slot_name = NONE means
-- there is no remote replication slot to manage, so it's also droppable
-- with no connection -- generate_fixtures.py's cleanup between schemas
-- relies on that.
CREATE SUBSCRIPTION objects_sub
    CONNECTION 'host=nonexistent dbname=nonexistent'
    PUBLICATION objects_pub_table
    WITH (connect = false, create_slot = false, enabled = false, slot_name = NONE);

-- STATISTICS: extended statistics object.
CREATE STATISTICS objects.orders_stats (dependencies)
    ON widget_id, quantity FROM objects.orders;

-- TABLE ATTACH, INDEX ATTACH: a partitioned table with a partition attached
-- at creation time, and an index on the parent so its per-partition local
-- index is auto-created and attached.
CREATE TABLE objects.events (
    id bigint NOT NULL,
    event_date date NOT NULL,
    payload text
) PARTITION BY RANGE (event_date);

CREATE TABLE objects.events_2024 PARTITION OF objects.events
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

CREATE INDEX events_event_date_idx ON objects.events (event_date);

INSERT INTO objects.events (id, event_date, payload) VALUES
    (1, '2024-06-01', 'first'),
    (2, '2024-12-31', 'second');

-- FOREIGN DATA WRAPPER, SERVER, USER MAPPING: object creation for all
-- three succeeds with no live connection -- only querying through the
-- server would require one.
CREATE EXTENSION IF NOT EXISTS postgres_fdw;
CREATE SERVER objects_remote FOREIGN DATA WRAPPER postgres_fdw
    OPTIONS (host 'nonexistent', dbname 'nonexistent');
CREATE USER MAPPING FOR postgres SERVER objects_remote
    OPTIONS (user 'nobody', password 'unused');

-- CAST: between two independent enum types, so this doesn't collide with
-- any implicit cast PostgreSQL already defines.
CREATE TYPE objects.color_rgb AS ENUM ('red', 'green', 'blue');
CREATE TYPE objects.color_cmyk AS ENUM ('cyan', 'magenta', 'yellow', 'black');

CREATE FUNCTION objects.rgb_to_cmyk(objects.color_rgb) RETURNS objects.color_cmyk
    LANGUAGE sql IMMUTABLE AS $$
    SELECT CASE $1::text
        WHEN 'red' THEN 'magenta'::objects.color_cmyk
        WHEN 'green' THEN 'yellow'::objects.color_cmyk
        ELSE 'cyan'::objects.color_cmyk
    END;
$$;

CREATE CAST (objects.color_rgb AS objects.color_cmyk)
    WITH FUNCTION objects.rgb_to_cmyk(objects.color_rgb);

-- AGGREGATE: the textbook minimal aggregate, built from an existing
-- transition function.
CREATE AGGREGATE objects.my_sum(integer) (
    sfunc = int4pl,
    stype = integer,
    initcond = '0'
);

-- COLLATION: copies the always-present "C" collation, so this needs no
-- locale beyond what every PostgreSQL build ships.
CREATE COLLATION objects.c_collation FROM "C";

-- TEXT SEARCH DICTIONARY, TEXT SEARCH CONFIGURATION: built from the
-- `simple` template/config every PostgreSQL build ships, so neither needs
-- an extension the way TEXT SEARCH PARSER/TEMPLATE would.
CREATE TEXT SEARCH DICTIONARY objects.simple_dict (TEMPLATE = simple);
CREATE TEXT SEARCH CONFIGURATION objects.simple_config (COPY = simple);
ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ALTER MAPPING FOR asciiword WITH objects.simple_dict;

-- CONVERSION: built from a conversion function every PostgreSQL build
-- registers regardless of the server's own encoding.
CREATE CONVERSION objects.latin1_to_utf8 FOR 'LATIN1' TO 'UTF8' FROM iso8859_1_to_utf8;

-- EVENT TRIGGER: needs a function returning event_trigger; disabled so it
-- never actually fires while the rest of this script runs.
CREATE FUNCTION objects.noop_event_trigger() RETURNS event_trigger
    LANGUAGE plpgsql AS $$
BEGIN
END;
$$;

CREATE EVENT TRIGGER objects_ddl_log ON ddl_command_start
    EXECUTE FUNCTION objects.noop_event_trigger();
ALTER EVENT TRIGGER objects_ddl_log DISABLE;

-- A table in the non-default tablespace created above: exercises the TOC
-- header's `; Tablespace: <name>` suffix (I18) and, since a table's
-- tablespace differs from the connection's own default, the
-- `SET default_tablespace = fixture_ts;` / `SET default_tablespace = '';`
-- framing pg_dump wraps its definition in (I19).
CREATE TABLE objects.tablespaced_table (id integer) TABLESPACE fixture_ts;

-- REVOKE: a function's EXECUTE privilege is granted to PUBLIC by default,
-- so revoking it is the one ACL shape whose target state has *fewer*
-- privileges than the default -- pg_dump's ACL diff then emits a solo
-- REVOKE with no offsetting GRANT, I19's REVOKE shape, otherwise
-- unexercised by any fixture. (Revoking a privilege from an *owner*
-- instead, e.g. objects.widgets above, always pairs the REVOKE with a
-- GRANT restoring the owner's remaining implicit privileges, which doesn't
-- isolate the shape as cleanly.)
CREATE FUNCTION objects.no_public_execute() RETURNS integer
    LANGUAGE sql IMMUTABLE AS $$ SELECT 1; $$;
REVOKE EXECUTE ON FUNCTION objects.no_public_execute() FROM PUBLIC;

-- BLOBS, BLOB METADATA: large objects are never conjured by any pg_dump
-- flag -- lo_from_bytea is the only way to get one into a fixture at all.
-- One large object gets a comment and a GRANT (mirroring koji's ACL-only
-- role discovery case; see the header comment above), the other is bare
-- content with no metadata, since a bare large object is the common case.
DO $$
DECLARE
    lo_with_meta oid;
    lo_bare oid;
BEGIN
    lo_with_meta := lo_from_bytea(0, decode('48656c6c6f2c204c4f21', 'hex'));
    lo_bare := lo_from_bytea(0, decode('00010203040506070809', 'hex'));
    EXECUTE format('COMMENT ON LARGE OBJECT %s IS %L', lo_with_meta, 'first large object');
    EXECUTE format('GRANT SELECT ON LARGE OBJECT %s TO fixture_reader', lo_with_meta);
END;
$$;
