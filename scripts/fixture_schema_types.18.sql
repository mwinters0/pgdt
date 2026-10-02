-- The `types` schema's v18 sidecar: DDL 13-17 refuse, loaded after
-- fixture_schema_types.sql at 18 and above (generate_fixtures.schema_files).
--
-- The three column shapes v18's `dumpTableSchema` writes between a column's
-- type and its `COLLATE` clause (docs/design/postgres-invariants.md, "I37"):
-- a named not-null constraint, `CONSTRAINT <name> NOT NULL`; a not-null
-- constraint carrying `NO INHERIT`; and a virtual generated column,
-- `GENERATED ALWAYS AS (expr)` with no `STORED`, which `COPY` never lists
-- (I5). Each is collated, so the clause is displaced behind the shape, and
-- each virtual column has a non-text type, so a resolution that dropped one
-- would show. The comment is on the named constraint, which v18 alone dumps
-- as its own `COMMENT ON CONSTRAINT`.
CREATE TABLE public.t_v18_columns (
    id integer PRIMARY KEY,
    v_named text COLLATE "C" CONSTRAINT t_v18_named_present NOT NULL,
    v_no_inherit text COLLATE "C" NOT NULL NO INHERIT,
    v_virtual text COLLATE "C" GENERATED ALWAYS AS (upper(v_named)) VIRTUAL,
    v_virtual_len integer GENERATED ALWAYS AS (length(v_no_inherit)) VIRTUAL,
    v_after date
);
COMMENT ON CONSTRAINT t_v18_named_present ON public.t_v18_columns IS 'a named not-null constraint';
INSERT INTO public.t_v18_columns (id, v_named, v_no_inherit, v_after) VALUES
    (1, 'alpha', 'beta', '2024-01-01'),
    (2, 'gamma', '', NULL);
