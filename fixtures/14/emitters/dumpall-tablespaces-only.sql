--
-- PostgreSQL database cluster dump
--

\restrict eU0u4gBKmy3mD6fvFUyZtpDkjdsIrBseGpaSej8io4a71Luhbx26dPQJloVfL0A

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict eU0u4gBKmy3mD6fvFUyZtpDkjdsIrBseGpaSej8io4a71Luhbx26dPQJloVfL0A

--
-- PostgreSQL database cluster dump complete
--

