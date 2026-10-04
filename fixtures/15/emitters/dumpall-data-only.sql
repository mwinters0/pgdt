--
-- PostgreSQL database cluster dump
--

\restrict 35ejjlaCaYDYVejASpAL0hMPRxZkubDicFlcD3eVAUcdYtRUlWdAkNXHhfnarOr

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict 35ejjlaCaYDYVejASpAL0hMPRxZkubDicFlcD3eVAUcdYtRUlWdAkNXHhfnarOr

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

\restrict U1mBBiYOKTdpTJPGadefqj1W2bpMo0lcT3aNjbkVq7eiMZmQMCJ2OHjXeAvaadC

-- Dumped from database version 15.19 (Debian 15.19-1.pgdg13+2)
-- Dumped by pg_dump version 15.19 (Debian 15.19-1.pgdg13+2)

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

\unrestrict U1mBBiYOKTdpTJPGadefqj1W2bpMo0lcT3aNjbkVq7eiMZmQMCJ2OHjXeAvaadC

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict JlnhWEjuOZSpXoDueSSByYzXeEuozpu3oRkFHWqLELG6QDZZbkGEubAlMX3crHS

-- Dumped from database version 15.19 (Debian 15.19-1.pgdg13+2)
-- Dumped by pg_dump version 15.19 (Debian 15.19-1.pgdg13+2)

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

CREATE DATABASE "pgdt-emitters" WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE_PROVIDER = libc LOCALE = 'en_US.utf8';


ALTER DATABASE "pgdt-emitters" OWNER TO postgres;

\unrestrict JlnhWEjuOZSpXoDueSSByYzXeEuozpu3oRkFHWqLELG6QDZZbkGEubAlMX3crHS
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict JlnhWEjuOZSpXoDueSSByYzXeEuozpu3oRkFHWqLELG6QDZZbkGEubAlMX3crHS

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

\unrestrict JlnhWEjuOZSpXoDueSSByYzXeEuozpu3oRkFHWqLELG6QDZZbkGEubAlMX3crHS

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict vwmaVvw8u4Wpj1sHQXEelCprcxDbrGWqpR4wIN8BYXPe0B0yB5hdAVvo3TtjlBc

-- Dumped from database version 15.19 (Debian 15.19-1.pgdg13+2)
-- Dumped by pg_dump version 15.19 (Debian 15.19-1.pgdg13+2)

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

CREATE DATABASE pgdt_fixture WITH TEMPLATE = template0 ENCODING = 'UTF8' LOCALE_PROVIDER = libc LOCALE = 'en_US.utf8';


ALTER DATABASE pgdt_fixture OWNER TO postgres;

\unrestrict vwmaVvw8u4Wpj1sHQXEelCprcxDbrGWqpR4wIN8BYXPe0B0yB5hdAVvo3TtjlBc
\connect pgdt_fixture
\restrict vwmaVvw8u4Wpj1sHQXEelCprcxDbrGWqpR4wIN8BYXPe0B0yB5hdAVvo3TtjlBc

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
-- Data for Name: collated; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.collated DISABLE TRIGGER ALL;

INSERT INTO emitters.collated (id, label) VALUES
	(1, 'b'),
	(2, 'a') ON CONFLICT DO NOTHING;


ALTER TABLE emitters.collated ENABLE TRIGGER ALL;

--
-- Data for Name: delimited; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.delimited DISABLE TRIGGER ALL;

INSERT INTO emitters.delimited (id, v, a) VALUES
	(1, '("{""a b"";c,d;""e;f""}")', '{"a b";c}'),
	(2, '("{{x;""y z""};{"""";NULL}}")', NULL) ON CONFLICT DO NOTHING;
INSERT INTO emitters.delimited (id, v, a) VALUES
	(3, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.delimited ENABLE TRIGGER ALL;

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
-- Data for Name: identified; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.identified DISABLE TRIGGER ALL;

INSERT INTO emitters.identified (id, label) OVERRIDING SYSTEM VALUE VALUES
	(1, 'one'),
	(2, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.identified ENABLE TRIGGER ALL;

--
-- Data for Name: keyed; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.keyed DISABLE TRIGGER ALL;

INSERT INTO emitters.keyed (id, code, note) VALUES
	(1, 'a', 'first'),
	(2, NULL, NULL) ON CONFLICT DO NOTHING;


ALTER TABLE emitters.keyed ENABLE TRIGGER ALL;

--
-- Data for Name: no_columns; Type: TABLE DATA; Schema: emitters; Owner: postgres
--

ALTER TABLE emitters.no_columns DISABLE TRIGGER ALL;

INSERT INTO emitters.no_columns DEFAULT VALUES;
INSERT INTO emitters.no_columns DEFAULT VALUES;


ALTER TABLE emitters.no_columns ENABLE TRIGGER ALL;

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
-- Name: identified_id_seq; Type: SEQUENCE SET; Schema: emitters; Owner: postgres
--

SELECT pg_catalog.setval('emitters.identified_id_seq', 2, true);


--
-- PostgreSQL database dump complete
--

\unrestrict vwmaVvw8u4Wpj1sHQXEelCprcxDbrGWqpR4wIN8BYXPe0B0yB5hdAVvo3TtjlBc

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict US6ebUq6XQQqfoRZeIu1VSbGRXNNPJ8HxHnJ57L9SijXrvfz697lbevgoAQ8MhM

-- Dumped from database version 15.19 (Debian 15.19-1.pgdg13+2)
-- Dumped by pg_dump version 15.19 (Debian 15.19-1.pgdg13+2)

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

\unrestrict US6ebUq6XQQqfoRZeIu1VSbGRXNNPJ8HxHnJ57L9SijXrvfz697lbevgoAQ8MhM

--
-- PostgreSQL database cluster dump complete
--

