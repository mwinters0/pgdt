--
-- PostgreSQL database cluster dump
--

\restrict xmYjJdeUEhfCjow794qejbQJwB2eEja07pKLiCDcF7RQ371BZa6qbnLdW14O8Vb

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict xmYjJdeUEhfCjow794qejbQJwB2eEja07pKLiCDcF7RQ371BZa6qbnLdW14O8Vb

--
-- PostgreSQL database cluster dump complete
--

