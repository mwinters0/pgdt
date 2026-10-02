--
-- PostgreSQL database dump
--

\restrict XSJz0fbetRu0MQixXdizgCskC68jz2cB7lcRKf7vUmp67wV8iloQA0TpNZPaVkx

-- Dumped from database version 18.6 (Debian 18.6-1.pgdg13+2)
-- Dumped by pg_dump version 18.6 (Debian 18.6-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA objects;


ALTER SCHEMA objects OWNER TO postgres;

--
-- Name: c_collation; Type: COLLATION; Schema: objects; Owner: postgres
--

CREATE COLLATION objects.c_collation (provider = libc, locale = 'C');


ALTER COLLATION objects.c_collation OWNER TO postgres;

--
-- Name: color_cmyk; Type: TYPE; Schema: objects; Owner: postgres
--

CREATE TYPE objects.color_cmyk AS ENUM (
    'cyan',
    'magenta',
    'yellow',
    'black'
);


ALTER TYPE objects.color_cmyk OWNER TO postgres;

--
-- Name: color_rgb; Type: TYPE; Schema: objects; Owner: postgres
--

CREATE TYPE objects.color_rgb AS ENUM (
    'red',
    'green',
    'blue'
);


ALTER TYPE objects.color_rgb OWNER TO postgres;

--
-- Name: rgb_to_cmyk(objects.color_rgb); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.rgb_to_cmyk(objects.color_rgb) RETURNS objects.color_cmyk
    LANGUAGE sql IMMUTABLE
    AS $_$
    SELECT CASE $1::text
        WHEN 'red' THEN 'magenta'::objects.color_cmyk
        WHEN 'green' THEN 'yellow'::objects.color_cmyk
        ELSE 'cyan'::objects.color_cmyk
    END;
$_$;


ALTER FUNCTION objects.rgb_to_cmyk(objects.color_rgb) OWNER TO postgres;

--
-- Name: log_widget_change(); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.log_widget_change() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    INSERT INTO objects.widget_audit (widget_id) VALUES (NEW.id);
    RETURN NEW;
END;
$$;


ALTER FUNCTION objects.log_widget_change() OWNER TO postgres;

--
-- Name: no_public_execute(); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.no_public_execute() RETURNS integer
    LANGUAGE sql IMMUTABLE
    AS $$ SELECT 1; $$;


ALTER FUNCTION objects.no_public_execute() OWNER TO postgres;

--
-- Name: noop_event_trigger(); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.noop_event_trigger() RETURNS event_trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
END;
$$;


ALTER FUNCTION objects.noop_event_trigger() OWNER TO postgres;

--
-- Name: my_sum(integer); Type: AGGREGATE; Schema: objects; Owner: postgres
--

CREATE AGGREGATE objects.my_sum(integer) (
    SFUNC = int4pl,
    STYPE = integer,
    INITCOND = '0'
);


ALTER AGGREGATE objects.my_sum(integer) OWNER TO postgres;

--
-- Name: latin1_to_utf8; Type: CONVERSION; Schema: objects; Owner: postgres
--

CREATE CONVERSION objects.latin1_to_utf8 FOR 'LATIN1' TO 'UTF8' FROM iso8859_1_to_utf8;


ALTER CONVERSION objects.latin1_to_utf8 OWNER TO postgres;

--
-- Name: simple_dict; Type: TEXT SEARCH DICTIONARY; Schema: objects; Owner: postgres
--

CREATE TEXT SEARCH DICTIONARY objects.simple_dict (
    TEMPLATE = pg_catalog.simple );


ALTER TEXT SEARCH DICTIONARY objects.simple_dict OWNER TO postgres;

--
-- Name: simple_config; Type: TEXT SEARCH CONFIGURATION; Schema: objects; Owner: postgres
--

CREATE TEXT SEARCH CONFIGURATION objects.simple_config (
    PARSER = pg_catalog."default" );

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR asciiword WITH objects.simple_dict;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR word WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR numword WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR email WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR url WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR host WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR sfloat WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR version WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR hword_numpart WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR hword_part WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR hword_asciipart WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR numhword WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR asciihword WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR hword WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR url_path WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR file WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR "float" WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR "int" WITH simple;

ALTER TEXT SEARCH CONFIGURATION objects.simple_config
    ADD MAPPING FOR uint WITH simple;


ALTER TEXT SEARCH CONFIGURATION objects.simple_config OWNER TO postgres;

SET default_tablespace = '';

--
-- Name: events; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events (
    id bigint NOT NULL,
    event_date date NOT NULL,
    payload text
)
PARTITION BY RANGE (event_date);


ALTER TABLE objects.events OWNER TO postgres;

--
-- Name: events_2024; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events_2024 (
    id bigint CONSTRAINT events_id_not_null NOT NULL,
    event_date date CONSTRAINT events_event_date_not_null NOT NULL,
    payload text
);


ALTER TABLE objects.events_2024 OWNER TO postgres;

--
-- Name: imported; Type: FOREIGN TABLE; Schema: objects; Owner: postgres
--

CREATE FOREIGN TABLE objects.imported (
    id integer NOT NULL,
    born date,
    label text
)
SERVER objects_files
OPTIONS (
    filename '/tmp/objects_imported.tsv'
);


ALTER FOREIGN TABLE objects.imported OWNER TO postgres;

--
-- Name: orders; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.orders (
    id integer NOT NULL,
    widget_id integer,
    quantity integer DEFAULT 1 NOT NULL
);


ALTER TABLE objects.orders OWNER TO postgres;

--
-- Name: orders_id_seq; Type: SEQUENCE; Schema: objects; Owner: postgres
--

CREATE SEQUENCE objects.orders_id_seq
    AS integer
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE objects.orders_id_seq OWNER TO postgres;

--
-- Name: orders_id_seq; Type: SEQUENCE OWNED BY; Schema: objects; Owner: postgres
--

ALTER SEQUENCE objects.orders_id_seq OWNED BY objects.orders.id;


--
-- Name: price$$list; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects."price$$list" (
    id integer NOT NULL,
    note text DEFAULT 'costs $x$ here'::text
);


ALTER TABLE objects."price$$list" OWNER TO postgres;

--
-- Name: TABLE "price$$list"; Type: COMMENT; Schema: objects; Owner: postgres
--

COMMENT ON TABLE objects."price$$list" IS 'priced in $$';


--
-- Name: secrets; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.secrets (
    owner_role text NOT NULL,
    payload text NOT NULL
);


ALTER TABLE objects.secrets OWNER TO postgres;

--
-- Name: standalone_seq; Type: SEQUENCE; Schema: objects; Owner: postgres
--

CREATE SEQUENCE objects.standalone_seq
    START WITH 100
    INCREMENT BY 5
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


ALTER SEQUENCE objects.standalone_seq OWNER TO postgres;

SET default_tablespace = fixture_ts;

--
-- Name: tablespaced_table; Type: TABLE; Schema: objects; Owner: postgres; Tablespace: fixture_ts
--

CREATE TABLE objects.tablespaced_table (
    id integer
);


ALTER TABLE objects.tablespaced_table OWNER TO postgres;

SET default_tablespace = '';

--
-- Name: widget_audit; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widget_audit (
    widget_id integer,
    changed_at timestamp with time zone DEFAULT now()
);


ALTER TABLE objects.widget_audit OWNER TO postgres;

--
-- Name: widgets; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widgets (
    id integer NOT NULL,
    label text DEFAULT 'unnamed'::text,
    created_at timestamp with time zone DEFAULT now()
);


ALTER TABLE objects.widgets OWNER TO postgres;

--
-- Name: TABLE widgets; Type: COMMENT; Schema: objects; Owner: postgres
--

COMMENT ON TABLE objects.widgets IS 'canonical widget catalog';


--
-- Name: COLUMN widgets.label; Type: COMMENT; Schema: objects; Owner: postgres
--

COMMENT ON COLUMN objects.widgets.label IS 'human-readable widget name';


--
-- Name: widget_orders; Type: VIEW; Schema: objects; Owner: postgres
--

CREATE VIEW objects.widget_orders AS
 SELECT w.id,
    w.label,
    o.quantity
   FROM (objects.widgets w
     JOIN objects.orders o ON ((o.widget_id = w.id)));


ALTER VIEW objects.widget_orders OWNER TO postgres;

--
-- Name: widget_totals; Type: MATERIALIZED VIEW; Schema: objects; Owner: postgres
--

CREATE MATERIALIZED VIEW objects.widget_totals AS
 SELECT widget_id,
    sum(quantity) AS total_quantity
   FROM objects.orders
  GROUP BY widget_id
  WITH NO DATA;


ALTER MATERIALIZED VIEW objects.widget_totals OWNER TO postgres;

--
-- Name: widgets_deleted_log; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widgets_deleted_log (
    id integer,
    deleted_at timestamp with time zone DEFAULT now()
);


ALTER TABLE objects.widgets_deleted_log OWNER TO postgres;

--
-- Name: events_2024; Type: TABLE ATTACH; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.events ATTACH PARTITION objects.events_2024 FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');


--
-- Name: orders id; Type: DEFAULT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.orders ALTER COLUMN id SET DEFAULT nextval('objects.orders_id_seq'::regclass);


--
-- Name: widget_orders quantity; Type: DEFAULT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.widget_orders ALTER COLUMN quantity SET DEFAULT 0;


--
-- Data for Name: events_2024; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.events_2024 (id, event_date, payload) FROM stdin;
1	2024-06-01	first
2	2024-12-31	second
\.


--
-- Data for Name: orders; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.orders (id, widget_id, quantity) FROM stdin;
1	1	3
2	2	1
3	1	7
\.


--
-- Data for Name: price$$list; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects."price$$list" (id, note) FROM stdin;
1	costs $x$ here
2	costs $x$ here
\.


--
-- Data for Name: secrets; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.secrets (owner_role, payload) FROM stdin;
\.


--
-- Data for Name: tablespaced_table; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.tablespaced_table (id) FROM stdin;
\.


--
-- Data for Name: widget_audit; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widget_audit (widget_id, changed_at) FROM stdin;
\.


--
-- Data for Name: widgets; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widgets (id, label, created_at) FROM stdin;
1	alpha	2026-10-02 04:19:18.468135+00
2	beta	2026-10-02 04:19:18.468135+00
\.


--
-- Data for Name: widgets_deleted_log; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widgets_deleted_log (id, deleted_at) FROM stdin;
\.


--
-- Name: orders_id_seq; Type: SEQUENCE SET; Schema: objects; Owner: postgres
--

SELECT pg_catalog.setval('objects.orders_id_seq', 3, true);


--
-- Name: standalone_seq; Type: SEQUENCE SET; Schema: objects; Owner: postgres
--

SELECT pg_catalog.setval('objects.standalone_seq', 100, true);


--
-- Name: orders orders_pkey; Type: CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.orders
    ADD CONSTRAINT orders_pkey PRIMARY KEY (id);


--
-- Name: price$$list price$$list_pkey; Type: CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects."price$$list"
    ADD CONSTRAINT "price$$list_pkey" PRIMARY KEY (id);


--
-- Name: widgets widgets_pkey; Type: CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.widgets
    ADD CONSTRAINT widgets_pkey PRIMARY KEY (id);


--
-- Name: events_event_date_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX events_event_date_idx ON ONLY objects.events USING btree (event_date);


--
-- Name: events_2024_event_date_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX events_2024_event_date_idx ON objects.events_2024 USING btree (event_date);


--
-- Name: widgets_label_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX widgets_label_idx ON objects.widgets USING btree (label);


--
-- Name: events_2024_event_date_idx; Type: INDEX ATTACH; Schema: objects; Owner: postgres
--

ALTER INDEX objects.events_event_date_idx ATTACH PARTITION objects.events_2024_event_date_idx;


--
-- Name: orders_stats; Type: STATISTICS; Schema: objects; Owner: postgres
--

CREATE STATISTICS objects.orders_stats (dependencies) ON widget_id, quantity FROM objects.orders;


ALTER STATISTICS objects.orders_stats OWNER TO postgres;

--
-- Name: widgets widgets_log_delete; Type: RULE; Schema: objects; Owner: postgres
--

CREATE RULE widgets_log_delete AS
    ON DELETE TO objects.widgets DO  INSERT INTO objects.widgets_deleted_log (id)
  VALUES (old.id);


--
-- Name: widgets widgets_audit_trigger; Type: TRIGGER; Schema: objects; Owner: postgres
--

CREATE TRIGGER widgets_audit_trigger AFTER INSERT OR UPDATE ON objects.widgets FOR EACH ROW EXECUTE FUNCTION objects.log_widget_change();


--
-- Name: orders orders_widget_id_fkey; Type: FK CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.orders
    ADD CONSTRAINT orders_widget_id_fkey FOREIGN KEY (widget_id) REFERENCES objects.widgets(id);


--
-- Name: secrets; Type: ROW SECURITY; Schema: objects; Owner: postgres
--

ALTER TABLE objects.secrets ENABLE ROW LEVEL SECURITY;

--
-- Name: secrets secrets_owner_only; Type: POLICY; Schema: objects; Owner: postgres
--

CREATE POLICY secrets_owner_only ON objects.secrets USING ((owner_role = CURRENT_USER));


--
-- Name: FUNCTION no_public_execute(); Type: ACL; Schema: objects; Owner: postgres
--

REVOKE ALL ON FUNCTION objects.no_public_execute() FROM PUBLIC;


--
-- Name: TABLE events; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.events TO fixture_reader;


--
-- Name: TABLE events_2024; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.events_2024 TO fixture_reader;


--
-- Name: TABLE imported; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.imported TO fixture_reader;


--
-- Name: TABLE "price$$list"; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects."price$$list" TO fixture_reader;


--
-- Name: TABLE tablespaced_table; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.tablespaced_table TO fixture_reader;


--
-- Name: TABLE widgets; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.widgets TO fixture_reader;
GRANT SELECT ON TABLE objects.widgets TO PUBLIC;


--
-- Name: DEFAULT PRIVILEGES FOR TABLES; Type: DEFAULT ACL; Schema: objects; Owner: postgres
--

ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA objects GRANT SELECT ON TABLES TO fixture_reader;


--
-- Name: widget_totals; Type: MATERIALIZED VIEW DATA; Schema: objects; Owner: postgres
--

REFRESH MATERIALIZED VIEW objects.widget_totals;


--
-- PostgreSQL database dump complete
--

\unrestrict XSJz0fbetRu0MQixXdizgCskC68jz2cB7lcRKf7vUmp67wV8iloQA0TpNZPaVkx

