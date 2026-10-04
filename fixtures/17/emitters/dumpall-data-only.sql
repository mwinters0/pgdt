--
-- PostgreSQL database cluster dump
--

\restrict N2YtLoQeZscFBhNWxF9hQ8PhZslERsZt10YfQL6bGfcH4WZDALYPe0KTRajaplL

SET default_transaction_read_only = off;

SET client_encoding = 'UTF8';
SET standard_conforming_strings = on;

\unrestrict N2YtLoQeZscFBhNWxF9hQ8PhZslERsZt10YfQL6bGfcH4WZDALYPe0KTRajaplL

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

\restrict at6fktSDRHyRmpBAxlemPsjkrM7yv8qYYPQbfhgB40chZzeVKtymBGpBOnKbacy

-- Dumped from database version 17.11 (Debian 17.11-1.pgdg13+2)
-- Dumped by pg_dump version 17.11 (Debian 17.11-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
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

\unrestrict at6fktSDRHyRmpBAxlemPsjkrM7yv8qYYPQbfhgB40chZzeVKtymBGpBOnKbacy

--
-- Database "pgdt-emitters" dump
--

--
-- PostgreSQL database dump
--

\restrict keU8d87haLUUv7cYSyvj6ifc4pT7H2nWdI0asCvhBKGvr2qWvTg5Udaslv7GazP

-- Dumped from database version 17.11 (Debian 17.11-1.pgdg13+2)
-- Dumped by pg_dump version 17.11 (Debian 17.11-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
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

\unrestrict keU8d87haLUUv7cYSyvj6ifc4pT7H2nWdI0asCvhBKGvr2qWvTg5Udaslv7GazP
\encoding SQL_ASCII
\connect -reuse-previous=on "dbname='pgdt-emitters'"
\restrict keU8d87haLUUv7cYSyvj6ifc4pT7H2nWdI0asCvhBKGvr2qWvTg5Udaslv7GazP

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
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

\unrestrict keU8d87haLUUv7cYSyvj6ifc4pT7H2nWdI0asCvhBKGvr2qWvTg5Udaslv7GazP

--
-- Database "pgdt_fixture" dump
--

--
-- PostgreSQL database dump
--

\restrict UfXlCVi1RD7wlhNFh0hAMCbRNKTEarCYOeA96jYblGxZa7B2O4c99s7dsKdvQaN

-- Dumped from database version 17.11 (Debian 17.11-1.pgdg13+2)
-- Dumped by pg_dump version 17.11 (Debian 17.11-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
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

\unrestrict UfXlCVi1RD7wlhNFh0hAMCbRNKTEarCYOeA96jYblGxZa7B2O4c99s7dsKdvQaN
\connect pgdt_fixture
\restrict UfXlCVi1RD7wlhNFh0hAMCbRNKTEarCYOeA96jYblGxZa7B2O4c99s7dsKdvQaN

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
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

\unrestrict UfXlCVi1RD7wlhNFh0hAMCbRNKTEarCYOeA96jYblGxZa7B2O4c99s7dsKdvQaN

--
-- Database "postgres" dump
--

\connect postgres

--
-- PostgreSQL database dump
--

\restrict IqAdT7Vt2y52QJpnId7idwgTllZZaw5hg8KATPauhBloVmowFKfY7OdIuz39Nei

-- Dumped from database version 17.11 (Debian 17.11-1.pgdg13+2)
-- Dumped by pg_dump version 17.11 (Debian 17.11-1.pgdg13+2)

SET statement_timeout = 0;
SET lock_timeout = 0;
SET idle_in_transaction_session_timeout = 0;
SET transaction_timeout = 0;
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

\unrestrict IqAdT7Vt2y52QJpnId7idwgTllZZaw5hg8KATPauhBloVmowFKfY7OdIuz39Nei

--
-- PostgreSQL database cluster dump complete
--

