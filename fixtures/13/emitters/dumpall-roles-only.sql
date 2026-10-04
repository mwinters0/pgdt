--
-- PostgreSQL database cluster dump
--

\restrict FRMrH2SEdxeL0Nlq43uHIaB7RrjmWNtqBKp0H6gQ3skESdiyiZ5u5VMvNl2DOBe

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE postgres;
ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;




\unrestrict FRMrH2SEdxeL0Nlq43uHIaB7RrjmWNtqBKp0H6gQ3skESdiyiZ5u5VMvNl2DOBe

--
-- PostgreSQL database cluster dump complete
--

