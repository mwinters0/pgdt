--
-- PostgreSQL database cluster dump
--

\restrict jsm1AubANJq94rUwN4Pr0bJOLuQ6bjQXbba97iks4NwOab9BYRmZQ8q1f7wGRr7

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

--
-- Tablespaces
--

CREATE TABLESPACE emitters_ts OWNER postgres LOCATION '/var/lib/postgresql/emitters_tablespace';
ALTER TABLESPACE emitters_ts SET (seq_page_cost=1.5);
COMMENT ON TABLESPACE emitters_ts IS 'the emitters fixture''s tablespace';


\unrestrict jsm1AubANJq94rUwN4Pr0bJOLuQ6bjQXbba97iks4NwOab9BYRmZQ8q1f7wGRr7

--
-- PostgreSQL database cluster dump complete
--

