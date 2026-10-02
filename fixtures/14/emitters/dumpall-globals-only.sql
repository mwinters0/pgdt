--
-- PostgreSQL database cluster dump
--

-- Started on 2026-10-02 04:21:40 UTC

\restrict FgZ8x4b1gdFEKrwVfOu15jQYsB4dlMeBCLMCoCEU7QQ7CxwShQlE1s0VBFpFI2y

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE postgres;
ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;




--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict FgZ8x4b1gdFEKrwVfOu15jQYsB4dlMeBCLMCoCEU7QQ7CxwShQlE1s0VBFpFI2y

-- Completed on 2026-10-02 04:21:40 UTC

--
-- PostgreSQL database cluster dump complete
--

