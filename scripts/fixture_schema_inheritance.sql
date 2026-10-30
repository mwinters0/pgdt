-- Inheritance fixture schema: a table read with its descendants
-- (`docs/design/roadmap-P38-descendants.md`, "The generic mechanism"), the
-- shapes `INHERITS` and declarative partitioning give a parent's rows that
-- `fixture_schema_partitions.sql` does not. Each flag set's restore is
-- answered by the server per table, read through and `ONLY`
-- (`generate_fixtures.py`'s `write_answers`).
--
-- `pg_dump` writes an inheritance child as `CREATE TABLE <child> (<its own
-- columns>) INHERITS (<parents>)`, inherited columns omitted, and its `COPY`
-- header lists every column in the child's own order: inherited first, then
-- its own.

-- ---------------------------------------------------------------------
-- Inheritance three levels deep, a parent with rows of its own at every
-- level, and children declaring columns of their own.
-- ---------------------------------------------------------------------
CREATE TABLE public.animal (id integer, name text);
CREATE TABLE public.mammal (legs integer) INHERITS (public.animal);
CREATE TABLE public.dog (breed text) INHERITS (public.mammal);

INSERT INTO public.animal VALUES (1, 'generic');
INSERT INTO public.mammal VALUES (2, 'whale', 0);
INSERT INTO public.dog VALUES (3, 'rex', 4, 'collie'), (4, 'fido', 4, 'pug');

-- A child in another schema, so a descendant's name is not its parent's
-- schema; and an empty one.
CREATE SCHEMA kennel;
CREATE TABLE kennel.puppy (weeks integer) INHERITS (public.dog);
CREATE TABLE public.fossil () INHERITS (public.animal);

INSERT INTO kennel.puppy VALUES (5, 'bit', 4, 'pug', 8);

-- ---------------------------------------------------------------------
-- Multiple inheritance: `cat` is read under both parents. `pet` has no
-- rows of its own; `cat`'s columns are `mammal`'s, then `pet`'s.
-- ---------------------------------------------------------------------
CREATE TABLE public.pet (owner text);
CREATE TABLE public.cat () INHERITS (public.mammal, public.pet);

INSERT INTO public.cat VALUES (6, 'tom', 4, 'ann');

-- ---------------------------------------------------------------------
-- A child declaring a column its parent has: the two merge, so `pg_dump`
-- writes it among the child's own, and the child's column order is the
-- parent's columns first all the same.
-- ---------------------------------------------------------------------
CREATE TABLE public.bird (name text, wings integer) INHERITS (public.animal);

INSERT INTO public.bird VALUES (7, 'tweety', 2);

-- ---------------------------------------------------------------------
-- Declarative partitioning two levels deep: a partition itself
-- partitioned, and a leaf whose columns are attached in another order.
-- Range and list on integers and text, so no flag set but
-- `--load-via-partition-root` loads through the root.
-- ---------------------------------------------------------------------
CREATE TABLE public.reading (year integer, region text, value integer) PARTITION BY RANGE (year);
CREATE TABLE public.reading_2020 PARTITION OF public.reading
    FOR VALUES FROM (2020) TO (2021) PARTITION BY LIST (region);
CREATE TABLE public.reading_2020_north PARTITION OF public.reading_2020 FOR VALUES IN ('north');
CREATE TABLE public.reading_2020_south (value integer, region text, year integer);
ALTER TABLE public.reading_2020 ATTACH PARTITION public.reading_2020_south FOR VALUES IN ('south');
CREATE TABLE public.reading_2021 PARTITION OF public.reading FOR VALUES FROM (2021) TO (2022);

INSERT INTO public.reading VALUES
    (2020, 'north', 1), (2020, 'south', 2), (2020, 'south', 3), (2021, 'north', 4);
