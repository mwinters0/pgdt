--
-- PostgreSQL database dump
--

\restrict rGzEeF9XLl7UhP2Fx4XmuYZqN7amV00UH82vthFwKR6L7Kpt5aEYiJUgHxmHoF7

-- Dumped from database version 16.15 (Debian 16.15-1.pgdg13+2)
-- Dumped by pg_dump version 16.15 (Debian 16.15-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = on;

SET SESSION AUTHORIZATION 'postgres';

--
-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA objects;


--
-- Name: c_collation; Type: COLLATION; Schema: objects; Owner: postgres
--

CREATE COLLATION objects.c_collation (provider = libc, locale = 'C');


SET SESSION AUTHORIZATION DEFAULT;

--
-- Name: file_fdw; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS file_fdw WITH SCHEMA public;


--
-- Name: EXTENSION file_fdw; Type: COMMENT; Schema: -; Owner: 
--

COMMENT ON EXTENSION file_fdw IS 'foreign-data wrapper for flat file access';


--
-- Name: postgres_fdw; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS postgres_fdw WITH SCHEMA public;


--
-- Name: EXTENSION postgres_fdw; Type: COMMENT; Schema: -; Owner: 
--

COMMENT ON EXTENSION postgres_fdw IS 'foreign-data wrapper for remote PostgreSQL servers';


SET SESSION AUTHORIZATION 'postgres';

--
-- Name: color_cmyk; Type: TYPE; Schema: objects; Owner: postgres
--

CREATE TYPE objects.color_cmyk AS ENUM (
    'cyan',
    'magenta',
    'yellow',
    'black'
);


--
-- Name: color_rgb; Type: TYPE; Schema: objects; Owner: postgres
--

CREATE TYPE objects.color_rgb AS ENUM (
    'red',
    'green',
    'blue'
);


--
-- Name: rgb_to_cmyk(objects.color_rgb); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.rgb_to_cmyk(objects.color_rgb) RETURNS objects.color_cmyk
    LANGUAGE sql IMMUTABLE
    AS '
    SELECT CASE $1::text
        WHEN ''red'' THEN ''magenta''::objects.color_cmyk
        WHEN ''green'' THEN ''yellow''::objects.color_cmyk
        ELSE ''cyan''::objects.color_cmyk
    END;
';


SET SESSION AUTHORIZATION DEFAULT;

--
-- Name: CAST (objects.color_rgb AS objects.color_cmyk); Type: CAST; Schema: -; Owner: -
--

CREATE CAST (objects.color_rgb AS objects.color_cmyk) WITH FUNCTION objects.rgb_to_cmyk(objects.color_rgb);


SET SESSION AUTHORIZATION 'postgres';

--
-- Name: log_widget_change(); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.log_widget_change() RETURNS trigger
    LANGUAGE plpgsql
    AS '
BEGIN
    INSERT INTO objects.widget_audit (widget_id) VALUES (NEW.id);
    RETURN NEW;
END;
';


--
-- Name: no_public_execute(); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.no_public_execute() RETURNS integer
    LANGUAGE sql IMMUTABLE
    AS ' SELECT 1; ';


--
-- Name: noop_event_trigger(); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.noop_event_trigger() RETURNS event_trigger
    LANGUAGE plpgsql
    AS '
BEGIN
END;
';


--
-- Name: my_sum(integer); Type: AGGREGATE; Schema: objects; Owner: postgres
--

CREATE AGGREGATE objects.my_sum(integer) (
    SFUNC = int4pl,
    STYPE = integer,
    INITCOND = '0'
);


--
-- Name: latin1_to_utf8; Type: CONVERSION; Schema: objects; Owner: postgres
--

CREATE CONVERSION objects.latin1_to_utf8 FOR 'LATIN1' TO 'UTF8' FROM iso8859_1_to_utf8;


--
-- Name: simple_dict; Type: TEXT SEARCH DICTIONARY; Schema: objects; Owner: postgres
--

CREATE TEXT SEARCH DICTIONARY objects.simple_dict (
    TEMPLATE = pg_catalog.simple );


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


--
-- Name: objects_files; Type: SERVER; Schema: -; Owner: postgres
--

CREATE SERVER objects_files FOREIGN DATA WRAPPER file_fdw;


--
-- Name: objects_remote; Type: SERVER; Schema: -; Owner: postgres
--

CREATE SERVER objects_remote FOREIGN DATA WRAPPER postgres_fdw OPTIONS (
    dbname 'nonexistent',
    host 'nonexistent'
);


--
-- Name: USER MAPPING postgres SERVER objects_remote; Type: USER MAPPING; Schema: -; Owner: postgres
--

CREATE USER MAPPING FOR postgres SERVER objects_remote OPTIONS (
    password 'unused',
    "user" 'nobody'
);


--
-- Name: events; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events (
    id bigint NOT NULL,
    event_date date NOT NULL,
    payload text
)
PARTITION BY RANGE (event_date);


SET default_table_access_method = heap;

--
-- Name: events_2024; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events_2024 (
    id bigint NOT NULL,
    event_date date NOT NULL,
    payload text
);


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


--
-- Name: orders; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.orders (
    id integer NOT NULL,
    widget_id integer,
    quantity integer DEFAULT 1 NOT NULL
);


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


--
-- Name: standalone_seq; Type: SEQUENCE; Schema: objects; Owner: postgres
--

CREATE SEQUENCE objects.standalone_seq
    START WITH 100
    INCREMENT BY 5
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;


--
-- Name: tablespaced_table; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.tablespaced_table (
    id integer
);


--
-- Name: widget_audit; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widget_audit (
    widget_id integer,
    changed_at timestamp with time zone DEFAULT now()
);


--
-- Name: widgets; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widgets (
    id integer NOT NULL,
    label text DEFAULT 'unnamed'::text,
    created_at timestamp with time zone DEFAULT now()
);


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


--
-- Name: widget_totals; Type: MATERIALIZED VIEW; Schema: objects; Owner: postgres
--

CREATE MATERIALIZED VIEW objects.widget_totals AS
 SELECT widget_id,
    sum(quantity) AS total_quantity
   FROM objects.orders
  GROUP BY widget_id
  WITH NO DATA;


--
-- Name: widgets_deleted_log; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widgets_deleted_log (
    id integer,
    deleted_at timestamp with time zone DEFAULT now()
);


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

INSERT INTO objects.events_2024 (id, event_date, payload) VALUES (1, '2024-06-01', 'first');
INSERT INTO objects.events_2024 (id, event_date, payload) VALUES (2, '2024-12-31', 'second');


--
-- Data for Name: orders; Type: TABLE DATA; Schema: objects; Owner: postgres
--

INSERT INTO objects.orders (id, widget_id, quantity) VALUES (1, 1, 3);
INSERT INTO objects.orders (id, widget_id, quantity) VALUES (2, 2, 1);
INSERT INTO objects.orders (id, widget_id, quantity) VALUES (3, 1, 7);


--
-- Data for Name: price$$list; Type: TABLE DATA; Schema: objects; Owner: postgres
--

INSERT INTO objects."price$$list" (id, note) VALUES (1, 'costs $x$ here');
INSERT INTO objects."price$$list" (id, note) VALUES (2, 'costs $x$ here');


--
-- Data for Name: secrets; Type: TABLE DATA; Schema: objects; Owner: postgres
--



--
-- Data for Name: tablespaced_table; Type: TABLE DATA; Schema: objects; Owner: postgres
--



--
-- Data for Name: widget_audit; Type: TABLE DATA; Schema: objects; Owner: postgres
--



--
-- Data for Name: widgets; Type: TABLE DATA; Schema: objects; Owner: postgres
--

INSERT INTO objects.widgets (id, label, created_at) VALUES (1, 'alpha', '2026-10-02 04:20:44.77992+00');
INSERT INTO objects.widgets (id, label, created_at) VALUES (2, 'beta', '2026-10-02 04:20:44.77992+00');


--
-- Data for Name: widgets_deleted_log; Type: TABLE DATA; Schema: objects; Owner: postgres
--



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
-- Name: objects_ddl_log; Type: EVENT TRIGGER; Schema: -; Owner: postgres
--

CREATE EVENT TRIGGER objects_ddl_log ON ddl_command_start
   EXECUTE FUNCTION objects.noop_event_trigger();

ALTER EVENT TRIGGER objects_ddl_log DISABLE;


--
-- Name: widget_totals; Type: MATERIALIZED VIEW DATA; Schema: objects; Owner: postgres
--

REFRESH MATERIALIZED VIEW objects.widget_totals;


--
-- PostgreSQL database dump complete
--

\unrestrict rGzEeF9XLl7UhP2Fx4XmuYZqN7amV00UH82vthFwKR6L7Kpt5aEYiJUgHxmHoF7

