--
-- PostgreSQL database cluster dump
--

\restrict Z8WNzTqKYO8zolIe1llFBOhdJIoMJfH3vg1sFXCqIHzOvYMJyf1GVRbAexzVNNl

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict Z8WNzTqKYO8zolIe1llFBOhdJIoMJfH3vg1sFXCqIHzOvYMJyf1GVRbAexzVNNl

--
-- PostgreSQL database cluster dump complete
--

