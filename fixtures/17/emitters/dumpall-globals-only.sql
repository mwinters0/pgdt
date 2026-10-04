--
-- PostgreSQL database cluster dump
--

-- Started on 2026-10-04 16:44:49 UTC

\restrict bmmjmdfJpdZi9SxhkLiPF2pipdeWdvLGOGK93yOa7RhxzZbVMiYacadtdlDKPI7

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


\unrestrict bmmjmdfJpdZi9SxhkLiPF2pipdeWdvLGOGK93yOa7RhxzZbVMiYacadtdlDKPI7

-- Completed on 2026-10-04 16:44:50 UTC

--
-- PostgreSQL database cluster dump complete
--

