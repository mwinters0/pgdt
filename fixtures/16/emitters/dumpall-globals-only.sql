--
-- PostgreSQL database cluster dump
--

-- Started on 2026-10-04 16:44:41 UTC

\restrict i8fZLtq1ou8xIM3eBoFth2BBkbZJqk5jcB3Yc220OK39ayNyNTBMqV9sbujVQOF

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


\unrestrict i8fZLtq1ou8xIM3eBoFth2BBkbZJqk5jcB3Yc220OK39ayNyNTBMqV9sbujVQOF

-- Completed on 2026-10-04 16:44:41 UTC

--
-- PostgreSQL database cluster dump complete
--

