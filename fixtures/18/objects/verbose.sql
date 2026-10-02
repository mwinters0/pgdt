--
-- PostgreSQL database dump
--

\restrict QZe5MJC0ildIo08PQ1KHgHgMdvecSKA9nA3fiuWuy36Egd0HqcbBbvc4PFBcYjN

-- Dumped from database version 18.6 (Debian 18.6-1.pgdg13+2)
-- Dumped by pg_dump version 18.6 (Debian 18.6-1.pgdg13+2)

-- Started on 2026-10-02 02:59:21 UTC

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
-- TOC entry 8 (class 2615 OID 16710)
-- Name: objects; Type: SCHEMA; Schema: -; Owner: postgres
--

CREATE SCHEMA objects;


ALTER SCHEMA objects OWNER TO postgres;

--
-- TOC entry 2183 (class 3456 OID 16830)
-- Name: c_collation; Type: COLLATION; Schema: objects; Owner: postgres
--

CREATE COLLATION objects.c_collation (provider = libc, locale = 'C');


ALTER COLLATION objects.c_collation OWNER TO postgres;

--
-- TOC entry 3 (class 3079 OID 16800)
-- Name: file_fdw; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS file_fdw WITH SCHEMA public;


--
-- TOC entry 3565 (class 0 OID 0)
-- Dependencies: 3
-- Name: EXTENSION file_fdw; Type: COMMENT; Schema: -; Owner: 
--

COMMENT ON EXTENSION file_fdw IS 'foreign-data wrapper for flat file access';


--
-- TOC entry 2 (class 3079 OID 16790)
-- Name: postgres_fdw; Type: EXTENSION; Schema: -; Owner: -
--

CREATE EXTENSION IF NOT EXISTS postgres_fdw WITH SCHEMA public;


--
-- TOC entry 3566 (class 0 OID 0)
-- Dependencies: 2
-- Name: EXTENSION postgres_fdw; Type: COMMENT; Schema: -; Owner: 
--

COMMENT ON EXTENSION postgres_fdw IS 'foreign-data wrapper for remote PostgreSQL servers';


--
-- TOC entry 912 (class 1247 OID 16818)
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
-- TOC entry 909 (class 1247 OID 16810)
-- Name: color_rgb; Type: TYPE; Schema: objects; Owner: postgres
--

CREATE TYPE objects.color_rgb AS ENUM (
    'red',
    'green',
    'blue'
);


ALTER TYPE objects.color_rgb OWNER TO postgres;

--
-- TOC entry 244 (class 1255 OID 16827)
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
-- TOC entry 3364 (class 2605 OID 16828)
-- Name: CAST (objects.color_rgb AS objects.color_cmyk); Type: CAST; Schema: -; Owner: -
--

CREATE CAST (objects.color_rgb AS objects.color_cmyk) WITH FUNCTION objects.rgb_to_cmyk(objects.color_rgb);


--
-- TOC entry 236 (class 1255 OID 16754)
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
-- TOC entry 246 (class 1255 OID 16848)
-- Name: no_public_execute(); Type: FUNCTION; Schema: objects; Owner: postgres
--

CREATE FUNCTION objects.no_public_execute() RETURNS integer
    LANGUAGE sql IMMUTABLE
    AS $$ SELECT 1; $$;


ALTER FUNCTION objects.no_public_execute() OWNER TO postgres;

--
-- TOC entry 245 (class 1255 OID 16834)
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
-- TOC entry 922 (class 1255 OID 16829)
-- Name: my_sum(integer); Type: AGGREGATE; Schema: objects; Owner: postgres
--

CREATE AGGREGATE objects.my_sum(integer) (
    SFUNC = int4pl,
    STYPE = integer,
    INITCOND = '0'
);


ALTER AGGREGATE objects.my_sum(integer) OWNER TO postgres;

--
-- TOC entry 3134 (class 2607 OID 16833)
-- Name: latin1_to_utf8; Type: CONVERSION; Schema: objects; Owner: postgres
--

CREATE CONVERSION objects.latin1_to_utf8 FOR 'LATIN1' TO 'UTF8' FROM iso8859_1_to_utf8;


ALTER CONVERSION objects.latin1_to_utf8 OWNER TO postgres;

--
-- TOC entry 2088 (class 3600 OID 16831)
-- Name: simple_dict; Type: TEXT SEARCH DICTIONARY; Schema: objects; Owner: postgres
--

CREATE TEXT SEARCH DICTIONARY objects.simple_dict (
    TEMPLATE = pg_catalog.simple );


ALTER TEXT SEARCH DICTIONARY objects.simple_dict OWNER TO postgres;

--
-- TOC entry 2119 (class 3602 OID 16832)
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

--
-- TOC entry 2123 (class 1417 OID 16804)
-- Name: objects_files; Type: SERVER; Schema: -; Owner: postgres
--

CREATE SERVER objects_files FOREIGN DATA WRAPPER file_fdw;


ALTER SERVER objects_files OWNER TO postgres;

--
-- TOC entry 2122 (class 1417 OID 16798)
-- Name: objects_remote; Type: SERVER; Schema: -; Owner: postgres
--

CREATE SERVER objects_remote FOREIGN DATA WRAPPER postgres_fdw OPTIONS (
    dbname 'nonexistent',
    host 'nonexistent'
);


ALTER SERVER objects_remote OWNER TO postgres;

--
-- TOC entry 3568 (class 0 OID 0)
-- Name: USER MAPPING postgres SERVER objects_remote; Type: USER MAPPING; Schema: -; Owner: postgres
--

CREATE USER MAPPING FOR postgres SERVER objects_remote OPTIONS (
    password 'unused',
    "user" 'nobody'
);


SET default_tablespace = '';

--
-- TOC entry 231 (class 1259 OID 16776)
-- Name: events; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events (
    id bigint NOT NULL,
    event_date date NOT NULL,
    payload text
)
PARTITION BY RANGE (event_date);


ALTER TABLE objects.events OWNER TO postgres;

SET default_table_access_method = heap;

--
-- TOC entry 232 (class 1259 OID 16781)
-- Name: events_2024; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.events_2024 (
    id bigint CONSTRAINT events_id_not_null NOT NULL,
    event_date date CONSTRAINT events_event_date_not_null NOT NULL,
    payload text
);


ALTER TABLE objects.events_2024 OWNER TO postgres;

--
-- TOC entry 233 (class 1259 OID 16805)
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
-- TOC entry 224 (class 1259 OID 16722)
-- Name: orders; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.orders (
    id integer NOT NULL,
    widget_id integer,
    quantity integer DEFAULT 1 NOT NULL
);


ALTER TABLE objects.orders OWNER TO postgres;

--
-- TOC entry 223 (class 1259 OID 16721)
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
-- TOC entry 3572 (class 0 OID 0)
-- Dependencies: 223
-- Name: orders_id_seq; Type: SEQUENCE OWNED BY; Schema: objects; Owner: postgres
--

ALTER SEQUENCE objects.orders_id_seq OWNED BY objects.orders.id;


--
-- TOC entry 235 (class 1259 OID 16839)
-- Name: price$$list; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects."price$$list" (
    id integer NOT NULL,
    note text DEFAULT 'costs $x$ here'::text
);


ALTER TABLE objects."price$$list" OWNER TO postgres;

--
-- TOC entry 3573 (class 0 OID 0)
-- Dependencies: 235
-- Name: TABLE "price$$list"; Type: COMMENT; Schema: objects; Owner: postgres
--

COMMENT ON TABLE objects."price$$list" IS 'priced in $$';


--
-- TOC entry 230 (class 1259 OID 16761)
-- Name: secrets; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.secrets (
    owner_role text NOT NULL,
    payload text NOT NULL
);


ALTER TABLE objects.secrets OWNER TO postgres;

--
-- TOC entry 225 (class 1259 OID 16736)
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
-- TOC entry 234 (class 1259 OID 16836)
-- Name: tablespaced_table; Type: TABLE; Schema: objects; Owner: postgres; Tablespace: fixture_ts
--

CREATE TABLE objects.tablespaced_table (
    id integer
);


ALTER TABLE objects.tablespaced_table OWNER TO postgres;

SET default_tablespace = '';

--
-- TOC entry 228 (class 1259 OID 16750)
-- Name: widget_audit; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widget_audit (
    widget_id integer,
    changed_at timestamp with time zone DEFAULT now()
);


ALTER TABLE objects.widget_audit OWNER TO postgres;

--
-- TOC entry 222 (class 1259 OID 16711)
-- Name: widgets; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widgets (
    id integer NOT NULL,
    label text DEFAULT 'unnamed'::text,
    created_at timestamp with time zone DEFAULT now()
);


ALTER TABLE objects.widgets OWNER TO postgres;

--
-- TOC entry 3576 (class 0 OID 0)
-- Dependencies: 222
-- Name: TABLE widgets; Type: COMMENT; Schema: objects; Owner: postgres
--

COMMENT ON TABLE objects.widgets IS 'canonical widget catalog';


--
-- TOC entry 3577 (class 0 OID 0)
-- Dependencies: 222
-- Name: COLUMN widgets.label; Type: COMMENT; Schema: objects; Owner: postgres
--

COMMENT ON COLUMN objects.widgets.label IS 'human-readable widget name';


--
-- TOC entry 226 (class 1259 OID 16738)
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
-- TOC entry 227 (class 1259 OID 16742)
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
-- TOC entry 229 (class 1259 OID 16756)
-- Name: widgets_deleted_log; Type: TABLE; Schema: objects; Owner: postgres
--

CREATE TABLE objects.widgets_deleted_log (
    id integer,
    deleted_at timestamp with time zone DEFAULT now()
);


ALTER TABLE objects.widgets_deleted_log OWNER TO postgres;

--
-- TOC entry 3366 (class 0 OID 0)
-- Name: events_2024; Type: TABLE ATTACH; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.events ATTACH PARTITION objects.events_2024 FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');


--
-- TOC entry 3369 (class 2604 OID 16725)
-- Name: orders id; Type: DEFAULT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.orders ALTER COLUMN id SET DEFAULT nextval('objects.orders_id_seq'::regclass);


--
-- TOC entry 3371 (class 2604 OID 16749)
-- Name: widget_orders quantity; Type: DEFAULT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.widget_orders ALTER COLUMN quantity SET DEFAULT 0;


--
-- TOC entry 3553 (class 0 OID 16781)
-- Dependencies: 232
-- Data for Name: events_2024; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.events_2024 (id, event_date, payload) FROM stdin;
1	2024-06-01	first
2	2024-12-31	second
\.


--
-- TOC entry 3547 (class 0 OID 16722)
-- Dependencies: 224
-- Data for Name: orders; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.orders (id, widget_id, quantity) FROM stdin;
1	1	3
2	2	1
3	1	7
\.


--
-- TOC entry 3555 (class 0 OID 16839)
-- Dependencies: 235
-- Data for Name: price$$list; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects."price$$list" (id, note) FROM stdin;
1	costs $x$ here
2	costs $x$ here
\.


--
-- TOC entry 3552 (class 0 OID 16761)
-- Dependencies: 230
-- Data for Name: secrets; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.secrets (owner_role, payload) FROM stdin;
\.


--
-- TOC entry 3554 (class 0 OID 16836)
-- Dependencies: 234
-- Data for Name: tablespaced_table; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.tablespaced_table (id) FROM stdin;
\.


--
-- TOC entry 3550 (class 0 OID 16750)
-- Dependencies: 228
-- Data for Name: widget_audit; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widget_audit (widget_id, changed_at) FROM stdin;
\.


--
-- TOC entry 3545 (class 0 OID 16711)
-- Dependencies: 222
-- Data for Name: widgets; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widgets (id, label, created_at) FROM stdin;
1	alpha	2026-10-02 02:59:21.371423+00
2	beta	2026-10-02 02:59:21.371423+00
\.


--
-- TOC entry 3551 (class 0 OID 16756)
-- Dependencies: 229
-- Data for Name: widgets_deleted_log; Type: TABLE DATA; Schema: objects; Owner: postgres
--

COPY objects.widgets_deleted_log (id, deleted_at) FROM stdin;
\.


--
-- TOC entry 3579 (class 0 OID 0)
-- Dependencies: 223
-- Name: orders_id_seq; Type: SEQUENCE SET; Schema: objects; Owner: postgres
--

SELECT pg_catalog.setval('objects.orders_id_seq', 3, true);


--
-- TOC entry 3580 (class 0 OID 0)
-- Dependencies: 225
-- Name: standalone_seq; Type: SEQUENCE SET; Schema: objects; Owner: postgres
--

SELECT pg_catalog.setval('objects.standalone_seq', 100, true);


--
-- TOC entry 3556 (class 2613 OID 16849)
-- Name: 16849; Type: BLOB METADATA; Schema: -; Owner: postgres
--

SELECT pg_catalog.lo_create('16849');

ALTER LARGE OBJECT 16849 OWNER TO postgres;

--
-- TOC entry 3581 (class 0 OID 0)
-- Dependencies: 3556
-- Name: LARGE OBJECT 16849; Type: COMMENT; Schema: -; Owner: postgres
--

COMMENT ON LARGE OBJECT 16849 IS 'first large object';


--
-- TOC entry 3558 (class 2613 OID 16850)
-- Name: 16850; Type: BLOB METADATA; Schema: -; Owner: postgres
--

SELECT pg_catalog.lo_create('16850');

ALTER LARGE OBJECT 16850 OWNER TO postgres;

--
-- TOC entry 3557 (class 0 OID 0)
-- Dependencies: 3556 3560
-- Data for Name: 16849; Type: BLOBS; Schema: -; Owner: postgres
--

BEGIN;

SELECT pg_catalog.lo_open('16849', 131072);
SELECT pg_catalog.lowrite(0, '\x48656c6c6f2c204c4f21');
SELECT pg_catalog.lo_close(0);

COMMIT;

--
-- TOC entry 3559 (class 0 OID 0)
-- Dependencies: 3558 3560
-- Data for Name: 16850; Type: BLOBS; Schema: -; Owner: postgres
--

BEGIN;

SELECT pg_catalog.lo_open('16850', 131072);
SELECT pg_catalog.lowrite(0, '\x00010203040506070809');
SELECT pg_catalog.lo_close(0);

COMMIT;

--
-- TOC entry 3379 (class 2606 OID 16730)
-- Name: orders orders_pkey; Type: CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.orders
    ADD CONSTRAINT orders_pkey PRIMARY KEY (id);


--
-- TOC entry 3383 (class 2606 OID 16847)
-- Name: price$$list price$$list_pkey; Type: CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects."price$$list"
    ADD CONSTRAINT "price$$list_pkey" PRIMARY KEY (id);


--
-- TOC entry 3377 (class 2606 OID 16720)
-- Name: widgets widgets_pkey; Type: CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.widgets
    ADD CONSTRAINT widgets_pkey PRIMARY KEY (id);


--
-- TOC entry 3380 (class 1259 OID 16788)
-- Name: events_event_date_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX events_event_date_idx ON ONLY objects.events USING btree (event_date);


--
-- TOC entry 3381 (class 1259 OID 16789)
-- Name: events_2024_event_date_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX events_2024_event_date_idx ON objects.events_2024 USING btree (event_date);


--
-- TOC entry 3375 (class 1259 OID 16737)
-- Name: widgets_label_idx; Type: INDEX; Schema: objects; Owner: postgres
--

CREATE INDEX widgets_label_idx ON objects.widgets USING btree (label);


--
-- TOC entry 3384 (class 0 OID 0)
-- Name: events_2024_event_date_idx; Type: INDEX ATTACH; Schema: objects; Owner: postgres
--

ALTER INDEX objects.events_event_date_idx ATTACH PARTITION objects.events_2024_event_date_idx;


--
-- TOC entry 3385 (class 3381 OID 16775)
-- Name: orders_stats; Type: STATISTICS; Schema: objects; Owner: postgres
--

CREATE STATISTICS objects.orders_stats (dependencies) ON widget_id, quantity FROM objects.orders;


ALTER STATISTICS objects.orders_stats OWNER TO postgres;

--
-- TOC entry 3537 (class 2618 OID 16760)
-- Name: widgets widgets_log_delete; Type: RULE; Schema: objects; Owner: postgres
--

CREATE RULE widgets_log_delete AS
    ON DELETE TO objects.widgets DO  INSERT INTO objects.widgets_deleted_log (id)
  VALUES (old.id);


--
-- TOC entry 3387 (class 2620 OID 16755)
-- Name: widgets widgets_audit_trigger; Type: TRIGGER; Schema: objects; Owner: postgres
--

CREATE TRIGGER widgets_audit_trigger AFTER INSERT OR UPDATE ON objects.widgets FOR EACH ROW EXECUTE FUNCTION objects.log_widget_change();


--
-- TOC entry 3386 (class 2606 OID 16731)
-- Name: orders orders_widget_id_fkey; Type: FK CONSTRAINT; Schema: objects; Owner: postgres
--

ALTER TABLE ONLY objects.orders
    ADD CONSTRAINT orders_widget_id_fkey FOREIGN KEY (widget_id) REFERENCES objects.widgets(id);


--
-- TOC entry 3538 (class 0 OID 16761)
-- Dependencies: 230
-- Name: secrets; Type: ROW SECURITY; Schema: objects; Owner: postgres
--

ALTER TABLE objects.secrets ENABLE ROW LEVEL SECURITY;

--
-- TOC entry 3539 (class 3256 OID 16768)
-- Name: secrets secrets_owner_only; Type: POLICY; Schema: objects; Owner: postgres
--

CREATE POLICY secrets_owner_only ON objects.secrets USING ((owner_role = CURRENT_USER));


--
-- TOC entry 3541 (class 6104 OID 16772)
-- Name: objects_pub_schema; Type: PUBLICATION; Schema: -; Owner: postgres
--

CREATE PUBLICATION objects_pub_schema WITH (publish = 'insert, update, delete, truncate');


ALTER PUBLICATION objects_pub_schema OWNER TO postgres;

--
-- TOC entry 3540 (class 6104 OID 16770)
-- Name: objects_pub_table; Type: PUBLICATION; Schema: -; Owner: postgres
--

CREATE PUBLICATION objects_pub_table WITH (publish = 'insert, update, delete, truncate');


ALTER PUBLICATION objects_pub_table OWNER TO postgres;

--
-- TOC entry 3542 (class 6106 OID 16771)
-- Name: objects_pub_table widgets; Type: PUBLICATION TABLE; Schema: objects; Owner: postgres
--

ALTER PUBLICATION objects_pub_table ADD TABLE ONLY objects.widgets;


--
-- TOC entry 3543 (class 6237 OID 16773)
-- Name: objects_pub_schema objects; Type: PUBLICATION TABLES IN SCHEMA; Schema: objects; Owner: postgres
--

ALTER PUBLICATION objects_pub_schema ADD TABLES IN SCHEMA objects;


--
-- TOC entry 3544 (class 6100 OID 16774)
-- Name: objects_sub; Type: SUBSCRIPTION; Schema: -; Owner: postgres
--

CREATE SUBSCRIPTION objects_sub CONNECTION 'host=nonexistent dbname=nonexistent' PUBLICATION objects_pub_table WITH (connect = false, slot_name = NONE, streaming = parallel);


ALTER SUBSCRIPTION objects_sub OWNER TO postgres;

--
-- TOC entry 3567 (class 0 OID 0)
-- Dependencies: 246
-- Name: FUNCTION no_public_execute(); Type: ACL; Schema: objects; Owner: postgres
--

REVOKE ALL ON FUNCTION objects.no_public_execute() FROM PUBLIC;


--
-- TOC entry 3569 (class 0 OID 0)
-- Dependencies: 231
-- Name: TABLE events; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.events TO fixture_reader;


--
-- TOC entry 3570 (class 0 OID 0)
-- Dependencies: 232
-- Name: TABLE events_2024; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.events_2024 TO fixture_reader;


--
-- TOC entry 3571 (class 0 OID 0)
-- Dependencies: 233
-- Name: TABLE imported; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.imported TO fixture_reader;


--
-- TOC entry 3574 (class 0 OID 0)
-- Dependencies: 235
-- Name: TABLE "price$$list"; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects."price$$list" TO fixture_reader;


--
-- TOC entry 3575 (class 0 OID 0)
-- Dependencies: 234
-- Name: TABLE tablespaced_table; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.tablespaced_table TO fixture_reader;


--
-- TOC entry 3578 (class 0 OID 0)
-- Dependencies: 222
-- Name: TABLE widgets; Type: ACL; Schema: objects; Owner: postgres
--

GRANT SELECT ON TABLE objects.widgets TO fixture_reader;
GRANT SELECT ON TABLE objects.widgets TO PUBLIC;


--
-- TOC entry 3582 (class 0 OID 0)
-- Dependencies: 3556
-- Name: LARGE OBJECT 16849; Type: ACL; Schema: -; Owner: postgres
--

GRANT SELECT ON LARGE OBJECT 16849 TO fixture_reader;


--
-- TOC entry 2124 (class 826 OID 16769)
-- Name: DEFAULT PRIVILEGES FOR TABLES; Type: DEFAULT ACL; Schema: objects; Owner: postgres
--

ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA objects GRANT SELECT ON TABLES TO fixture_reader;


--
-- TOC entry 3365 (class 3466 OID 16835)
-- Name: objects_ddl_log; Type: EVENT TRIGGER; Schema: -; Owner: postgres
--

CREATE EVENT TRIGGER objects_ddl_log ON ddl_command_start
   EXECUTE FUNCTION objects.noop_event_trigger();

ALTER EVENT TRIGGER objects_ddl_log DISABLE;


ALTER EVENT TRIGGER objects_ddl_log OWNER TO postgres;

--
-- TOC entry 3549 (class 0 OID 16742)
-- Dependencies: 227 3561
-- Name: widget_totals; Type: MATERIALIZED VIEW DATA; Schema: objects; Owner: postgres
--

REFRESH MATERIALIZED VIEW objects.widget_totals;


-- Completed on 2026-10-02 02:59:21 UTC

--
-- PostgreSQL database dump complete
--

\unrestrict QZe5MJC0ildIo08PQ1KHgHgMdvecSKA9nA3fiuWuy36Egd0HqcbBbvc4PFBcYjN

