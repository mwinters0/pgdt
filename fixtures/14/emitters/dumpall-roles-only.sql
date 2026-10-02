--
-- PostgreSQL database cluster dump
--

\restrict PbSNzVTMGpJuJxlywvZHs4hgkaDMspGa1oKaY0zZ43fTiS0enxYHjEY7ehO9qpn

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Roles
--

CREATE ROLE postgres;
ALTER ROLE postgres WITH SUPERUSER INHERIT CREATEROLE CREATEDB LOGIN REPLICATION BYPASSRLS;




\unrestrict PbSNzVTMGpJuJxlywvZHs4hgkaDMspGa1oKaY0zZ43fTiS0enxYHjEY7ehO9qpn

--
-- PostgreSQL database cluster dump complete
--

