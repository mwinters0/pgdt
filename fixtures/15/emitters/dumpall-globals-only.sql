--
-- PostgreSQL database cluster dump
--

-- Started on 2026-10-02 04:21:48 UTC

\restrict nRNzvEm9Iu1jPvB7Aydng9alRfBaaEJSHeIITmYrORTL8C9e40HAPFutDPFF20P

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE postgres;
ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;

--
-- User Configurations
--






--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict nRNzvEm9Iu1jPvB7Aydng9alRfBaaEJSHeIITmYrORTL8C9e40HAPFutDPFF20P

-- Completed on 2026-10-02 04:21:48 UTC

--
-- PostgreSQL database cluster dump complete
--

