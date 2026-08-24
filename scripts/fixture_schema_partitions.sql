-- Partitioned-table fixture schema: the one shape in which a single
-- `COPY <name> FROM stdin;` header owns MORE THAN ONE block in a single
-- pg_dump output. See I2 in docs/design/postgres-invariants.md.
--
-- Backs docs/design/roadmap-phase3-object-inventory.md, "Mapping and
-- streaming are separate passes": a query may stop scanning once the
-- queried table's block closes, EXCEPT for blocks carrying pg_dump's
-- `-- load via partition root <root>` marker, which is exactly this shape.
-- Without this schema that rule would ship tested only against
-- hand-written strings.
--
-- Two independent ways to reach it, and both are exercised:
--
--   1. `--load-via-partition-root`, the explicit flag (this schema's second
--      flag set). Applies to every partitioned table, `evt` included.
--   2. NO FLAG AT ALL. `forcePartitionRootLoad()` turns the mode on by
--      itself for a table hash-partitioned on an enum column, because the
--      hash codes depend on enum value OIDs that don't survive a
--      dump-and-reload. Present in all six routine majors (13-18), so the
--      `default` flag set produces the multi-block shape on every one.
--      That is `feel` below.
--
-- The partition names matter. TABLE DATA entries sort by the PARTITION's
-- own name (`DOTypeNameCompare` in pg_dump_sort.c), not the root's, so an
-- unrelated table whose name sorts between two partition names is emitted
-- between their blocks. `evt_m` and `feel_m` exist only to sit in that
-- gap, which is what makes "read forward until the next block names a
-- different table" an unsound way to enumerate a name's blocks. Keep the
-- _a / _m / _z naming: it is the whole point of these three tables.

-- ---------------------------------------------------------------------
-- LIST partitioning. Dumps under the partitions' own names by default;
-- under `public.evt` (twice) with --load-via-partition-root.
-- ---------------------------------------------------------------------
CREATE TABLE public.evt (id integer, region text) PARTITION BY LIST (region);
CREATE TABLE public.evt_a PARTITION OF public.evt FOR VALUES IN ('a');
CREATE TABLE public.evt_z PARTITION OF public.evt FOR VALUES IN ('z');

-- Sorts between evt_a and evt_z, so its block lands between theirs.
CREATE TABLE public.evt_m (note text);

INSERT INTO public.evt VALUES (1, 'a'), (2, 'a'), (3, 'z');
INSERT INTO public.evt_m VALUES ('unrelated');

-- ---------------------------------------------------------------------
-- HASH partitioning on an enum column: pg_dump forces
-- load-via-partition-root here with no flag, so BOTH flag sets emit two
-- `COPY public.feel` blocks, each preceded by the marker line.
-- ---------------------------------------------------------------------
CREATE TYPE public.mood AS ENUM ('sad', 'ok', 'happy');

CREATE TABLE public.feel (id integer, m public.mood) PARTITION BY HASH (m);
CREATE TABLE public.feel_a PARTITION OF public.feel
    FOR VALUES WITH (MODULUS 2, REMAINDER 0);
CREATE TABLE public.feel_z PARTITION OF public.feel
    FOR VALUES WITH (MODULUS 2, REMAINDER 1);

-- Sorts between feel_a and feel_z, same reason as evt_m.
CREATE TABLE public.feel_m (note text);

INSERT INTO public.feel VALUES (1, 'sad'), (2, 'ok'), (3, 'happy');
INSERT INTO public.feel_m VALUES ('unrelated');

-- ---------------------------------------------------------------------
-- A partitioned table with an EMPTY partition. An empty partition still
-- gets its own TABLE DATA entry, so the root name owns a block with zero
-- rows -- a data span containing no data, which the map must tile like
-- any other.
-- ---------------------------------------------------------------------
CREATE TABLE public.spread (id integer, m public.mood) PARTITION BY HASH (m);
CREATE TABLE public.spread_a PARTITION OF public.spread
    FOR VALUES WITH (MODULUS 2, REMAINDER 0);
CREATE TABLE public.spread_z PARTITION OF public.spread
    FOR VALUES WITH (MODULUS 2, REMAINDER 1);

-- Only values hashing to one side, leaving the other partition empty.
INSERT INTO public.spread SELECT 1, 'sad';
