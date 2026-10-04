--
-- PostgreSQL database cluster dump
--

-- Started on 2026-10-04 19:47:45 UTC

\restrict AswXNpPQdmEsZjqpzkv0yZPFU8kRrFbj6TdPrd4bjf6S5WAMCinsy3Ehrog83eI

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
-- Role memberships
--

GRANT emitters_grantor TO emitters_member WITH ADMIN OPTION GRANTED BY postgres;


--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict AswXNpPQdmEsZjqpzkv0yZPFU8kRrFbj6TdPrd4bjf6S5WAMCinsy3Ehrog83eI

-- Completed on 2026-10-04 19:47:45 UTC

--
-- PostgreSQL database cluster dump complete
--

