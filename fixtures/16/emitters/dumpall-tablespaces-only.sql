--
-- PostgreSQL database cluster dump
--

\restrict R2f9sFdAg8sXg7phwdclxPJ5r5C926wjYZ6BcjGJMLeHDFpsh2X5pSOrEBoI9xG

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict R2f9sFdAg8sXg7phwdclxPJ5r5C926wjYZ6BcjGJMLeHDFpsh2X5pSOrEBoI9xG

--
-- PostgreSQL database cluster dump complete
--

