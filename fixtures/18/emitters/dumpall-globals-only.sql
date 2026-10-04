--
-- PostgreSQL database cluster dump
--

-- Started on 2026-10-04 19:48:23 UTC

\restrict F2BUPSYr0MtnJozoM2cudb2PgBIyNTg6nX9uquvdVdzYxXDwXDRVMdvTACDs8fG

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE emitters_grantor;
ALTER ROLE emitters_grantor WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;
CREATE ROLE emitters_member;
ALTER ROLE emitters_member WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;
CREATE ROLE emitters_other;
ALTER ROLE emitters_other WITH NOSUPERUSER INHERIT NOCREATEROLE NOCREATEDB NOLOGIN NOREPLICATION NOBYPASSRLS;
CREATE ROLE postgres;
ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;

--
-- User Configurations
--


--
-- Role memberships
--

GRANT emitters_grantor TO emitters_member WITH ADMIN OPTION, INHERIT TRUE GRANTED BY postgres;
GRANT emitters_grantor TO emitters_other WITH INHERIT FALSE, SET FALSE GRANTED BY postgres;




--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict F2BUPSYr0MtnJozoM2cudb2PgBIyNTg6nX9uquvdVdzYxXDwXDRVMdvTACDs8fG

-- Completed on 2026-10-04 19:48:23 UTC

--
-- PostgreSQL database cluster dump complete
--

