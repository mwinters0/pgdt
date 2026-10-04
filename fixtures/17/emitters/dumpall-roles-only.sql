--
-- PostgreSQL database cluster dump
--

\restrict GvXdgvZkCIeyVJSIwnt1BcG3NbvcwwtE1nWISiHkMUObWgkfzWE4eQbGVxxe5hx

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




\unrestrict GvXdgvZkCIeyVJSIwnt1BcG3NbvcwwtE1nWISiHkMUObWgkfzWE4eQbGVxxe5hx

--
-- PostgreSQL database cluster dump complete
--

