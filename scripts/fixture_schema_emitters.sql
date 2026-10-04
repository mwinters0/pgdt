-- The emitter-coverage fixture schema: DDL whose dump holds a literal of
-- `pg_dump`'s emitters that no other schema reaches
-- (docs/design/decisions.md, "D71";
-- scripts/emitter_register.py is the register it is joined against).
--
-- Every object below exists for a literal some listed emitter appends, named
-- beside it, so a reading that misses one is a register row going uncovered
-- rather than a gap nobody sees. Each table carries rows, so a reading that
-- mis-types a column shows in a typed read and not only in the map.
--
-- The cluster-global half -- a commented tablespace with options, and a
-- database whose name `appendPsqlMetaConnect` cannot write bare -- is set up
-- by scripts/generate_fixtures.py (`CLUSTER_SCRIPTS`), since `pg_dumpall` is
-- what reads it.

CREATE SCHEMA emitters;

-- dumpBaseType: a base type over built-in I/O functions, which the superuser
-- the generator connects as may declare `LANGUAGE internal`. One per
-- alignment and storage `dumpBaseType` spells, between them every optional
-- property it writes. `bt_varchar` borrows varchar's typmod functions, so a
-- column of it carries a typmod.
CREATE TYPE emitters.bt_varchar;
CREATE FUNCTION emitters.bt_varchar_in(cstring, oid, integer) RETURNS emitters.bt_varchar
    AS 'varcharin' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_varchar_out(emitters.bt_varchar) RETURNS cstring
    AS 'varcharout' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_varchar_recv(internal, oid, integer) RETURNS emitters.bt_varchar
    AS 'varcharrecv' LANGUAGE internal STABLE STRICT;
CREATE FUNCTION emitters.bt_varchar_send(emitters.bt_varchar) RETURNS bytea
    AS 'varcharsend' LANGUAGE internal STABLE STRICT;
CREATE FUNCTION emitters.bt_varchar_typmod_in(cstring[]) RETURNS integer
    AS 'varchartypmodin' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_varchar_typmod_out(integer) RETURNS cstring
    AS 'varchartypmodout' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_varchar_analyze(internal) RETURNS boolean
    AS 'ts_typanalyze' LANGUAGE internal STRICT;
CREATE TYPE emitters.bt_varchar (
    INTERNALLENGTH = variable,
    INPUT = emitters.bt_varchar_in,
    OUTPUT = emitters.bt_varchar_out,
    RECEIVE = emitters.bt_varchar_recv,
    SEND = emitters.bt_varchar_send,
    TYPMOD_IN = emitters.bt_varchar_typmod_in,
    TYPMOD_OUT = emitters.bt_varchar_typmod_out,
    ANALYZE = emitters.bt_varchar_analyze,
    COLLATABLE = true,
    DEFAULT = 'none',
    CATEGORY = 'S',
    PREFERRED = true,
    DELIMITER = ';',
    STORAGE = external
);

CREATE TYPE emitters.bt_char;
CREATE FUNCTION emitters.bt_char_in(cstring) RETURNS emitters.bt_char
    AS 'charin' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_char_out(emitters.bt_char) RETURNS cstring
    AS 'charout' LANGUAGE internal IMMUTABLE STRICT;
CREATE TYPE emitters.bt_char (
    INTERNALLENGTH = 1,
    INPUT = emitters.bt_char_in,
    OUTPUT = emitters.bt_char_out,
    ALIGNMENT = char,
    STORAGE = plain,
    PASSEDBYVALUE
);

CREATE TYPE emitters.bt_int2;
CREATE FUNCTION emitters.bt_int2_in(cstring) RETURNS emitters.bt_int2
    AS 'int2in' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_int2_out(emitters.bt_int2) RETURNS cstring
    AS 'int2out' LANGUAGE internal IMMUTABLE STRICT;
CREATE TYPE emitters.bt_int2 (
    INTERNALLENGTH = 2,
    INPUT = emitters.bt_int2_in,
    OUTPUT = emitters.bt_int2_out,
    ALIGNMENT = int2,
    PASSEDBYVALUE
);

CREATE TYPE emitters.bt_text_main;
CREATE FUNCTION emitters.bt_text_main_in(cstring) RETURNS emitters.bt_text_main
    AS 'textin' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_text_main_out(emitters.bt_text_main) RETURNS cstring
    AS 'textout' LANGUAGE internal IMMUTABLE STRICT;
CREATE TYPE emitters.bt_text_main (
    INTERNALLENGTH = variable,
    INPUT = emitters.bt_text_main_in,
    OUTPUT = emitters.bt_text_main_out,
    STORAGE = main
);

-- A fixed-length type with an element, as `point` is: from 14 the server
-- gives it `raw_array_subscript_handler`, which `dumpBaseType` writes as
-- `SUBSCRIPT`.
CREATE TYPE emitters.bt_pair;
CREATE FUNCTION emitters.bt_pair_in(cstring) RETURNS emitters.bt_pair
    AS 'point_in' LANGUAGE internal IMMUTABLE STRICT;
CREATE FUNCTION emitters.bt_pair_out(emitters.bt_pair) RETURNS cstring
    AS 'point_out' LANGUAGE internal IMMUTABLE STRICT;
CREATE TYPE emitters.bt_pair (
    INTERNALLENGTH = 16,
    INPUT = emitters.bt_pair_in,
    OUTPUT = emitters.bt_pair_out,
    ELEMENT = float8,
    ALIGNMENT = double
);

CREATE TABLE emitters.base_values (
    id integer PRIMARY KEY,
    v_varchar emitters.bt_varchar(8),
    v_char emitters.bt_char,
    v_int2 emitters.bt_int2,
    v_main emitters.bt_text_main,
    v_pair emitters.bt_pair
);
INSERT INTO emitters.base_values VALUES
    (1, 'alpha', 'a', '7', 'main text', '(1.5,2)'),
    (2, NULL, NULL, NULL, NULL, NULL);

-- dumpBaseType's `DELIMITER`, as the value form it selects: `array_out`
-- separates a `bt_varchar` array's elements with `;`, quoting one holding a
-- blank or the delimiter, and a composite holding such an array is where a
-- strict parse splits it (I22).
CREATE TYPE emitters.bt_list AS (items emitters.bt_varchar[]);
CREATE TABLE emitters.delimited (
    id integer PRIMARY KEY,
    v emitters.bt_list,
    a emitters.bt_varchar[]
);
INSERT INTO emitters.delimited VALUES
    (1, ROW('{"a b";c,d;"e;f"}'::emitters.bt_varchar[]), '{"a b";c}'),
    (2, ROW('{{x;"y z"};{"";NULL}}'::emitters.bt_varchar[]), NULL),
    (3, NULL, NULL);

-- dumpRangeType: a canonical function, declared on the shell type as only a
-- C-language one can be; a difference function; and an operator class that is
-- not the subtype's default.
CREATE TYPE emitters.r_canon;
CREATE FUNCTION emitters.r_canon_canonical(emitters.r_canon) RETURNS emitters.r_canon
    AS 'int4range_canonical' LANGUAGE internal IMMUTABLE STRICT;
CREATE TYPE emitters.r_canon AS RANGE (
    subtype = integer,
    canonical = emitters.r_canon_canonical
);
CREATE TYPE emitters.r_diff AS RANGE (subtype = float8, subtype_diff = float8mi);
CREATE TYPE emitters.r_pattern AS RANGE (subtype = text, subtype_opclass = text_pattern_ops);

CREATE TABLE emitters.range_values (
    id integer PRIMARY KEY,
    v_canon emitters.r_canon,
    v_diff emitters.r_diff,
    v_pattern emitters.r_pattern
);
INSERT INTO emitters.range_values VALUES
    (1, '[1,5]', '[1.5,2.5)', '[a,m)'),
    (2, 'empty', '(,0]', '[n,)'),
    (3, NULL, NULL, NULL);

-- dumpDomain: a named `CHECK` constraint; dumpEnumType and
-- dumpCompositeType: one of each, so `--clean` writes each emitter's `DROP`.
CREATE DOMAIN emitters.positive AS integer CONSTRAINT positive_check CHECK (VALUE > 0);
CREATE TYPE emitters.mood AS ENUM ('calm', 'busy');
CREATE TYPE emitters.pair AS (left_part integer, right_part text);

CREATE TABLE emitters.domain_values (
    id integer PRIMARY KEY,
    v_positive emitters.positive,
    v_mood emitters.mood,
    v_pair emitters.pair
);
INSERT INTO emitters.domain_values VALUES
    (1, 3, 'busy', ROW(1, 'one')),
    (2, NULL, NULL, NULL);

-- dumpCompositeType under `--binary-upgrade`: a dropped attribute is
-- recreated and then dropped by `ALTER TYPE ... DROP ATTRIBUTE`.
CREATE TYPE emitters.trio AS (a integer, b text, c date);
ALTER TYPE emitters.trio DROP ATTRIBUTE b;
CREATE TABLE emitters.trios (id integer PRIMARY KEY, v emitters.trio);
INSERT INTO emitters.trios VALUES (1, ROW(1, '2024-01-01')), (2, NULL);

-- dumpTableSchema's per-table forms: reloptions, a view's check option, row
-- security forced, both replica identities it spells, a column statistics
-- target, and a view over the table so `--clean` writes `DROP VIEW`.
CREATE TABLE emitters.tuned (
    id integer PRIMARY KEY,
    note text,
    amount numeric(8,2)
) WITH (fillfactor = 70, toast.autovacuum_enabled = false);
ALTER TABLE emitters.tuned ALTER COLUMN amount SET STATISTICS 500;
ALTER TABLE emitters.tuned ENABLE ROW LEVEL SECURITY;
ALTER TABLE emitters.tuned FORCE ROW LEVEL SECURITY;
ALTER TABLE emitters.tuned REPLICA IDENTITY FULL;
INSERT INTO emitters.tuned VALUES (1, 'one', 1.50), (2, NULL, NULL);

CREATE TABLE emitters.unidentified (id integer, label text);
ALTER TABLE emitters.unidentified REPLICA IDENTITY NOTHING;
INSERT INTO emitters.unidentified VALUES (1, 'x');

CREATE VIEW emitters.positive_tuned WITH (security_barrier) AS
    SELECT id, note FROM emitters.tuned WHERE id > 0
    WITH LOCAL CHECK OPTION;

-- `--binary-upgrade`'s table forms: a populated materialized view, a column
-- added with a default after rows exist (its missing value), and two dropped
-- columns in one table.
CREATE MATERIALIZED VIEW emitters.tuned_totals AS
    SELECT count(*) AS n FROM emitters.tuned WITH DATA;

CREATE TABLE emitters.grown (id integer PRIMARY KEY, a text, b text, c text);
INSERT INTO emitters.grown VALUES (1, 'a', 'b', 'c'), (2, 'a2', 'b2', 'c2');
ALTER TABLE emitters.grown ADD COLUMN added date DEFAULT '2024-02-29';
ALTER TABLE emitters.grown DROP COLUMN a;
ALTER TABLE emitters.grown DROP COLUMN c;
INSERT INTO emitters.grown (id, b) VALUES (3, 'b3');

-- `CREATE UNLOGGED TABLE`.
CREATE UNLOGGED TABLE emitters.scratch (id integer, at date, ok boolean);
INSERT INTO emitters.scratch VALUES (1, '2024-03-01', true), (2, NULL, false);

-- Inheritance: `INHERITS (`, and under `--binary-upgrade` the
-- inherited columns', the inherited `CHECK`'s and (at 18) the inherited not-null
-- constraint's fix-ups. The child's `label` is `NOT NULL` where the parent's
-- is not, which 13-17 write as a separate `SET NOT NULL`.
CREATE TABLE emitters.parent (
    id integer NOT NULL,
    label text,
    born date,
    CONSTRAINT parent_id_positive CHECK (id > 0)
);
CREATE TABLE emitters.child (extra numeric(6,2)) INHERITS (emitters.parent);
ALTER TABLE emitters.child ALTER COLUMN label SET NOT NULL;
INSERT INTO emitters.parent VALUES (1, 'p', '2024-01-01');
INSERT INTO emitters.child VALUES (2, 'c', '2024-01-02', 1.25), (3, 'd', NULL, NULL);

-- A default holding a comma inside `ARRAY[...]`, then a real column named like
-- the word after that comma: the shape `KD69` split into a column `now` of type
-- `()]` that the real one resolved through. With the parent's inline `CHECK`
-- and, at 18, the child's table-level `NOT NULL label`, the fragments a
-- column list holds that are no column.
CREATE TABLE emitters.stamped (
    id integer PRIMARY KEY,
    stamps timestamp with time zone[] DEFAULT ARRAY[now(), now()],
    now integer
);
INSERT INTO emitters.stamped VALUES
    (1, '{"2024-01-01 00:00:00+00","2024-01-02 12:30:00+00"}', 7),
    (2, NULL, NULL);

-- A typed table: `OF`, with a column written without its type
-- because it carries `NOT NULL`, and under `--binary-upgrade` the typed-table
-- fix-up.
CREATE TYPE emitters.person AS (name text, born date, height integer);
CREATE TABLE emitters.people OF emitters.person (name NOT NULL);
INSERT INTO emitters.people VALUES ('ann', '1990-05-01', 170), ('bob', NULL, NULL);

-- A foreign column's own options, which 17 and later write as `ALTER FOREIGN
-- TABLE ONLY ... OPTIONS`, and a dropped foreign column, which 13-16 drop as
-- `ALTER FOREIGN TABLE ONLY` under `--binary-upgrade`.
CREATE EXTENSION IF NOT EXISTS file_fdw;
CREATE SERVER emitters_files FOREIGN DATA WRAPPER file_fdw;
COPY (VALUES (1, 'one')) TO '/tmp/emitters_external.tsv';
CREATE FOREIGN TABLE emitters.external (id integer, gone text, label text)
    SERVER emitters_files OPTIONS (filename '/tmp/emitters_external.tsv');
ALTER FOREIGN TABLE emitters.external ALTER COLUMN label OPTIONS (force_not_null 'true');
ALTER FOREIGN TABLE emitters.external DROP COLUMN gone;

-- dumpConstraint: a `UNIQUE` constraint with included columns, deferred,
-- and one whose index is the table's replica identity, which it writes as its
-- own `ALTER TABLE ONLY ... REPLICA IDENTITY USING INDEX`.
CREATE TABLE emitters.keyed (
    id integer NOT NULL,
    code text,
    note text,
    CONSTRAINT keyed_id_key UNIQUE (id),
    CONSTRAINT keyed_code_key UNIQUE (code) INCLUDE (note) DEFERRABLE INITIALLY DEFERRED
);
ALTER TABLE emitters.keyed REPLICA IDENTITY USING INDEX keyed_id_key;
INSERT INTO emitters.keyed VALUES (1, 'a', 'first'), (2, NULL, NULL);
-- A comment holding a backslash, which `appendStringLiteralAH` writes as
-- `E'...'` with the backslash doubled where `standard_conforming_strings` is
-- off (`emitters/standard-conforming-strings-off`, I50).
COMMENT ON TABLE emitters.keyed IS 'keyed by C:\path';

-- dumpCollation: a libc collation whose `LC_COLLATE` and `LC_CTYPE` differ,
-- which it writes as two properties rather than `locale`; a column of it.
CREATE COLLATION emitters.c_split (lc_collate = 'C', lc_ctype = 'POSIX');
CREATE TABLE emitters.collated (id integer, label text COLLATE emitters.c_split);
INSERT INTO emitters.collated VALUES (1, 'b'), (2, 'a');

-- dumpTableData_insert under `--inserts`: a table with no columns, whose rows
-- it writes as `DEFAULT VALUES`, and one with an identity column `GENERATED
-- ALWAYS`, whose rows it writes `OVERRIDING SYSTEM VALUE`.
CREATE TABLE emitters.no_columns ();
INSERT INTO emitters.no_columns DEFAULT VALUES;
INSERT INTO emitters.no_columns DEFAULT VALUES;
CREATE TABLE emitters.identified (id integer GENERATED ALWAYS AS IDENTITY, label text);
INSERT INTO emitters.identified (label) VALUES ('one'), (NULL);

-- buildACLCommands: a privilege granted `WITH GRANT OPTION` to a role, and the
-- privilege that role grants on, which it writes between `SET SESSION
-- AUTHORIZATION` and `RESET SESSION AUTHORIZATION`. The roles are the cluster
-- script's (fixture_schema_emitters_cluster.sql).
GRANT USAGE ON SCHEMA emitters TO emitters_grantor;
GRANT SELECT ON TABLE emitters.keyed TO emitters_grantor WITH GRANT OPTION;
SET ROLE emitters_grantor;
GRANT SELECT ON TABLE emitters.keyed TO emitters_member;
RESET ROLE;

-- The filter file `pg_dumpall --filter` reads at 17 and later
-- (scripts/generate_fixtures.py, `emitters/dumpall-filter`).
COPY (VALUES ('exclude database postgres')) TO '/tmp/emitters_dumpall_filter.txt';
