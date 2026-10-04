--
-- PostgreSQL database cluster dump
--

\restrict 3csheorPymDOVouOVTsLYUQBxvp3o4qN6OV1cLVHfivqmeJm7Ws1sOod8XyddFa

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


\unrestrict 3csheorPymDOVouOVTsLYUQBxvp3o4qN6OV1cLVHfivqmeJm7Ws1sOod8XyddFa

--
-- PostgreSQL database cluster dump complete
--

