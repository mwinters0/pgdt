#!/usr/bin/env python3
"""Unit tests for emitter_register.py, run with
`uv run python -m unittest test_emitter_register`.

Stdlib `unittest`, as the other reconciliations' tests are. The extraction
reads C, so its failure mode is silence: a literal it mis-reads is a row
nobody asked about, and one it misses is a branch nobody will. The unit tests
run it over short synthetic sources so each rule fails in front of its own
five lines; `CommittedTree` runs the join over the committed registers and
fixtures, and holds the unit to what the spec says it would have caught.
"""

from __future__ import annotations

import io
import unittest
import unittest.mock
from pathlib import Path
from tempfile import TemporaryDirectory

import emitter_register as er
import generate_fixtures as gf


def rows_of(body: str, function: str = "f") -> list[str]:
    return [row.entry for row in er.literals(body, "x.c", function)]


class Lexing(unittest.TestCase):
    def test_a_comment_marker_inside_a_literal_is_the_literal_s(self):
        tokens = er.lex('a("x /* dummy */"); /* gone */ b // gone\n')
        self.assertEqual(
            [t.text for t in tokens], ["a", "(", "x /* dummy */", ")", ";", "b"]
        )

    def test_escapes_are_decoded(self):
        (token,) = er.lex(r'"\\connect\n\t\"q\"\101"')
        self.assertEqual(token.text, '\\connect\n\t"q"A')

    def test_a_preprocessor_line_is_dropped(self):
        tokens = er.lex('#ifdef X\n"kept"\n#endif\n')
        self.assertEqual([t.text for t in tokens], ["kept"])

    def test_a_function_is_found_by_its_exact_name(self):
        text = (
            "static void\ndumpTypeColComments(int a)\n{\n\tx();\n}\n\n"
            "static void\ndumpType(int a)\n{\n\ty();\n}\n"
        )
        self.assertEqual(er.function_body(text, "dumpType"), "{\n\ty();\n}")
        self.assertIsNone(er.function_body(text, "dumpNothing"))


class Literals(unittest.TestCase):
    def test_a_format_string_gives_its_longest_constant_run(self):
        self.assertEqual(
            rows_of('{ appendPQExpBuffer(q, "ALTER %s OWNER TO %s;\\n", a, b); }'),
            [" OWNER TO "],
        )

    def test_a_constant_among_the_arguments_is_a_row(self):
        # The ternary's branch is where `UNLOGGED ` is, which is the point.
        body = '{ appendPQExpBuffer(q, "CREATE %s%s %s", u ? "UNLOGGED " : "", k, n); }'
        self.assertEqual(rows_of(body), ["CREATE ", "UNLOGGED "])

    def test_a_percent_percent_is_a_percent_inside_a_run(self):
        self.assertEqual(rows_of('{ appendPQExpBuffer(q, "LIKE 10%% OFF %s", x); }'), ["LIKE 10% OFF "])

    def test_a_plain_constant_s_percent_is_its_own(self):
        self.assertEqual(rows_of('{ appendPQExpBufferStr(q, "a %s b"); }'), ["a %s b"])

    def test_a_format_macro_ends_a_run(self):
        body = '{ appendPQExpBuffer(q, "SET x = " INT64_FORMAT " AND more", v); }'
        self.assertEqual(rows_of(body), [" AND more"])

    def test_adjacent_literals_are_one_constant(self):
        self.assertEqual(rows_of('{ appendPQExpBufferStr(q, "ab" "cd"); }'), ["abcd"])

    def test_a_run_under_the_minimum_is_no_entry(self):
        body = '{ appendPQExpBufferStr(q, ";\\n"); appendPQExpBuffer(q, "(%s)", x); }'
        self.assertEqual(rows_of(body), [])

    def test_a_repeated_entry_is_one_row(self):
        body = '{ appendPQExpBufferStr(q, "DROP "); appendPQExpBufferStr(d, "DROP "); }'
        self.assertEqual(rows_of(body), ["DROP "])

    def test_a_query_buffer_s_text_is_no_row(self):
        body = """{
            appendPQExpBufferStr(query, "SELECT oid FROM pg_type");
            res = ExecuteSqlQuery(fout, query->data, PGRES_TUPLES_OK);
            appendPQExpBufferStr(&conn, "SET x");
            ExecuteSqlStatement(fout, conn.data);
            appendPQExpBufferStr(q, "CREATE TYPE ");
        }"""
        self.assertEqual(rows_of(body), ["CREATE TYPE "])

    def test_a_buffer_reset_after_its_query_is_output_after_the_reset(self):
        body = """{
            appendPQExpBufferStr(query, "SELECT typname FROM pg_type");
            res = ExecuteSqlQuery(fout, query->data, PGRES_TUPLES_OK);
            resetPQExpBuffer(query);
            appendPQExpBufferStr(query, "CREATE TYPE ");
            ArchiveEntry(fout, query->data);
        }"""
        self.assertEqual(rows_of(body), ["CREATE TYPE "])

    def test_a_buffer_executed_when_connected_and_printed_otherwise_is_output(self):
        # The archiver's helpers: `_selectTablespace` and its siblings.
        body = """{
            appendPQExpBuffer(qry, "SET default_tablespace = %s", want);
            if (RestoringToDB(AH))
                res = PQexec(AH->connection, qry->data);
            else
                ahprintf(AH, "%s;\\n\\n", qry->data);
        }"""
        self.assertEqual(rows_of(body), ["SET default_tablespace = ", ";\n\n"])

    def test_a_query_quoted_by_a_diagnostic_stays_a_query(self):
        body = """{
            appendPQExpBufferStr(q, "COPY (SELECT ");
            q->data[q->len - 1] = ' ';
            res = ExecuteSqlQuery(fout, q->data, PGRES_COPY_OUT);
            pg_log_error_detail("Command was: %s", q->data);
            fprintf(stderr, "%s", q->data);
        }"""
        self.assertEqual(rows_of(body), [])

    def test_a_buffer_nothing_here_reads_is_output(self):
        # Handed out by pointer, as `appendPsqlMetaConnect`'s is.
        self.assertEqual(rows_of('{ appendPQExpBufferStr(buf, "\\\\connect "); }'), ["\\connect "])

    def test_a_shell_command_s_buffer_is_no_output(self):
        body = """{
            appendPQExpBufferStr(pgdumpopts, " --binary-upgrade");
            appendShellString(pgdumpopts, optarg);
            fprintf(OPF, "SET default_transaction_read_only = off;\\n\\n");
        }"""
        self.assertEqual(rows_of(body), ["SET default_transaction_read_only = off;\n\n"])

    def test_a_string_measured_or_compared_is_no_row(self):
        body = '{ appendPQExpBuffer(q, "GROUP %s;", fmtId(g->data + strlen("group "))); }'
        self.assertEqual(rows_of(body), ["GROUP "])

    def test_a_result_column_s_name_is_no_row(self):
        body = '{ appendPQExpBuffer(q, ",\\n    x = %s", PQgetvalue(res, 0, PQfnumber(res, "rngtype"))); }'
        self.assertEqual(rows_of(body), [",\n    x = "])

    def test_what_goes_to_stderr_is_no_row(self):
        body = '{ fprintf(stderr, _("bad name: \\"%s\\"\\n"), n); fprintf(OPF, "--\\n-- Tablespaces\\n"); }'
        self.assertEqual(rows_of(body), ["--\n-- Tablespaces\n"])

    def test_a_row_keeps_the_whole_constant_as_its_detail(self):
        (row,) = er.literals('{ ahprintf(AH, "-- TOC entry %d (class %u)\\n", a, b); }', "x.c", "f")
        self.assertEqual((row.entry, row.detail), ("-- TOC entry ", "-- TOC entry %d (class %u)\n"))


class ReaderKeywords(unittest.TestCase):
    def test_a_keyword_call_s_constant_is_a_keyword_in_any_case(self):
        src = 'fn f(p: &mut Cursor) { p.eat_keyword(b"copy")?; strip_kw(s, "ATTACH"); x("plain"); }'
        self.assertEqual(er.reader_keywords(src), ["copy", "ATTACH"])

    def test_a_constant_shaped_as_a_keyword_is_one_anywhere(self):
        src = r'''fn f() { let a = ["CREATE TABLE", "Name: "]; s.strip_prefix("-- Name: "); b"\\."; "\\connect "; b"\\N"; }'''
        self.assertEqual(er.reader_keywords(src), ["CREATE TABLE", "-- Name: ", "\\.", "\\connect "])

    def test_comments_and_the_test_module_are_not_read(self):
        src = 'fn f() {} // strip_kw(s, "GONE")\n/* "ALSO GONE" */\n#[cfg(test)]\nmod tests { strip_kw(s, "TESTED"); }\n'
        self.assertEqual(er.reader_keywords(src), [])

    def test_a_lifetime_is_no_literal(self):
        src = "fn f<'a>(s: &'a str) -> Option<&'a str> { strip_kw(s, \"GRANT \") }"
        self.assertEqual(er.reader_keywords(src), ["GRANT "])

    SOURCES = {"r.rs": 'fn f() { strip_kw(s, "ALTER TABLE"); strip_kw(s, "ONLY"); }'}

    def test_each_keyword_has_a_row_naming_listed_emitters(self):
        reads = (
            er.Read("r.rs", "ALTER TABLE", ("dumpTableSchema",)),
            er.Clause("r.rs", "ONLY", ("ALTER TABLE",)),
        )
        self.assertEqual(er.reads_problems(reads, self.SOURCES), [])

    def test_a_keyword_with_no_row_fails(self):
        reads = (er.Read("r.rs", "ALTER TABLE", ("dumpTableSchema",)),)
        (problem,) = er.reads_problems(reads, self.SOURCES)
        self.assertIn("'ONLY'", problem)

    def test_a_row_the_reader_no_longer_holds_fails(self):
        reads = (
            er.Read("r.rs", "ALTER TABLE", ("dumpTableSchema",)),
            er.Clause("r.rs", "ONLY", ("ALTER TABLE",)),
            er.Read("r.rs", "ATTACH", ("dumpTableAttach",)),
        )
        (problem,) = er.reads_problems(reads, self.SOURCES)
        self.assertIn("no longer recognises", problem)

    def test_a_keyword_written_by_an_unlisted_function_fails(self):
        reads = (
            er.Read("r.rs", "ALTER TABLE", ("dumpTableSchema", "dumpSomethingNew")),
            er.Clause("r.rs", "ONLY", ("ALTER TABLE",)),
        )
        (problem,) = er.reads_problems(reads, self.SOURCES)
        self.assertIn("`dumpSomethingNew`", problem)

    def test_a_clause_lies_within_a_read_of_its_own_reader(self):
        reads = (
            er.Read("r.rs", "ALTER TABLE", ("dumpTableSchema",)),
            er.Clause("r.rs", "ONLY", ("CREATE TABLE",)),
        )
        (problem,) = er.reads_problems(reads, self.SOURCES)
        self.assertIn("'CREATE TABLE'", problem)


class Options(unittest.TestCase):
    TABLE = """
    int main(void) {
        static struct option long_options[] = {
            {"data-only", no_argument, NULL, 'a'},
            {"if-exists", no_argument, &dopt.if_exists, 1},
            {"section", required_argument, NULL, 5},
            {NULL, 0, NULL, 0}
        };
    }
    """

    def test_each_entry_is_a_row_with_its_short_letter(self):
        rows, problems = er.options(self.TABLE, "pg_dump.c", "pg_dump")
        self.assertEqual(problems, [])
        self.assertEqual(
            [(r.entry, r.detail) for r in rows],
            [("data-only", "a"), ("if-exists", ""), ("section", "")],
        )

    def test_no_table_is_a_problem(self):
        rows, problems = er.options("int main(void) {}", "pg_dump.c", "pg_dump")
        self.assertEqual(rows, [])
        self.assertTrue(problems)

    def test_flags_resolve_by_long_and_short_form(self):
        rows, _ = er.options(self.TABLE, "pg_dump.c", "pg_dump")
        found, problems = er.resolve_flags(["-a", "--section=data", "dbname"], rows)
        self.assertEqual((found, problems), ({"data-only", "section"}, []))

    def test_a_flag_naming_no_option_is_a_problem(self):
        rows, _ = er.options(self.TABLE, "pg_dump.c", "pg_dump")
        _, problems = er.resolve_flags(["--no-such-thing"], rows)
        self.assertEqual(len(problems), 1)

    def test_a_flag_set_counts_only_at_its_minimum_major_and_above(self):
        schemas = {"s": {"default": [], "late": ("18", ["--statistics"]), "all": gf.Dumpall()}}
        with unittest.mock.patch.object(gf, "SCHEMAS", schemas):
            self.assertNotIn("--statistics", er.flags_at("17")["pg_dump"])
            self.assertIn("--statistics", er.flags_at("18")["pg_dump"])
            self.assertEqual(er.flags_at("18")["pg_dumpall"], gf.PG_DUMPALL_ARGS)


class RegisterFile(unittest.TestCase):
    def test_a_register_reads_back_as_written(self):
        rows = [
            er.Row("literal", "a.c", "f", "\\connect -x\n\t", "\\connect %s\n"),
            er.Row("option", "a.c", "pg_dump", "data-only", "a"),
        ]
        back = er.parse(er.render(rows, "18.6"))
        self.assertEqual((back.release, back.rows, back.problems), ("18.6", rows, []))

    def test_a_malformed_row_is_a_problem(self):
        text = er.render([], "18.6") + "literal\tonly-two\n"
        self.assertEqual(len(er.parse(text).problems), 1)


def write_tree(root: Path, major: str, register: str, dump: str) -> None:
    (root / major / "types").mkdir(parents=True)
    (root / major / "types" / "default.sql").write_text(dump)
    (root / major / er.REGISTER_NAME).write_text(register)


class Joining(unittest.TestCase):
    ROWS = [
        er.Row("literal", "a.c", "f", "CREATE TABLE ", "CREATE TABLE %s"),
        er.Row("literal", "a.c", "f", "UNLOGGED ", "UNLOGGED "),
        er.Row("option", "a.c", "pg_dump", "username", "U"),
        er.Row("option", "a.c", "pg_dump", "jobs", "j"),
        er.Row("option", "b.c", "pg_dumpall", "username", "U"),
        er.Row("option", "b.c", "pg_dumpall", "no-role-passwords", ""),
    ]

    def join(
        self,
        register: str,
        dump: str = "CREATE TABLE t (a int);\n",
        exemptions: tuple[er.Exemption, ...] = (),
    ):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_tree(root, "18", register, dump)
            with (
                unittest.mock.patch.object(gf, "SCHEMAS", {"types": {"default": []}}),
                unittest.mock.patch.object(er, "EXEMPTIONS", exemptions),
            ):
                return er.join_major("18", root)

    def test_a_literal_no_dump_holds_and_an_option_no_run_passes_are_uncovered(self):
        result, problems = self.join(er.render(self.ROWS, er.pinned_release("18")))
        self.assertEqual(problems, [])
        self.assertEqual(
            [u.entry for u in result.uncovered if u.kind != "value"], ["UNLOGGED ", "jobs"]
        )
        self.assertEqual((result.literals, result.options), (2, 4))

    def test_a_value_form_is_covered_by_a_dump_holding_its_spelling(self):
        forms = (
            er.ValueForm("v.c", "byteaout", "\\\\000", "types/default", "held"),
            er.ValueForm("v.c", "byteaout", "\\\\377", "types/default", "not held"),
        )
        with unittest.mock.patch.object(er, "VALUE_FORMS", forms):
            result, problems = self.join(
                er.render(self.ROWS, er.pinned_release("18")),
                dump="CREATE TABLE t (a int);\n1\t\\\\000\n",
            )
        self.assertEqual(problems, [])
        self.assertEqual(result.values, 2)
        self.assertEqual(
            [(u.kind, u.entry) for u in result.uncovered if u.kind == "value"],
            [("value", "\\\\377")],
        )

    def test_a_value_form_naming_no_flag_set_is_a_problem(self):
        form = er.ValueForm("v.c", "byteaout", "x", "types/no-such-set", "")
        with unittest.mock.patch.object(gf, "SCHEMAS", {"types": {"default": []}}):
            self.assertEqual(len(er.value_form_problems((form,))), 1)
            self.assertEqual(
                er.value_form_problems((er.ValueForm("v.c", "f", "x", "types/default", ""),)), []
            )

    def test_an_exempt_row_is_not_uncovered(self):
        exempt = er.Exemption("literal", "f", "UNLOGGED ", er.Known("KD65"))
        result, problems = self.join(
            er.render(self.ROWS, er.pinned_release("18")), exemptions=(exempt,)
        )
        self.assertEqual(problems, [])
        self.assertEqual([u.entry for u in result.uncovered if u.kind != "value"], ["jobs"])
        self.assertEqual(result.exempt, [(self.ROWS[1], exempt)])

    def test_an_exemption_a_fixture_reaches_is_stale(self):
        exempt = er.Exemption("literal", "f", "CREATE TABLE ", er.Known("KD65"))
        result, _ = self.join(er.render(self.ROWS, er.pinned_release("18")), exemptions=(exempt,))
        self.assertEqual(result.stale, [exempt])
        self.assertEqual(len(er.exemption_problems([result], (exempt,))), 1)

    def test_an_exemption_no_register_holds_is_dead(self):
        exempt = er.Exemption("option", "pg_dump", "no-such-option", er.NoOutput("x"))
        result, _ = self.join(er.render(self.ROWS, er.pinned_release("18")), exemptions=(exempt,))
        problems = er.exemption_problems([result], (exempt,))
        self.assertEqual(len(problems), 1)
        self.assertIn("no register holds it", problems[0])

    def test_a_no_output_exemption_needs_its_evidence_row(self):
        exempt = er.Exemption("option", "pg_dump", "jobs", er.NoOutput("numWorkers"))
        bare = er.render(self.ROWS, er.pinned_release("18"))
        _, problems = self.join(bare, exemptions=(exempt,))
        self.assertEqual(len(problems), 1)
        self.assertIn("re-run", problems[0])
        evidence = er.Row("evidence", "a.c", "pg_dump", "jobs", "numWorkers")
        result, problems = self.join(
            er.render(self.ROWS + [evidence], er.pinned_release("18")), exemptions=(exempt,)
        )
        self.assertEqual(problems, [])
        self.assertEqual([u.entry for u in result.uncovered if u.kind != "value"], ["UNLOGGED "])

    def test_a_register_from_another_minor_is_a_problem(self):
        _, problems = self.join(er.render(self.ROWS, "18.0"))
        self.assertTrue(any("read from 18.0" in p for p in problems), problems)

    def test_a_missing_register_is_a_problem(self):
        with TemporaryDirectory() as tmp:
            _, problems = er.join_major("18", Path(tmp))
        self.assertTrue(problems)


class Dispositions(unittest.TestCase):
    INVARIANTS = "## I52 — a property\n"
    COMPATIBILITY = "| `--format=custom` | Planned (P8) | prose |\n"

    def problems(self, *exemptions: er.Exemption) -> list[str]:
        return er.reason_problems(exemptions, self.INVARIANTS, self.COMPATIBILITY, {"KD65"})

    def test_each_reason_resolves_against_the_record(self):
        self.assertEqual(
            self.problems(
                er.Exemption("literal", "f", "x", er.Invariant("I52")),
                er.Exemption("literal", "f", "y", er.Known("KD65")),
                er.Exemption("option", "pg_dump", "format", er.Unsupported("`--format=custom`")),
                er.Exemption("option", "pg_dump", "host", er.NoOutput("pghost")),
            ),
            [],
        )

    def test_an_unresolved_reason_is_a_problem(self):
        for exemption in (
            er.Exemption("literal", "f", "x", er.Invariant("I9999")),
            er.Exemption("literal", "f", "y", er.Known("KD1")),
            er.Exemption("option", "pg_dump", "format", er.Unsupported("`--format=tar`")),
        ):
            with self.subTest(exemption=exemption):
                self.assertEqual(len(self.problems(exemption)), 1)

    def test_an_unsupported_exemption_lapses_once_its_row_is_read(self):
        # The phase landing the reader moves the row, and the option then
        # needs a flag set rather than an exemption.
        exemption = er.Exemption("option", "pg_dump", "format", er.Unsupported("`--format=custom`"))
        for status, lapsed in (
            ("Planned (P8)", False),
            ("Unsupported (errors)", False),
            ("Tested (fixture)", True),
            ("Untested", True),
        ):
            with self.subTest(status=status):
                problems = er.reason_problems(
                    (exemption,), self.INVARIANTS, f"| `--format=custom` | {status} | prose |\n", set()
                )
                self.assertEqual(len(problems), int(lapsed))

    def test_a_literal_is_exempt_only_by_an_invariant_or_a_deficiency(self):
        # Every byte passes through the map, so "pgdt does not read this" is
        # no reason a literal may go unfixtured.
        self.assertEqual(
            len(self.problems(er.Exemption("literal", "f", "x", er.NoOutput("x")))), 1
        )

    def test_the_extraction_records_a_found_needle_and_refuses_a_missing_one(self):
        rows = [er.Row("option", "src/bin/pg_dump/pg_dump.c", "pg_dump", "host", "h")]
        ok = er.Exemption("option", "pg_dump", "host", er.NoOutput("pghost = x;"))
        gone = er.Exemption("option", "pg_dump", "host", er.NoOutput("not there"))
        text = {"src/bin/pg_dump/pg_dump.c": "case 'h': pghost = x; break;"}
        with unittest.mock.patch.object(er, "EXEMPTIONS", (ok,)):
            found, problems = er.evidence_rows(text, rows)
        self.assertEqual(
            (found, problems),
            ([er.Row("evidence", "src/bin/pg_dump/pg_dump.c", "pg_dump", "host", "pghost = x;")], []),
        )
        with unittest.mock.patch.object(er, "EXEMPTIONS", (gone,)):
            found, problems = er.evidence_rows(text, rows)
        self.assertEqual((found, len(problems)), ([], 1))

    def test_an_option_a_major_lacks_needs_no_evidence_there(self):
        exempt = er.Exemption("option", "pg_dump", "sync-method", er.NoOutput("anything"))
        with unittest.mock.patch.object(er, "EXEMPTIONS", (exempt,)):
            self.assertEqual(er.evidence_rows({}, []), ([], []))


class Extracting(unittest.TestCase):
    def test_a_checkout_of_another_release_is_refused(self):
        with TemporaryDirectory() as tmp:
            checkout = Path(tmp) / f"release-v{er.pinned_release('18')}"
            checkout.mkdir()
            (checkout / "configure").write_text("PACKAGE_VERSION='18.0'\n")
            problems = er.extract_major("18", Path(tmp), Path(tmp) / "fixtures")
        self.assertEqual(len(problems), 1)
        self.assertIn("'18.0'", problems[0])

    def test_a_function_gone_from_the_tree_is_a_problem_not_a_shorter_register(self):
        for major in ("13", "18"):
            with self.subTest(major=major), TemporaryDirectory() as tmp:
                checkout = Path(tmp)
                for file, _ in er.FUNCTIONS:
                    (checkout / file).parent.mkdir(parents=True, exist_ok=True)
                    (checkout / file).write_text("static void\nunrelated(void)\n{\n}\n")
                _, problems = er.extract(checkout, major)
                expected = sum(e.at(major) for _, emitters in er.FUNCTIONS for e in emitters)
                self.assertEqual(sum("no function" in p for p in problems), expected)

    def test_a_function_is_asked_for_only_at_the_majors_it_exists_at(self):
        attach = er.listed()["dumpTableAttach"]
        self.assertEqual((attach.at("13"), attach.at("14"), attach.at("18")), (False, True, True))
        blobs = er.listed()["StartRestoreBlobs"]
        self.assertEqual((blobs.at("13"), blobs.at("15"), blobs.at("16")), (True, True, False))


class CommittedTree(unittest.TestCase):
    """The committed registers against the committed fixtures."""

    def test_every_major_has_a_register_from_its_pinned_release(self):
        for major in er.majors():
            with self.subTest(major=major):
                _, problems = er.join_major(major)
                self.assertEqual(problems, [])

    def test_every_value_form_names_a_flag_set_the_generator_runs(self):
        self.assertEqual(er.value_form_problems(), [])

    def test_every_row_is_held_by_a_fixture_or_exempt(self):
        # The gate: a literal, option or value form no fixture reaches and no
        # exemption covers fails here, and so fails `mise run check`.
        out = io.StringIO()
        self.assertEqual(er.main([], out=out), 0, out.getvalue())
        self.assertIn("every row is held by a fixture or exempt", out.getvalue())

    def test_every_exemption_s_reason_resolves(self):
        self.assertEqual(er.reason_problems(), [])

    def test_the_literals_the_unit_was_chosen_to_catch_are_rows(self):
        # KD61, KD64, KD65, KD66 and KD68, each a literal (the spec, "The
        # emitter register"): a register missing one has lost the reach that
        # justified its unit.
        wanted = {
            ("dumpTablespaces", "CREATE TABLESPACE "),
            ("dumpTableSchema", "\nINHERITS ("),
            ("dumpTableSchema", "UNLOGGED "),
            ("dumpCompositeType", " INTEGER /* dummy */"),
            ("appendPsqlMetaConnect", "\\connect -reuse-previous=on "),
        }
        for major in er.majors():
            register = er.parse((er.FIXTURES / major / er.REGISTER_NAME).read_text())
            found = {(r.function, r.entry) for r in register.rows if r.kind == "literal"}
            with self.subTest(major=major):
                self.assertEqual(wanted - found, set())

    def test_every_listed_function_contributes_but_the_one_that_only_queries(self):
        for major in er.majors():
            register = er.parse((er.FIXTURES / major / er.REGISTER_NAME).read_text())
            seen = {r.function for r in register.rows if r.kind == "literal"}
            listed = {e.name for _, emitters in er.FUNCTIONS for e in emitters if e.at(major)}
            with self.subTest(major=major):
                self.assertEqual(listed - seen, {"setup_connection"})

    def test_every_keyword_a_reader_recognises_maps_to_a_listed_emitter(self):
        # The check that keeps FUNCTIONS on the spec's criterion: a reader
        # taught a statement brings its emitter into the list.
        self.assertEqual(er.reads_problems(), [])

    def test_the_emitter_the_list_once_missed_is_read(self):
        # `dumpTableAttach`, the drift the check exists for
        # (docs/status/history/2026-10-04.md).
        for major in er.majors():
            register = er.parse((er.FIXTURES / major / er.REGISTER_NAME).read_text())
            found = {(r.function, r.entry) for r in register.rows if r.kind == "literal"}
            with self.subTest(major=major):
                self.assertEqual(
                    ("dumpTableAttach", "ATTACH PARTITION ") in found, int(major) >= 14
                )


if __name__ == "__main__":
    unittest.main()
