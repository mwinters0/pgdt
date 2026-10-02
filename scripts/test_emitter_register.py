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
    rows, problems = er.literals(body, "x.c", function)
    assert problems == [], problems
    return [row.entry for row in rows]


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

    def test_a_query_buffer_also_read_as_output_is_a_problem(self):
        body = """{
            appendPQExpBufferStr(query, "SELECT 1");
            ExecuteSqlStatement(fout, query->data);
            appendPQExpBufferStr(q, query->data);
        }"""
        _, problems = er.literals(body, "x.c", "f")
        self.assertEqual(len(problems), 1)
        self.assertIn("query", problems[0])

    def test_a_result_column_s_name_is_no_row(self):
        body = '{ appendPQExpBuffer(q, ",\\n    x = %s", PQgetvalue(res, 0, PQfnumber(res, "rngtype"))); }'
        self.assertEqual(rows_of(body), [",\n    x = "])

    def test_what_goes_to_stderr_is_no_row(self):
        body = '{ fprintf(stderr, _("bad name: \\"%s\\"\\n"), n); fprintf(OPF, "--\\n-- Tablespaces\\n"); }'
        self.assertEqual(rows_of(body), ["--\n-- Tablespaces\n"])

    def test_a_row_keeps_the_whole_constant_as_its_detail(self):
        (row,), _ = er.literals('{ ahprintf(AH, "-- TOC entry %d (class %u)\\n", a, b); }', "x.c", "f")
        self.assertEqual((row.entry, row.detail), ("-- TOC entry ", "-- TOC entry %d (class %u)\n"))


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
        schemas = {"s": {"default": [], "late": ("18", ["--statistics"]), "all": None}}
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

    def join(self, register: str, dump: str = "CREATE TABLE t (a int);\n"):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            write_tree(root, "18", register, dump)
            with unittest.mock.patch.object(gf, "SCHEMAS", {"types": {"default": []}}):
                return er.join_major("18", root)

    def test_a_literal_no_dump_holds_and_an_option_no_run_passes_are_uncovered(self):
        result, problems = self.join(er.render(self.ROWS, er.pinned_release("18")))
        self.assertEqual(problems, [])
        self.assertEqual([u.entry for u in result.uncovered], ["UNLOGGED ", "jobs"])
        self.assertEqual((result.literals, result.options), (2, 4))

    def test_a_register_from_another_minor_is_a_problem(self):
        _, problems = self.join(er.render(self.ROWS, "18.0"))
        self.assertTrue(any("read from 18.0" in p for p in problems), problems)

    def test_a_missing_register_is_a_problem(self):
        with TemporaryDirectory() as tmp:
            _, problems = er.join_major("18", Path(tmp))
        self.assertTrue(problems)


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
        with TemporaryDirectory() as tmp:
            checkout = Path(tmp)
            for file, _ in er.FUNCTIONS:
                (checkout / file).parent.mkdir(parents=True, exist_ok=True)
                (checkout / file).write_text("static void\nunrelated(void)\n{\n}\n")
            _, problems = er.extract(checkout)
        expected = sum(len(names) for _, names in er.FUNCTIONS)
        self.assertEqual(sum("no function" in p for p in problems), expected)


class CommittedTree(unittest.TestCase):
    """The committed registers against the committed fixtures."""

    def test_every_major_has_a_register_from_its_pinned_release(self):
        for major in er.majors():
            with self.subTest(major=major):
                _, problems = er.join_major(major)
                self.assertEqual(problems, [])

    def test_the_join_reports_without_failing(self):
        out = io.StringIO()
        self.assertEqual(er.main([], out=out), 0)
        self.assertIn("uncovered", out.getvalue())

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
            listed = {name for _, names in er.FUNCTIONS for name in names}
            with self.subTest(major=major):
                self.assertEqual(listed - seen, {"setup_connection"})


if __name__ == "__main__":
    unittest.main()
