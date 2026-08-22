--
-- Hand-written scanner fixture. NOT pg_dump output: it is deliberately
-- deterministic (no generated timestamps) so snapshots are stable, and it
-- packs the edge cases from docs/design/mvp.md into one small file.
--

\restrict fZqK1exampleTOKENnotReal

SET client_encoding = 'UTF8';
SELECT pg_catalog.set_config('search_path', '', false);

--
-- A dollar-quoted body whose lines start with COPY. Neither matches the
-- header grammar, so both are skipped as ordinary SQL.
--

CREATE FUNCTION public.sample_fn() RETURNS void
    LANGUAGE plpgsql
    AS $$
BEGIN
COPY public.widgets TO stdout;
COPY public.widgets FROM stdin WITH (FORMAT csv);
END;
$$;

--
-- Data for Name: empty_table; Type: TABLE DATA; Schema: public
--

COPY public.empty_table (id, value) FROM stdin;
\.


--
-- Data for Name: widgets; Type: TABLE DATA; Schema: public
--

COPY public.widgets (id, name, description, created_at) FROM stdin;
1	alpha	a simple widget	2024-01-01 00:00:00+00
2	beta	\N	2024-01-02 00:00:00+00
3	gamma	multi\nline\twith a backslash \\ inside	\N
4	delta	contains a COPY-like phrase: COPY public.widgets (id) FROM stdin; -- not a directive	2024-01-04 00:00:00+00
5		empty name to the left	2024-01-05 00:00:00+00
6	epsilon	carriage\rreturn, octal \101, hex \x42	2024-01-06 00:00:00+00
\.


--
-- Data for Name: Odd Table; Type: TABLE DATA; Schema: My Schema
--

COPY "My Schema"."Odd Table" ("Id", "select") FROM stdin;
1	quoted identifiers
\.


--
-- Data for Name: no_column_list; Type: TABLE DATA; Schema: public
--

COPY public.no_column_list FROM stdin;
\\.
just a value
\.


\unrestrict fZqK1exampleTOKENnotReal

--
-- PostgreSQL database dump complete
--
