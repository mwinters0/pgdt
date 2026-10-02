--
-- PostgreSQL database cluster dump
--

\restrict b4y8usP2WwxOk2bPf9FmFhJoaa3HalQSTDuX7rPif32p9LJcZwvnf6NK3gzukHP

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict b4y8usP2WwxOk2bPf9FmFhJoaa3HalQSTDuX7rPif32p9LJcZwvnf6NK3gzukHP

--
-- Databases
--

--
-- Database "template1" dump
--

\connect template1

--
-- PostgreSQL database dump
--

\restrict vwWKEZVoQMXcv0P4Ym30DgAs99LzXJrlanSIdTaP3tHasqIE7yw2d0xa5wuS7wf

-- Dumped from database version 13.23 (Debian 13.23-1.pgdg13+1)
-- Dumped by pg_dump version 13.23 (Debian 13.23-1.pgdg13+1)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- PostgreSQL database dump complete
--

\unrestrict vwWKEZVoQMXcv0P4Ym30DgAs99LzXJrlanSIdTaP3tHasqIE7yw2d0xa5wuS7wf

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict 9HzUW3lXQpgrntIps2LoZ5BcG7Sw08W6pUxM4gn3zh2ddhWUVu11ko54unJfGA6

-- Dumped from database version 13.23 (Debian 13.23-1.pgdg13+1)
-- Dumped by pg_dump version 13.23 (Debian 13.23-1.pgdg13+1)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: pgdt-emitters; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE "pgdt-emitters" WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE = 'en_US.utf8';


ALTER DATABASE "pgdt-emitters" OWNER TO postgres;

\unrestrict 9HzUW3lXQpgrntIps2LoZ5BcG7Sw08W6pUxM4gn3zh2ddhWUVu11ko54unJfGA6
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict 9HzUW3lXQpgrntIps2LoZ5BcG7Sw08W6pUxM4gn3zh2ddhWUVu11ko54unJfGA6

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Data for Name: named; Type: TABLE DATA; Schema: public; Owner: postgres
--

SET SESSION AUTHORIZATION 'postgres';

ALTER TABLE public.named DISABLE TRIGGER ALL;

INSERT INTO public.named (id, label, born) VALUES
	(1, 'first', '2024-04-01'),
	(2, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE public.named ENABLE TRIGGER ALL;

--
-- PostgreSQL database dump complete
--

\unrestrict 9HzUW3lXQpgrntIps2LoZ5BcG7Sw08W6pUxM4gn3zh2ddhWUVu11ko54unJfGA6

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict 0aTkFcdgAisP09ZSPOi97Tymh8tqCB003k6vYCuBYbMXs705YORPVx1kM8AxC2r

-- Dumped from database version 13.23 (Debian 13.23-1.pgdg13+1)
-- Dumped by pg_dump version 13.23 (Debian 13.23-1.pgdg13+1)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Name: pgdt_fixture; Type: DATABASE; Schema: -; Owner: postgres
--

CREATE DATABASE pgdt_fixture WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE = 'en_US.utf8';


ALTER DATABASE pgdt_fixture OWNER TO postgres;

\unrestrict 0aTkFcdgAisP09ZSPOi97Tymh8tqCB003k6vYCuBYbMXs705YORPVx1kM8AxC2r
\connect pgdt_fixture
\restrict 0aTkFcdgAisP09ZSPOi97Tymh8tqCB003k6vYCuBYbMXs705YORPVx1kM8AxC2r

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- Data for Name: base_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

SET SESSION AUTHORIZATION 'postgres';

ALTER TABLE emitters.base_values DISABLE TRIGGER ALL;

INSERT INTO emitters.base_values (id, v_varchar, v_char, v_int2, v_main, v_pair) VALUES
	(1, 'alpha', 'a', '7', 'main text', '(1.5,2)'),
	(2, NULL, NULL, NULL, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.base_values ENABLE TRIGGER ALL;

--
-- Data for Name: child; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.child DISABLE TRIGGER ALL;

INSERT INTO emitters.child (id, label, born, extra) VALUES
	(2, 'c', '2024-01-02', 1.25),
	(3, 'd', NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.child ENABLE TRIGGER ALL;

--
-- Data for Name: domain_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.domain_values DISABLE TRIGGER ALL;

INSERT INTO emitters.domain_values (id, v_positive, v_mood, v_pair) VALUES
	(1, 3, 'busy', '(1,one)'),
	(2, NULL, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.domain_values ENABLE TRIGGER ALL;

--
-- Data for Name: grown; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.grown DISABLE TRIGGER ALL;

INSERT INTO emitters.grown (id, b, added) VALUES
	(1, 'b', '2024-02-29'),
	(2, 'b2', '2024-02-29') ON CONFLICT DO NOTHING;
INSERT INTO emitters.grown (id, b, added) VALUES
	(3, 'b3', '2024-02-29') ON CONFLICT DO NOTHING;


ALTER TABLE emitters.grown ENABLE TRIGGER ALL;

--
-- Data for Name: parent; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.parent DISABLE TRIGGER ALL;

INSERT INTO emitters.parent (id, label, born) VALUES
	(1, 'p', '2024-01-01') ON CONFLICT DO NOTHING;


ALTER TABLE emitters.parent ENABLE TRIGGER ALL;

--
-- Data for Name: people; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.people DISABLE TRIGGER ALL;

INSERT INTO emitters.people (name, born, height) VALUES
	('ann', '1990-05-01', 170),
	('bob', NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.people ENABLE TRIGGER ALL;

--
-- Data for Name: range_values; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.range_values DISABLE TRIGGER ALL;

INSERT INTO emitters.range_values (id, v_canon, v_diff, v_pattern) VALUES
	(1, '[1,6)', '[1.5,2.5)', '[a,m)'),
	(2, 'empty', '(,0]', '[n,)') ON CONFLICT DO NOTHING;
INSERT INTO emitters.range_values (id, v_canon, v_diff, v_pattern) VALUES
	(3, NULL, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.range_values ENABLE TRIGGER ALL;

--
-- Data for Name: scratch; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.scratch DISABLE TRIGGER ALL;

INSERT INTO emitters.scratch (id, at, ok) VALUES
	(1, '2024-03-01', true),
	(2, NULL, false) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.scratch ENABLE TRIGGER ALL;

--
-- Data for Name: stamped; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.stamped DISABLE TRIGGER ALL;

INSERT INTO emitters.stamped (id, stamps, now) VALUES
	(1, '{"2024-01-01 00:00:00+00","2024-01-02 12:30:00+00"}', 7),
	(2, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.stamped ENABLE TRIGGER ALL;

--
-- Data for Name: trios; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.trios DISABLE TRIGGER ALL;

INSERT INTO emitters.trios (id, v) VALUES
	(1, '(1,2024-01-01)'),
	(2, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.trios ENABLE TRIGGER ALL;

--
-- Data for Name: tuned; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.tuned DISABLE TRIGGER ALL;

INSERT INTO emitters.tuned (id, note, amount) VALUES
	(1, 'one', 1.50),
	(2, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.tuned ENABLE TRIGGER ALL;

--
-- Data for Name: unidentified; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.unidentified DISABLE TRIGGER ALL;

INSERT INTO emitters.unidentified (id, label) VALUES
	(1, 'x') ON CONFLICT DO NOTHING;


ALTER TABLE emitters.unidentified ENABLE TRIGGER ALL;

--
-- PostgreSQL database dump complete
--

\unrestrict 0aTkFcdgAisP09ZSPOi97Tymh8tqCB003k6vYCuBYbMXs705YORPVx1kM8AxC2r

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict Rlah4DwcI7O2h6357IHURqOmLk3ykUVRwwPdP922sJgDddC1Hww8haXHf4IerKJ

-- Dumped from database version 13.23 (Debian 13.23-1.pgdg13+1)
-- Dumped by pg_dump version 13.23 (Debian 13.23-1.pgdg13+1)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;
SELECT pg_catalog.set_config('search_path', '', false);
SET check_function_bodies = false;
SET xmloption = content;
SET client_min_messages = warning;
SET row_security = off;

--
-- PostgreSQL database dump complete
--

\unrestrict Rlah4DwcI7O2h6357IHURqOmLk3ykUVRwwPdP922sJgDddC1Hww8haXHf4IerKJ

--
-- PostgreSQL database cluster dump complete
--

