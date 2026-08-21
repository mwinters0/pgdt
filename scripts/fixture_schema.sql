-- Synthetic fixture schema/data for pg_dump compatibility testing.
--
-- Deliberately not koji-derived: covers the edge-case breadth called out in
-- docs/design/mvp.md (multiple schemas, NULLs, an empty table, COPY TEXT
-- escaping of newlines/tabs/backslashes/quotes, and a data value containing
-- a COPY-directive-like substring mid-line) rather than koji's specific
-- structure.

CREATE SCHEMA IF NOT EXISTS logs;

CREATE TABLE public.widgets (
    id integer PRIMARY KEY,
    name text,
    description text,
    is_active boolean,
    created_at timestamptz
);

INSERT INTO public.widgets (id, name, description, is_active, created_at) VALUES
    (1, 'alpha', 'a simple widget', true, '2024-01-01 00:00:00+00'),
    (2, 'beta', NULL, false, '2024-01-02 00:00:00+00'),
    (3, 'gamma', E'multi\nline\tdescription with a literal backslash \\ and a quote '' inside', true, NULL),
    (4, 'delta', 'contains a COPY-like phrase: COPY public.widgets (id, name) TO stdout; -- not a real directive', true, '2024-01-04 00:00:00+00'),
    (5, '', 'empty name above', NULL, '2024-01-05 00:00:00+00');

CREATE TABLE public.empty_table (
    id integer PRIMARY KEY,
    value text
);

CREATE TABLE logs.events (
    event_id bigint PRIMARY KEY,
    widget_id integer REFERENCES public.widgets(id),
    message text,
    logged_at timestamptz DEFAULT now()
);

INSERT INTO logs.events (event_id, widget_id, message) VALUES
    (100, 1, 'created'),
    (101, 2, NULL),
    (102, 3, E'updated\twith a tab char');
