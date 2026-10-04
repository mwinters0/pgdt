--
-- PostgreSQL database cluster dump
--

\restrict c2ybLwhdaiohekUhenTvKkkawoAAQsaMRn78NGa2beOSUSOJCUrmaZnPDnPmxj6

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict c2ybLwhdaiohekUhenTvKkkawoAAQsaMRn78NGa2beOSUSOJCUrmaZnPDnPmxj6

--
-- PostgreSQL database cluster dump complete
--

