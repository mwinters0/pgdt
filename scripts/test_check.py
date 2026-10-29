#!/usr/bin/env python3
"""Unit tests for check.py, run with `uv run python -m unittest test_check`.

Each summarizer is fed the output shape its tool really prints, passing and
failing, because a summary that misreads a failure as a pass is worse than no
summary. The stamp and `--verify` cases build a real git repository in a
temporary directory and run stand-in checks there: what the stamp means is
defined by git, so a fake would test the fake.
"""

from __future__ import annotations

import io
import json
import subprocess
import sys
import tempfile
import unittest
import unittest.mock
from dataclasses import replace
from pathlib import Path
from typing import Collection, Sequence

import check
import repoint
from test_repoint import git, make_repo, write

NEXTEST_PASS = """\
    Starting 3 tests across 2 binaries
        PASS [   0.012s] (1/3) pgdump_query::stream a
        SLOW [> 60.000s] (─────────) pgdump_query::pruning b
        PASS [ 105.117s] (3/3) pgdump_query::pruning b
────────────
     Summary [ 115.658s] 3 tests run: 3 passed (1 slow), 0 skipped
"""

NEXTEST_FAIL = """\
        FAIL [  21.248s] (2/3) datafusion-pgdump::statistics never_change
    test never_change ... FAILED
    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 9 filtered out
────────────
     Summary [ 112.311s] 3 tests run: 2 passed, 1 failed, 0 skipped
        FAIL [  21.248s] (2/3) datafusion-pgdump::statistics never_change
error: test run failed
"""

NEXTEST_BUILD_ERROR = """\
   Compiling pgdump_query v0.1.0
error[E0425]: cannot find value `x` in this scope
error: could not compile `pgdump_query` (lib test) due to 1 previous error
"""

DOCTEST_PASS = """\
   Doc-tests datafusion_pgdump

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests pgdump_query

running 2 tests
test src/lib.rs - f (line 3) ... ok
test src/lib.rs - g (line 9) ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s
"""

DOCTEST_FAIL = """\
   Doc-tests pgdump_query

running 2 tests
test src/lib.rs - f (line 3) ... ok
test src/lib.rs - g (line 9) ... FAILED

failures:

---- src/lib.rs - g (line 9) stdout ----
assertion failed

failures:
    src/lib.rs - g (line 9)

test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.20s

error: doctest failed, to rerun pass `-p pgdump_query --doc`
"""

CLIPPY_CLEAN = """\
    Checking pgdump_query v0.1.0
    Finished `dev` profile [optimized + debuginfo] target(s) in 3.10s
"""

CLIPPY_WARNS = """\
    Checking pgdump_query v0.1.0
warning: unused variable: `x`
 --> pgdump_query/src/lib.rs:3:9
  |
3 |     let x = 1;
  |         ^ help: prefix it with an underscore: `_x`

warning: `pgdump_query` (lib) generated 1 warning
warning: unused variable: `x`
 --> pgdump_query/src/lib.rs:3:9
warning: `pgdump_query` (lib test) generated 1 warning (1 duplicate)
    Finished `dev` profile [optimized + debuginfo] target(s) in 3.10s
"""

# Two libraries' builds and a feature set's meeting one warning of the first's,
# and a done line.
ALONE_WARNS = """\
alone: base: cargo clippy -p base
    Checking base v0.1.0
warning: variant `Fetched` is never constructed
 --> base/src/feed.rs:5:5
warning: `base` (lib) generated 1 warning
alone: base: cargo clippy --all-targets -p base
warning: variant `Fetched` is never constructed
 --> base/src/feed.rs:5:5
alone: app: cargo clippy -p app
warning: variant `Fetched` is never constructed
 --> base/src/feed.rs:5:5
warning: unused import: `self::walk`
 --> app/src/lib.rs:2:5
alone: base/http: cargo clippy -p base --features http
warning: unused variable: `url`
 --> base/src/remote.rs:9:9
alone: 2 libraries, 1 feature sets, 4 builds
"""

ALONE_CLEAN = """\
alone: base: cargo clippy -p base
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.10s
alone: base: cargo clippy --all-targets -p base
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.10s
alone: 1 libraries, 0 feature sets, 2 builds
"""

FMT_DIFF = """\
Diff in /repo/pgdump_query/src/lib.rs:12:
-    let  x = 1;
+    let x = 1;
Diff in /repo/pgdump_query/src/lib.rs:40:
Diff in /repo/pgdt/src/main.rs:7:
"""

UNITTEST_OK = """\
..........s.
----------------------------------------------------------------------
Ran 12 tests in 3.214s

OK (skipped=1)
"""

UNITTEST_FAIL = """\
..F.E
======================================================================
ERROR: test_a (test_x.T.test_a)
----------------------------------------------------------------------
Traceback (most recent call last):
======================================================================
FAIL: test_b (test_y.T.test_b)
----------------------------------------------------------------------
AssertionError: OK
----------------------------------------------------------------------
Ran 5 tests in 0.100s

FAILED (failures=1, errors=1)
"""


class Summaries(unittest.TestCase):
    def test_nextest_passing_carries_its_summary_line(self):
        o = check.summarize_nextest(NEXTEST_PASS, 0)
        self.assertEqual(o.status, "ok")
        self.assertEqual(o.headline, "Summary [ 115.658s] 3 tests run: 3 passed (1 slow), 0 skipped")
        self.assertEqual(o.details, ())

    def test_nextest_failing_names_each_failed_test(self):
        o = check.summarize_nextest(NEXTEST_FAIL, 100)
        self.assertEqual(o.status, "FAILED")
        self.assertIn("1 failed", o.headline)
        self.assertEqual(
            o.details, ("FAIL [  21.248s] (2/3) datafusion-pgdump::statistics never_change",)
        )

    def test_nextest_that_never_ran_says_why(self):
        o = check.summarize_nextest(NEXTEST_BUILD_ERROR, 101)
        self.assertEqual(o.status, "FAILED")
        self.assertEqual(o.headline, "nextest printed no Summary line")
        self.assertIn("error[E0425]: cannot find value `x` in this scope", o.details)

    def test_a_zero_exit_is_not_trusted_without_a_summary_line(self):
        self.assertEqual(check.summarize_nextest("", 0).status, "FAILED")

    def test_doctests_sum_every_target(self):
        o = check.summarize_doctest(DOCTEST_PASS, 0)
        self.assertEqual((o.status, o.headline), ("ok", "2 doc-test targets: 2 passed; 0 failed"))

    def test_a_failing_doctest_is_named(self):
        o = check.summarize_doctest(DOCTEST_FAIL, 101)
        self.assertEqual(o.status, "FAILED")
        self.assertEqual(o.headline, "1 doc-test targets: 1 passed; 1 failed")
        self.assertEqual(o.details, ("src/lib.rs - g (line 9)",))

    def test_clean_clippy_passes(self):
        o = check.summarize_clippy(CLIPPY_CLEAN, 0)
        self.assertEqual((o.status, o.headline), ("ok", "0 warnings, 0 errors"))

    def test_a_clippy_warning_fails_though_clippy_exits_zero(self):
        o = check.summarize_clippy(CLIPPY_WARNS, 0)
        self.assertEqual(o.status, "FAILED")
        self.assertEqual(o.headline, "2 warnings, 0 errors")
        self.assertEqual(
            o.details, ("warning: unused variable: `x`  (pgdump_query/src/lib.rs:3:9)",)
        )

    def test_clean_alone_builds_pass(self):
        o = check.summarize_alone(ALONE_CLEAN, 0)
        self.assertEqual(
            (o.status, o.headline),
            ("ok", "1 libraries alone, 0 feature sets, 2 builds: 0 warnings, 0 errors"),
        )

    def test_an_alone_warning_is_counted_once_naming_the_first_library_meeting_it(self):
        o = check.summarize_alone(ALONE_WARNS, 0)
        self.assertEqual(o.status, "FAILED")
        self.assertEqual(
            o.headline, "2 libraries alone, 1 feature sets, 4 builds: 3 warnings, 0 errors"
        )
        self.assertEqual(
            o.details,
            (
                "warning: variant `Fetched` is never constructed  (base/src/feed.rs:5:5; alone: base)",
                "warning: unused import: `self::walk`  (app/src/lib.rs:2:5; alone: app)",
                "warning: unused variable: `url`  (base/src/remote.rs:9:9; alone: base/http)",
            ),
        )

    def test_a_failed_alone_build_fails_and_one_that_never_finished_says_so(self):
        failed = ALONE_CLEAN.replace("2 builds\n", "2 builds; 1 failed: cargo clippy -p base exited 101\n")
        o = check.summarize_alone(failed, 1)
        self.assertEqual(o.status, "FAILED")
        self.assertTrue(o.headline.startswith("1 libraries alone, 0 feature sets, 2 builds"))
        cut = check.summarize_alone(ALONE_CLEAN.rsplit("alone: 1", 1)[0], 1)
        self.assertEqual((cut.status, cut.headline), ("FAILED", "check.py --alone printed no result"))

    def test_fmt_lists_each_file_once(self):
        o = check.summarize_fmt(FMT_DIFF, 1)
        self.assertEqual(o.status, "FAILED")
        self.assertEqual(o.details, ("/repo/pgdump_query/src/lib.rs", "/repo/pgdt/src/main.rs"))
        self.assertEqual(check.summarize_fmt("", 0).status, "ok")

    def test_the_module_graph_names_each_cycle(self):
        ok = "== cycle owed: a::x, a::y: the next cut\nmodule-graph: the tree: 2 modules, 2 edges, 1 cycles, 0 accepted, 1 owed; 0 doc links against the edges (--docs lists them)\n"
        o = check.summarize_module_graph(ok, 0)
        self.assertEqual((o.status, o.headline), ("ok", ok.splitlines()[1]))
        bad = "!! cycle: a::x, a::y\n     a::x -> a::y\n" + ok.splitlines()[1] + "\n"
        o = check.summarize_module_graph(bad, 1)
        self.assertEqual((o.status, o.details), ("FAILED", ("!! cycle: a::x, a::y",)))

    def test_unittest_passing(self):
        o = check.summarize_unittest(UNITTEST_OK, 0)
        self.assertEqual((o.status, o.headline), ("ok", "Ran 12 tests in 3.214s; OK (skipped=1)"))

    def test_unittest_failing_names_each_case(self):
        o = check.summarize_unittest(UNITTEST_FAIL, 1)
        self.assertEqual(o.status, "FAILED")
        self.assertEqual(o.headline, "Ran 5 tests in 0.100s; FAILED (failures=1, errors=1)")
        self.assertEqual(
            o.details, ("ERROR: test_a (test_x.T.test_a)", "FAIL: test_b (test_y.T.test_b)")
        )

    def test_unittest_that_never_reported_fails(self):
        self.assertEqual(check.summarize_unittest("Traceback\nImportError\n", 1).status, "FAILED")

    def test_a_long_failure_list_is_cut_and_points_at_the_log(self):
        entry = {
            "name": "nextest", "status": "FAILED", "seconds": 1.0, "headline": "h",
            "details": [f"FAIL {i}" for i in range(check.DETAIL_LINES + 3)], "log": "L",
        }
        lines = check.render_check(entry).splitlines()
        self.assertEqual(len(lines), 1 + check.DETAIL_LINES + 1)
        self.assertEqual(lines[-1], "    ... 3 more in L")


class Repoint(unittest.TestCase):
    """The meter's line is recognized as the real script prints it."""

    def run_repoint(self, budget: int, extra: dict[str, str] | None = None) -> check.Outcome:
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp)
            for rel, text in (extra or {}).items():
                write(repo, rel, text)
            out = io.StringIO()
            code = repoint.check(repo, budget=budget, out=out)
            return check.summarize_repoint(out.getvalue(), code)

    def test_green(self):
        o = self.run_repoint(repoint.GROWTH_BUDGET)
        self.assertEqual(o.status, "ok")
        self.assertIn("repoint: green", o.headline)

    def test_a_red_meter_alone_is_not_a_failure(self):
        o = self.run_repoint(-1)
        self.assertEqual(o.status, "meter")
        self.assertEqual(len(o.details), 1)

    def test_a_red_register_meter_alone_is_not_a_failure(self):
        grown = "# A (`a.rs`)\n\n" + "x" * (repoint.REGISTER_GROWTH_BYTES + 1) + "\n\n### D1 x\nbody\n"
        o = self.run_repoint(repoint.GROWTH_BUDGET, {"docs/design/decisions/a.md": grown})
        self.assertEqual(o.status, "meter")
        self.assertEqual(len(o.details), 1)

    def test_a_cap_fails_even_beside_a_red_meter(self):
        o = self.run_repoint(-1, {"CLAUDE.md": "x\n" * 1000})
        self.assertEqual(o.status, "FAILED")


def stand_in(name: str, code: str, cwd: str = ".") -> check.Check:
    """A check running `python -c code`, summarized by its exit status alone."""
    return check.Check(
        name,
        (sys.executable, "-c", code),
        cwd,
        lambda text, exit_code: check.Outcome(
            "ok" if exit_code == 0 else "FAILED", text.strip() or "(silent)"
        ),
    )


PASSING = (stand_in("one", "print('fine')"), stand_in("two", "print('also fine')"))


def repo_with_runs_ignored(tmp: str) -> Path:
    repo = Path(tmp)
    write(repo, ".gitignore", "/runs\n")
    write(repo, "a.txt", "a\n")
    git(repo, "init", "-q", "-b", "main")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "base")
    return repo


class Stamp(unittest.TestCase):
    def test_a_clean_tree_stamps_as_head_s_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            self.assertEqual(check.tree_stamp(repo), git(repo, "rev-parse", "HEAD^{tree}"))

    def test_an_edit_and_an_untracked_file_each_move_it_and_an_ignored_one_does_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            clean = check.tree_stamp(repo)
            write(repo, "runs/x.log", "ignored\n")
            self.assertEqual(check.tree_stamp(repo), clean)
            write(repo, "new.txt", "untracked\n")
            untracked = check.tree_stamp(repo)
            self.assertNotEqual(untracked, clean)
            write(repo, "a.txt", "edited\n")
            self.assertNotIn(check.tree_stamp(repo), (clean, untracked))

    def test_the_real_index_is_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            write(repo, "new.txt", "untracked\n")
            index = (repo / ".git" / "index").read_bytes()
            check.tree_stamp(repo)
            self.assertEqual((repo / ".git" / "index").read_bytes(), index)
            self.assertEqual(git(repo, "status", "--porcelain"), "?? new.txt")


class RunAndVerify(unittest.TestCase):
    def test_a_run_logs_each_check_whole_and_records_the_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            out = io.StringIO()
            self.assertEqual(check.run(repo, PASSING, out), 0)
            (run_dir,) = (repo / check.RUNS).iterdir()
            record = json.loads((run_dir / "record.json").read_text())
            self.assertEqual(record["tree"], git(repo, "rev-parse", "HEAD^{tree}"))
            self.assertTrue(record["passed"])
            self.assertEqual((run_dir / "one.log").read_text(), "fine\n")
            self.assertEqual((run_dir / "summary.txt").read_text(), out.getvalue())
            self.assertEqual(out.getvalue().splitlines()[-1], "check: passed")

    def test_every_check_runs_though_one_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            checks = (stand_in("bad", "raise SystemExit(3)"), *PASSING)
            out = io.StringIO()
            self.assertEqual(check.run(repo, checks, out), 1)
            record = json.loads(next((repo / check.RUNS).glob("*/record.json")).read_text())
            self.assertEqual([c["exit"] for c in record["checks"]], [3, 0, 0])
            self.assertEqual(out.getvalue().splitlines()[-1], "check: FAILED (bad)")

    def test_a_missing_program_fails_its_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            missing = check.Check("gone", ("no-such-program-anywhere",), ".", check.summarize_fmt)
            self.assertEqual(check.run(repo, (missing,), io.StringIO()), 1)

    def test_verify_reuses_a_passing_run_of_this_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            first = io.StringIO()
            check.run(repo, PASSING, first)
            again = io.StringIO()
            self.assertEqual(check.verify(repo, PASSING, again), 0)
            self.assertEqual(len(list((repo / check.RUNS).iterdir())), 1)
            self.assertTrue(again.getvalue().startswith("check: reusing "))
            self.assertTrue(again.getvalue().endswith(first.getvalue()))

    def test_verify_runs_again_on_a_changed_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            check.run(repo, PASSING, io.StringIO())
            write(repo, "a.txt", "edited\n")
            out = io.StringIO()
            self.assertEqual(check.verify(repo, PASSING, out), 0)
            self.assertTrue(out.getvalue().startswith("check: no run of this tree"))
            self.assertEqual(len(list((repo / check.RUNS).iterdir())), 2)

    def test_verify_runs_again_for_a_different_list_of_checks(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            check.run(repo, PASSING[:1], io.StringIO())
            check.verify(repo, PASSING, io.StringIO())
            self.assertEqual(len(list((repo / check.RUNS).iterdir())), 2)

    def test_the_newest_run_of_a_tree_is_its_verdict(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            flag = repo / "runs" / "fail"
            flaky = (
                stand_in("flaky", f"import pathlib,sys; sys.exit(pathlib.Path({str(flag)!r}).exists())"),
            )
            check.run(repo, flaky, io.StringIO())
            flag.parent.mkdir(exist_ok=True)
            flag.write_text("")
            self.assertEqual(check.run(repo, flaky, io.StringIO()), 1)
            flag.unlink()
            out = io.StringIO()
            check.verify(repo, flaky, out)
            self.assertIn("did not pass; running the checks", out.getvalue())

    def test_a_later_failure_of_another_list_is_the_verdict(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            flag = repo / "runs" / "fail"
            flaky = stand_in("flaky", f"import pathlib,sys; sys.exit(pathlib.Path({str(flag)!r}).exists())")
            whole = (flaky, *PASSING)
            self.assertEqual(check.run(repo, whole, io.StringIO()), 0)
            flag.parent.mkdir(exist_ok=True)
            flag.write_text("")
            self.assertEqual(check.run(repo, (flaky,), io.StringIO()), 1)
            flag.unlink()
            out = io.StringIO()
            self.assertEqual(check.verify(repo, whole, out), 0)
            self.assertIn("did not pass; running the checks", out.getvalue())
            self.assertEqual(len(list((repo / check.RUNS).iterdir())), 3)

    def test_a_later_pass_of_another_list_leaves_the_earlier_pass_reusable(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            first = io.StringIO()
            check.run(repo, PASSING, first)
            check.run(repo, PASSING[:1], io.StringIO())
            out = io.StringIO()
            self.assertEqual(check.verify(repo, PASSING, out), 0)
            self.assertTrue(out.getvalue().startswith("check: reusing "))
            self.assertTrue(out.getvalue().endswith(first.getvalue()))
            self.assertEqual(len(list((repo / check.RUNS).iterdir())), 2)

    def test_a_tree_that_moves_during_the_run_verifies_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            edits = (stand_in("edits", "open('a.txt', 'w').write('moved\\n')"),)
            out = io.StringIO()
            self.assertEqual(check.run(repo, edits, out), 1)
            self.assertIn("the tree changed while the checks ran", out.getvalue())
            # Neither the tree the run started on nor the one it ended on is reused.
            after = io.StringIO()
            check.verify(repo, edits, after)
            self.assertTrue(after.getvalue().startswith("check: no run of this tree"))
            write(repo, "a.txt", "a\n")
            before = io.StringIO()
            check.verify(repo, edits, before)
            self.assertIn("did not pass; running the checks", before.getvalue())

    def test_the_real_checks_are_the_rounds_nine_commands(self):
        self.assertEqual(
            [c.name for c in check.CHECKS],
            ["fmt", "clippy", "alone", "nextest", "doctest", "graph", "unittest", "repoint", "seams"],
        )
        alone = next(c for c in check.CHECKS if c.name == "alone")
        self.assertEqual(alone.argv, check.alone_check(()).argv)
        self.assertTrue((check.REPO / alone.cwd / alone.argv[2]).is_file())
        for c in check.CHECKS:
            self.assertTrue((check.REPO / c.cwd).is_dir(), c.cwd)

    def test_every_cargo_check_runs_in_the_image_and_the_scripts_on_the_host(self):
        on_host = {c.name for c in check.CHECKS if not c.image}
        self.assertEqual(on_host, set(check.ALWAYS))
        self.assertTrue(check.doctest_check(["base"]).image)

    def test_an_image_check_is_run_and_recorded_as_release_py_runs_it(self):
        graph = next(c for c in check.CHECKS if c.name == "graph")
        argv, cwd = graph.command()
        self.assertEqual(
            list(argv),
            ["python3", "scripts/release.py", "run", "--cwd=scripts", "--", "uv", "run", "module_graph.py"],
        )
        self.assertEqual(cwd, ".")
        self.assertTrue((check.REPO / argv[1]).is_file())
        self.assertEqual(graph.key(), ["graph", list(argv), "."])
        on_host = next(c for c in check.CHECKS if c.name == "unittest")
        self.assertEqual(on_host.command(), (on_host.argv, "scripts"))
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            boxed = replace(stand_in("boxed", "pass"), image=True)
            stub = (sys.executable, "-c", "pass")
            with unittest.mock.patch.object(check, "IMAGE_RUN", stub):
                self.assertEqual(check.run(repo, [boxed], io.StringIO()), 0)
                ran = list(boxed.command()[0])
            record = check.runs_of(repo, check.tree_stamp(repo))[-1]
            self.assertEqual(record["checks"][0]["argv"], ran)
            self.assertEqual(ran[:3], list(stub))

    def test_a_note_is_printed_and_kept_in_the_summary(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            out = io.StringIO()
            check.run(repo, PASSING, out, note="affected: why")
            self.assertEqual(out.getvalue().splitlines()[1], "affected: why")
            (run_dir,) = (repo / check.RUNS).iterdir()
            self.assertEqual((run_dir / "summary.txt").read_text(), out.getvalue())

    def test_an_affected_verify_reuses_a_whole_run_of_the_tree(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            check.run(repo, PASSING, io.StringIO())
            out = io.StringIO()
            self.assertEqual(check.verify(repo, PASSING[1:], out, also=(PASSING,)), 0)
            self.assertTrue(out.getvalue().startswith("check: reusing "))
            self.assertEqual(len(list((repo / check.RUNS).iterdir())), 1)


class Literals(unittest.TestCase):
    def test_strings_are_read_past_comments_chars_and_lifetimes(self):
        src = (
            '// "docs/a.md"\n/* "x/y" /* nested */ "z" */\n'
            "fn f<'a>() { let c = '\"'; let e = '\\''; }\n"
            'let s = "../fixtures/{m}"; let r = r#"raw/"path"#; let b = b"by\\"te";\n'
            '/// let d = "../scripts";\n'
        )
        self.assertEqual(
            check.string_literals(src), ["../fixtures/{m}", 'raw/"path', 'by\\"te', "../scripts"]
        )

    def test_a_templated_literal_names_its_directory_and_a_bare_dot_nothing(self):
        self.assertEqual(check.literal_path("../fixtures/{major}/types"), "../fixtures/")
        self.assertEqual(check.literal_path("../fixtures/v{major}"), "../fixtures/")
        self.assertEqual(check.literal_path("tests/data/x.sql"), "tests/data/x.sql")
        self.assertEqual(check.literal_path("src"), "src")
        self.assertEqual(check.literal_path(".."), "..")
        for value in (".", "./", ".{}-{}.tmp", "{x}", "name{}", "./{x}", "a b/c", "/abs", "\\."):
            self.assertIsNone(check.literal_path(value), value)

    def test_a_literal_names_a_path_from_the_package_and_from_its_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            write(repo, "p/tests/common/mod.rs", 'f("../fixtures/{x}/y"); g("a b/c"); h("")')
            self.assertEqual(
                check.read_prefixes(repo, "p", "p/tests/common/mod.rs"),
                {"fixtures", "p/tests/fixtures"},
            )

    def test_modules_are_followed_from_a_test_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            write(repo, "p/tests/t.rs", "mod common;\nmod inline { }\n")
            write(repo, "p/tests/common/mod.rs", "pub mod oracle;\n")
            write(repo, "p/tests/common/oracle.rs", "")
            self.assertEqual(
                check.module_files(repo, "p/tests/t.rs"),
                ["p/tests/t.rs", "p/tests/common/mod.rs", "p/tests/common/oracle.rs"],
            )



class CodeOf(unittest.TestCase):
    """What a comment-only change may touch, and what it may not."""

    def same(self, a: str, b: str) -> bool:
        return check.code_of(a) == check.code_of(b)

    def test_comments_and_doc_prose_are_not_code(self):
        base = "/// Does f.\nfn f() -> u8 {\n    1 // one\n}\n"
        self.assertTrue(self.same(base, "/// Does f, rewritten\n/// over two lines.\nfn f() -> u8 {\n    1\n}\n"))
        self.assertTrue(self.same(base, "//! Module.\n/* block /* nested */ */\n/// Does f.\nfn f() -> u8 {\n    1\n}\n"))
        self.assertTrue(self.same(base, "/// Does f.\n//// four slashes is ordinary\nfn f() -> u8 { 1 }\n"))

    def test_a_doctest_is_code(self):
        doc = "/// ```\n/// assert_eq!(f(), 1);\n/// ```\nfn f() -> u8 { 1 }\n"
        self.assertFalse(self.same(doc, doc.replace("1);", "2);")))
        self.assertFalse(self.same(doc, doc.replace("/// ```\n///", "/// ```text\n///", 1)))
        self.assertTrue(self.same(doc, "/// Prose.\n" + doc))
        block = "/** ```\nf()\n``` */\nfn f() {}\n"
        self.assertFalse(self.same(block, block.replace("f()\n", "g()\n")))

    def test_a_comment_separates_tokens(self):
        self.assertFalse(self.same("let ab = 1;", "let a/* c */b = 1;"))
        self.assertTrue(self.same("let a b = 1;", "let a/* c */b = 1;"))

    def test_literals_are_verbatim(self):
        self.assertFalse(self.same('f("a  b")', 'f("a b")'))
        self.assertFalse(self.same('f("x // y")', 'f("x")'))
        self.assertFalse(self.same('f(r#"a\n\nb"#)', 'f(r#"a\nb"#)'))
        self.assertFalse(self.same("f(' ')", "f('_')"))
        self.assertTrue(self.same("fn f<'a>(x: &'a u8) {}", "fn f<'a>(x: &'a u8) {} // tail"))

    def test_code_and_its_spacing_are_code(self):
        self.assertFalse(self.same("f(a, b)", "f(a,b)"))
        self.assertFalse(self.same("f(a)", "g(a)"))


class HasDoctest(unittest.TestCase):
    """Which doc comments hold a block `cargo test --doc` compiles."""

    def test_a_rust_fence_is_a_doctest(self):
        for info in ("", "rust", "ignore", "no_run,should_panic", "compile_fail", "edition2021", "{.rust}"):
            self.assertTrue(check.has_doctest(f"/// ```{info}\n/// f();\n/// ```\nfn f() {{}}\n"), info)
        self.assertTrue(check.has_doctest("//! ~~~\n//! f();\n//! ~~~\n"))

    def test_another_language_is_not(self):
        for info in ("text", "sql", "sh", "rust-not", "ignore,sql"):
            self.assertFalse(check.has_doctest(f"/// ```{info}\n/// SELECT 1;\n/// ```\nfn f() {{}}\n"), info)

    def test_an_indented_block_is_one_and_an_ordinary_comment_never(self):
        self.assertTrue(check.has_doctest("/// Prose.\n///\n///     f();\nfn f() {}\n"))
        self.assertFalse(check.has_doctest("/// - item\n///   continued\nfn f() {}\n"))
        self.assertFalse(check.has_doctest("// ```\n// f();\n// ```\n//// ```\nfn f() {}\n"))

    def test_doc_text_it_does_not_read_counts(self):
        self.assertTrue(check.has_doctest("/** Docs. */\nfn f() {}\n"))
        self.assertTrue(check.has_doctest('#![doc = include_str!("../README.md")]\n'))
        self.assertFalse(check.has_doctest('fn f() { g("/**/"); }\n#[doc(hidden)]\nfn g() {}\n'))


WORKSPACE = {
    "Cargo.toml": '[workspace]\nresolver = "2"\nmembers = ["base", "app"]\n',
    "base/Cargo.toml": '[package]\nname = "base"\nversion = "0.1.0"\nedition = "2021"\n',
    "base/src/lib.rs": "/// ```\n/// base::f();\n/// ```\npub fn f() {}\n",
    "base/src/plain.rs": "pub fn g() {}\n",
    "base/tests/t.rs": "mod common;\n#[test]\nfn t() { common::p(); }\n",
    "base/tests/common/mod.rs": 'pub fn p() -> &\'static str { "../fixtures" }\n',
    "base/tests/data/x.sql": "",
    "app/Cargo.toml": (
        '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
        '[package.metadata.pgdt]\napparatus = true\n'
        '[dependencies]\nbase = { path = "../base" }\n'
    ),
    "app/src/main.rs": 'fn main() { let _ = "../base/tests/data/x.sql"; }\n',
    "app/tests/cli.rs": '// "../docs"\n#[test]\nfn t() {}\n',
    "fixtures/a.sql": "",
    "docs/d.md": "",
}


def argv_of(plan: check.Plan) -> dict[str, list[str]]:
    return {c.name: list(c.argv) for c in plan.checks}


class Affected(unittest.TestCase):
    """What `--affected` runs, over a two-package workspace `cargo metadata` reads."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.repo = Path(cls.tmp.name)
        for rel, text in WORKSPACE.items():
            write(cls.repo, rel, text)
        cls.packages = check.workspace(cls.repo)

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def plan(self, *changed: str) -> check.Plan:
        return check.plan_changes(self.repo, changed, lambda _: self.packages)

    def test_metadata_reads_the_targets_and_the_member_dependencies(self):
        by = {p.name: p for p in self.packages}
        self.assertEqual(by["app"].deps, {"base"})
        self.assertEqual(
            sorted(t.binary_id for t in by["base"].targets), ["base", "base::t"]
        )
        self.assertEqual(
            sorted(t.binary_id for t in by["app"].targets), ["app::bin/app", "app::cli"]
        )
        (t,) = [t for t in by["base"].targets if t.kind == "test"]
        self.assertEqual(t.sources, ("base/tests/t.rs", "base/tests/common/mod.rs"))
        # The manifest's marker, read for the release suite alone.
        self.assertEqual({n: p.apparatus for n, p in by.items()}, {"app": True, "base": False})

    def test_an_affected_run_builds_in_the_image_as_a_whole_one_does(self):
        plan = self.plan("base/src/lib.rs")
        built = [c for c in plan.checks if c.name not in check.ALWAYS]
        self.assertEqual(
            [c.name for c in built], ["fmt", "clippy", "alone", "nextest", "doctest", "graph"]
        )
        self.assertTrue(all(c.image for c in built))

    def test_a_markdown_file_is_read_only_by_a_literal_naming_it(self):
        self.assertFalse(check.reads("p/tests/data", "p/tests/data/README.md"))
        self.assertTrue(check.reads("p/README.md", "p/README.md"))
        self.assertTrue(check.reads("p/tests/data", "p/tests/data/x.sql"))

    def test_only_paths_no_test_reads_run_no_cargo_and_read_no_metadata(self):
        def refuse(_):
            raise AssertionError("cargo metadata was read")

        plan = check.plan_changes(self.repo, ["docs/d.md", "app/README.md", ".claude/x"], refuse)
        self.assertEqual([c.name for c in plan.checks], ["unittest", "repoint", "seams"])
        self.assertIn("none read by a test", plan.note)

    def test_a_library_change_runs_its_dependents(self):
        argv = argv_of(self.plan("base/src/lib.rs"))
        self.assertEqual(argv["nextest"][4:], ["-p", "app", "-p", "base"])
        self.assertEqual(argv["clippy"], ["cargo", "clippy", "--all-targets", "-p", "app", "-p", "base"])
        # `app` has no library, so no doctests.
        self.assertEqual(argv["doctest"], ["cargo", "test", "--doc", "--no-fail-fast", "-p", "base"])
        self.assertEqual(argv["graph"], ["uv", "run", "module_graph.py"])
        # `app` has no library, so `base` alone is linted alone.
        self.assertEqual(argv["alone"], ["uv", "run", "check.py", "--alone", "base"])

    def test_a_change_reaching_no_library_lints_none_alone(self):
        plan = self.plan("app/src/main.rs")
        self.assertIn("clippy", argv_of(plan))
        self.assertNotIn("alone", argv_of(plan))

    def test_each_library_is_built_alone_twice_and_a_binary_never(self):
        self.assertEqual(
            check.alone_builds(self.packages),
            [
                ("base", ("cargo", "clippy", "-p", "base")),
                ("base", ("cargo", "clippy", "--all-targets", "-p", "base")),
            ],
        )
        self.assertEqual(check.alone_builds(self.packages, ["base"]), check.alone_builds(self.packages))
        with self.assertRaisesRegex(ValueError, "not a workspace library or feature set: app"):
            check.alone_builds(self.packages, ["app"])

    def test_every_alone_build_runs_though_one_fails_and_the_count_closes_it(self):
        two = [replace(p, name=n) for p in self.packages if p.name == "base" for n in ("one", "two")]
        builds = ((sys.executable, "-c", "import sys; sys.exit(sys.argv[-1] == 'one')"),)
        out = io.StringIO()
        with unittest.mock.patch.object(check, "ALONE_BUILDS", builds), \
                unittest.mock.patch.object(check.sys, "stdout", out):
            code = check.run_alone(self.repo, (), lambda _: two)
        self.assertEqual(code, 1)
        lines = out.getvalue().splitlines()
        self.assertEqual([line.split(": ")[1] for line in lines[:2]], ["one", "two"])
        self.assertTrue(
            lines[-1].startswith("alone: 2 libraries, 0 feature sets, 2 builds; 1 failed: ")
        )
        o = check.summarize_alone(out.getvalue(), code)
        self.assertEqual(o.status, "FAILED")

    def test_a_library_holding_no_doctest_runs_none(self):
        plain = [
            replace(p, targets=tuple(
                replace(t, sources=("base/src/plain.rs",)) if t.kind == "lib" else t
                for t in p.targets
            ))
            for p in self.packages
        ]
        plan = check.plan_changes(self.repo, ["base/src/lib.rs"], lambda _: plain)
        self.assertNotIn("doctest", argv_of(plan))
        self.assertNotIn("doctest", [c.name for c in check.every_check(self.repo, lambda _: plain)])

    def test_a_comment_only_change_lints_its_package_and_runs_no_test(self):
        plan = check.plan_changes(
            self.repo, ["base/src/lib.rs"], lambda _: self.packages, {"base/src/lib.rs"}
        )
        self.assertEqual(
            [c.name for c in plan.checks], ["fmt", "clippy", "alone", "unittest", "repoint", "seams"]
        )
        self.assertEqual(argv_of(plan)["clippy"][-2:], ["-p", "base"])
        self.assertEqual(argv_of(plan)["alone"][-1], "base")
        self.assertIn("linted only, their changes being comments, base", plan.note)

    def test_a_feature_set_is_linted_at_both_builds_beside_the_libraries(self):
        sets = (
            check.FeatureBuild("app", ("x",), default_features=False),
            check.FeatureBuild("base", ("y",), workspace=True),
            check.FeatureBuild("elsewhere", ("z",)),
        )
        with unittest.mock.patch.object(check, "FEATURE_BUILDS", sets):
            self.assertEqual(
                check.alone_builds(self.packages)[2:],
                [
                    (name, (*build, *args))
                    for name, args in (
                        ("app/x", ("-p", "app", "--no-default-features", "--features", "x")),
                        ("base/y", ("--workspace", "--features", "base/y")),
                    )
                    for build in check.ALONE_BUILDS
                ],
            )
            self.assertEqual(
                [n for n, _ in check.alone_builds(self.packages, ["app/x"])], ["app/x", "app/x"]
            )
            # A set of a package the workspace does not hold is no set of it.
            with self.assertRaisesRegex(ValueError, "feature set: elsewhere/z"):
                check.alone_builds(self.packages, ["elsewhere/z"])
            out = io.StringIO()
            builds = ((sys.executable, "-c", "pass"),)
            with unittest.mock.patch.object(check, "ALONE_BUILDS", builds), \
                    unittest.mock.patch.object(check.sys, "stdout", out):
                code = check.run_alone(self.repo, (), lambda _: self.packages)
            self.assertEqual(code, 0)
            done = out.getvalue().splitlines()[-1]
            self.assertEqual(done, "alone: 1 libraries, 2 feature sets, 3 builds")
            self.assertEqual(check.summarize_alone(out.getvalue(), code).status, "ok")

    def test_a_feature_set_runs_where_the_change_reaches_its_package(self):
        sets = (
            check.FeatureBuild("app", ("x",), default_features=False),
            check.FeatureBuild("base", ("y",), workspace=True),
        )
        with unittest.mock.patch.object(check, "FEATURE_BUILDS", sets):
            # A binary's change lints no library alone, but its package's sets.
            argv = argv_of(self.plan("app/src/main.rs"))
            self.assertEqual(argv["alone"][3:], ["--alone", "app/x", "base/y"])
            argv = argv_of(self.plan("base/src/lib.rs"))
            self.assertEqual(argv["alone"][3:], ["--alone", "app/x", "base", "base/y"])
            # A comment reaches its package alone, a workspace-wide set every one.
            plan = check.plan_changes(
                self.repo, ["base/src/lib.rs"], lambda _: self.packages, {"base/src/lib.rs"}
            )
            self.assertEqual(argv_of(plan)["alone"][3:], ["--alone", "base", "base/y"])
            plan = self.plan("docs/d.md")
            self.assertNotIn("alone", argv_of(plan))

    def test_a_comment_only_change_still_runs_a_target_reading_it(self):
        changed = ["base/tests/data/x.sql", "app/src/main.rs"]
        plan = check.plan_changes(self.repo, changed, lambda _: self.packages, {"app/src/main.rs"})
        argv = argv_of(plan)
        self.assertEqual(argv["nextest"][-2:], ["-E", "package(=base) | binary_id(=app::bin/app)"])
        self.assertEqual(argv["clippy"][-4:], ["-p", "app", "-p", "base"])
        self.assertNotIn("app", argv["doctest"])

    def test_a_comment_beside_a_code_change_to_its_package_changes_nothing(self):
        changed = ["base/src/lib.rs", "base/src/other.rs"]
        plan = check.plan_changes(self.repo, changed, lambda _: self.packages, {"base/src/other.rs"})
        self.assertEqual(argv_of(plan)["nextest"][4:], ["-p", "app", "-p", "base"])
        self.assertNotIn("linted only", plan.note)

    def test_comment_only_paths_compares_the_two_trees(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            write(repo, "c.rs", "fn c() {}\n")
            write(repo, "d.rs", "fn d() {}\n")
            git(repo, "add", "-A")
            git(repo, "commit", "-q", "-m", "rs")
            head = check.head_tree(repo)[1]
            write(repo, "c.rs", "/// Says c.\nfn c() {}\n")
            write(repo, "d.rs", "fn d() { () }\n")
            write(repo, "e.rs", "// new\n")
            write(repo, "a.txt", "edited\n")
            tree = check.tree_stamp(repo)
            changed = check.changed_paths(repo, head, tree)
            self.assertEqual(check.comment_only_paths(repo, head, tree, changed), {"c.rs"})

    def test_a_test_change_runs_its_own_package_only(self):
        argv = argv_of(self.plan("base/tests/t.rs"))
        self.assertEqual(argv["nextest"], ["cargo", "nextest", "run", "--no-fail-fast", "-p", "base"])

    def test_a_read_path_runs_its_readers_and_lints_nothing(self):
        plan = self.plan("fixtures/a.sql")
        self.assertEqual([c.name for c in plan.checks], ["nextest", "unittest", "repoint", "seams"])
        self.assertEqual(
            argv_of(plan)["nextest"],
            ["cargo", "nextest", "run", "--no-fail-fast", "-p", "base", "-E", "binary_id(=base::t)"],
        )

    def test_another_package_reading_a_path_runs_beside_it(self):
        argv = argv_of(self.plan("base/tests/data/x.sql"))
        self.assertEqual(
            argv["nextest"][-2:], ["-E", "package(=base) | binary_id(=app::bin/app)"]
        )
        self.assertEqual(argv["clippy"][-2:], ["-p", "base"])

    def test_a_path_in_no_crate_and_read_by_nothing_runs_everything(self):
        for path in ("Cargo.toml", "vendor/x/src/lib.rs"):
            plan = self.plan("base/src/lib.rs", path)
            self.assertEqual(plan.checks, check.every_check(self.repo, lambda _: self.packages))
            self.assertEqual(
                argv_of(plan)["doctest"], ["cargo", "test", "--doc", "--no-fail-fast", "-p", "base"]
            )
            self.assertIn(path, plan.note)

    def test_the_change_is_the_stamped_tree_against_head(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = repo_with_runs_ignored(tmp)
            plan = check.plan_affected(repo, check.tree_stamp(repo), check.head_tree(repo)[1])
            self.assertIn("nothing changed", plan.note)
            write(repo, "docs/new.md", "x\n")
            write(repo, "a.txt", "edited\n")
            tree = check.tree_stamp(repo)
            self.assertEqual(
                check.changed_paths(repo, check.head_tree(repo)[1], tree), ["a.txt", "docs/new.md"]
            )
            self.assertEqual(
                check.plan_affected(repo, tree, None, lambda _: []).checks,
                tuple(c for c in check.CHECKS if c.name != "doctest"),
            )


class Support(unittest.TestCase):
    """A library only dev-dependencies reach reads for its dependents' tests."""

    def test_a_path_a_support_library_reads_runs_every_target_depending_on_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            files = {
                "Cargo.toml": '[workspace]\nresolver = "2"\nmembers = ["vocab", "base", "app"]\n',
                "vocab/Cargo.toml": '[package]\nname = "vocab"\nversion = "0.1.0"\nedition = "2021"\n',
                "vocab/src/lib.rs": 'pub fn root() -> &\'static str { "../fixtures" }\n',
                "base/Cargo.toml": (
                    '[package]\nname = "base"\nversion = "0.1.0"\nedition = "2021"\n'
                    '[dev-dependencies]\nvocab = { path = "../vocab" }\n'
                ),
                "base/src/lib.rs": "pub fn f() {}\n",
                "base/tests/t.rs": "#[test]\nfn t() {}\n",
                "app/Cargo.toml": (
                    '[package]\nname = "app"\nversion = "0.1.0"\nedition = "2021"\n'
                    '[dependencies]\nbase = { path = "../base" }\n'
                ),
                "app/src/main.rs": "fn main() {}\n",
                "fixtures/a.sql": "",
            }
            for rel, text in files.items():
                write(repo, rel, text)
            packages = check.workspace(repo)
            self.assertEqual([p.name for p in packages if p.support], ["vocab"])
            plan = check.plan_changes(repo, ["fixtures/a.sql"], lambda _: packages)
            self.assertEqual(
                argv_of(plan)["nextest"][-2:],
                ["-E", "binary_id(=base) | binary_id(=base::t) | binary_id(=vocab)"],
            )


Roots = list[tuple[str, frozenset[str], bool]]


def build_of(argv: Sequence[str], members: Collection[str]) -> tuple[Roots, bool]:
    """What a `cargo clippy` argv builds, as `features_on` takes it: each
    selected package with the features asked of it and whether it keeps its
    defaults, and whether every target is built. Only the flags the round's
    builds pass are read; any other is refused."""
    selected: list[str] = []
    asked: list[str] = []
    default, all_targets, i = True, False, 2
    while i < len(argv):
        a = argv[i]
        if a == "--workspace":
            selected = sorted(members)
        elif a == "-p":
            i += 1
            selected.append(argv[i])
        elif a == "--features":
            i += 1
            asked += argv[i].split(",")
        elif a == "--no-default-features":
            default = False
        elif a == "--all-targets":
            all_targets = True
        else:
            raise ValueError(f"a flag this reading does not know: {a}")
        i += 1
    per: dict[str, set[str]] = {name: set() for name in selected}
    for f in asked:
        pkg, _, feat = f.rpartition("/")
        if not pkg:
            (pkg,) = selected
        if pkg not in per:
            raise ValueError(f"a feature of a package the build does not select: {f}")
        per[pkg].add(feat)
    return [(n, frozenset(fs), default) for n, fs in per.items()], all_targets


def features_on(
    meta: dict, roots: Sequence[tuple[str, Collection[str], bool]], all_targets: bool
) -> set[str]:
    """The `package/feature`s of the workspace's members one build turns on,
    read off `cargo metadata --no-deps` as resolver 2 unifies them within a
    build: each root under the features asked and, if it keeps them, its
    defaults; a member dependency under its declaration's; `dep:`, `x/f` and
    `x?/f` as cargo reads them; a dev-dependency only of a root, and only
    with every target built."""
    members = set(meta["workspace_members"])
    pkgs = {p["name"]: p for p in meta["packages"] if p["id"] in members}
    by_dir = {Path(p["manifest_path"]).parent.resolve(): n for n, p in pkgs.items()}
    root_names = {r for r, _, _ in roots}
    on: set[tuple[str, str]] = set()
    active: set[str] = set()
    active_deps: set[tuple[str, str]] = set()
    weak: set[tuple[str, str, str]] = set()

    def alias(d):
        return d.get("rename") or d["name"]

    def decls(name):
        for d in pkgs[name]["dependencies"]:
            if d.get("kind") != "dev" or (all_targets and name in root_names):
                yield d

    def member(d):
        return by_dir.get(Path(d["path"]).resolve()) if d.get("path") else None

    def enable(name, d):
        active_deps.add((name, alias(d)))
        if (m := member(d)) is not None:
            activate(m)
            if d["uses_default_features"]:
                feature(m, "default")
            for f in d["features"]:
                apply(m, f)

    def activate(name):
        if name not in active:
            active.add(name)
            for d in decls(name):
                if not d["optional"]:
                    enable(name, d)

    def optional(name, a):
        for d in decls(name):
            if d["optional"] and alias(d) == a:
                enable(name, d)

    def feature(name, f):
        if (name, f) in on:
            return
        activate(name)
        declared = pkgs[name]["features"]
        if f in declared:
            on.add((name, f))
            for entry in declared[f]:
                apply(name, entry)
        else:
            optional(name, f)  # an optional dependency's implicit feature

    def through(name, a, f):
        for d in decls(name):
            if alias(d) == a and (m := member(d)) is not None:
                feature(m, f)

    def apply(name, entry):
        if entry.startswith("dep:"):
            optional(name, entry[4:])
        elif "/" in entry:
            a, f = entry.split("/", 1)
            if a.endswith("?"):
                weak.add((name, a[:-1], f))
            else:
                optional(name, a)
                through(name, a, f)
        else:
            feature(name, entry)

    for name, asked, default in roots:
        activate(name)
        if default:
            feature(name, "default")
        for f in asked:
            feature(name, f)
    done: set[tuple[str, str, str]] = set()
    while pending := {w for w in weak - done if w[:2] in active_deps}:
        for name, a, f in pending:
            through(name, a, f)
        done |= pending
    return {f"{n}/{f}" for n, f in on if f != "default"}


def synthetic_meta() -> dict:
    """A three-member workspace `features_on` is read against, by hand."""
    def pkg(name, features, deps):
        return {
            "id": name, "name": name, "manifest_path": f"/w/{name}/Cargo.toml",
            "features": features, "dependencies": deps,
        }

    def dep(name, *, kind=None, optional=False, default=True, features=(), path=True):
        return {
            "name": name, "rename": None, "kind": kind, "optional": optional,
            "uses_default_features": default, "features": list(features),
            "path": f"/w/{name}" if path else None,
        }

    return {
        "workspace_members": ["core", "io", "app"],
        "packages": [
            pkg("core", {"default": ["fast"], "fast": [], "trace": [], "test-support": []}, []),
            pkg(
                "io",
                {"http": ["dep:remote"], "trace": ["core/trace"], "weak": ["core?/test-support"]},
                [dep("core"), dep("remote", optional=True, path=False),
                 dep("core", kind="dev", features=["test-support"])],
            ),
            pkg(
                "app",
                {"default": ["io/http"], "lean": [], "deep": ["io/trace", "io/weak"]},
                [dep("io", default=False)],
            ),
        ],
    }


class FeatureResolution(unittest.TestCase):
    """`features_on`, which the coverage assertion stands on, read against a
    workspace whose answers are worked by hand."""

    def test_defaults_declarations_and_forwarded_features(self):
        meta = synthetic_meta()
        self.assertEqual(features_on(meta, [("app", (), True)], False), {"io/http", "core/fast"})
        self.assertEqual(features_on(meta, [("app", (), False)], False), {"core/fast"})
        self.assertEqual(
            features_on(meta, [("app", ("deep",), False)], False),
            {"app/deep", "io/trace", "io/weak", "core/trace", "core/test-support", "core/fast"},
        )

    def test_a_dev_dependency_is_built_only_for_a_root_with_every_target(self):
        meta = synthetic_meta()
        self.assertNotIn("core/test-support", features_on(meta, [("io", (), True)], False))
        self.assertIn("core/test-support", features_on(meta, [("io", (), True)], True))
        # `io`'s dev-dependency is not built for `app`'s tests.
        self.assertNotIn("core/test-support", features_on(meta, [("app", (), True)], True))

    def test_an_argv_reads_as_the_build_cargo_makes(self):
        members = ("app", "core", "io")
        self.assertEqual(
            build_of(("cargo", "clippy", "--all-targets", "-p", "io"), members),
            ([("io", frozenset(), True)], True),
        )
        lean = ("cargo", "clippy", "-p", "app", "--no-default-features", "--features", "deep")
        self.assertEqual(build_of(lean, members), ([("app", frozenset({"deep"}), False)], False))
        roots, _ = build_of(("cargo", "clippy", "--workspace", "--features", "io/trace"), members)
        self.assertEqual(
            sorted(roots),
            [("app", frozenset(), True), ("core", frozenset(), True), ("io", frozenset({"trace"}), True)],
        )
        with self.assertRaisesRegex(ValueError, "--release"):
            build_of(("cargo", "clippy", "--release"), members)
        with self.assertRaisesRegex(ValueError, "does not select: core/trace"):
            build_of(("cargo", "clippy", "-p", "io", "--features", "core/trace"), members)


class RealWorkspace(unittest.TestCase):
    """The classification holds of this repository's own tests."""

    @classmethod
    def setUpClass(cls):
        cls.packages = check.workspace(check.REPO)
        cls.prefixes = {
            t.binary_id: set().union(*(check.read_prefixes(check.REPO, p.dir, s) for s in t.sources))
            for p in cls.packages
            for t in p.targets
        }

    def test_no_rust_target_reads_a_path_no_test_reads(self):
        tracked = git(check.REPO, "ls-files").splitlines()
        untested = [p for p in tracked if check.UNTESTED_RE.search(p)]
        self.assertTrue(untested)
        for bid, prefixes in self.prefixes.items():
            read = [p for p in untested if any(check.reads(x, p) for x in prefixes)]
            self.assertEqual(read, [], bid)
            whole = [x for x in prefixes if x in (".", "docs", ".claude")]
            self.assertEqual(whole, [], bid)

    def test_a_script_module_a_rust_test_runs_is_one_the_scripts_check_runs(self):
        named = {
            lit
            for p in self.packages
            for t in p.targets
            for s in t.sources
            for lit in check.string_literals((check.REPO / s).read_text(encoding="utf-8"))
            if lit.startswith("test_") and lit.isidentifier()
        }
        self.assertIn("test_measure", named)
        for module in named:
            self.assertTrue((check.REPO / "scripts" / f"{module}.py").is_file(), module)

    def test_a_fixture_reaches_the_sweeps_and_a_binary_change_its_package(self):
        load = lambda _: self.packages  # noqa: E731
        fixture = check.plan_changes(check.REPO, ["fixtures/16/statistics/default.sql"], load)
        nextest = argv_of(fixture)["nextest"]
        self.assertIn("binary_id(=pgdt-query::pruning)", nextest[-1])
        self.assertIn("binary_id(=datafusion-pgdump::pushdown)", nextest[-1])
        plan = check.plan_changes(check.REPO, ["crates/pgdt/src/main.rs"], load)
        self.assertEqual(argv_of(plan)["nextest"][4:], ["-p", "pgdt"])

    def test_a_library_reads_its_data_and_not_its_whole_package(self):
        # `copy.rs` holds the value "." and `cache.rs` the template
        # ".{}-{}.tmp", each of which read as the package directory once;
        # the fixture vocabulary reads the tree and its own hand-written dump.
        libs = {p.name: t for p in self.packages for t in p.targets if t.kind == "lib"}
        prefixes = self.prefixes[libs["pgdt-fixtures"].binary_id]
        self.assertIn("fixtures", prefixes)
        self.assertIn("crates/pgdt-fixtures/edge_cases.sql", prefixes)
        self.assertEqual([p.name for p in self.packages if p.support], ["pgdt-fixtures"])
        for name in ("pgdt-core", "pgdt-dump", "pgdt-mapping"):
            prefixes = self.prefixes[libs[name].binary_id]
            self.assertNotIn(f"crates/{name}", prefixes)
            self.assertNotIn(f"crates/{name}/src", prefixes)
        load = lambda _: self.packages  # noqa: E731
        plan = check.plan_changes(
            check.REPO, ["crates/pgdt-core/src/copy.rs"], load, {"crates/pgdt-core/src/copy.rs"}
        )
        self.assertNotIn("nextest", argv_of(plan))
        self.assertNotIn("doctest", argv_of(plan))

    def test_every_member_holding_a_library_is_linted_alone(self):
        sets = {s.name for s in check.FEATURE_BUILDS}
        alone = {name for name, _ in check.alone_builds(self.packages)} - sets
        self.assertEqual(alone, {p.name for p in self.packages if p.has_lib})
        # The two a feature the workspace unifies on once hid dead code in.
        self.assertLessEqual({"pgdt-io", "pgdt-mapping"}, alone)
        self.assertNotIn("pgdt", alone)

    def test_every_declared_feature_is_turned_on_by_a_linted_build_or_exempt(self):
        meta = json.loads(
            subprocess.run(
                ["cargo", "metadata", "--no-deps", "--format-version", "1"],
                cwd=check.REPO, check=True, capture_output=True, text=True,
            ).stdout
        )
        members = [p["name"] for p in meta["packages"] if p["id"] in meta["workspace_members"]]
        declared = {
            f"{p['name']}/{f}"
            for p in meta["packages"]
            if p["id"] in meta["workspace_members"]
            for f in p["features"]
            if f != "default"
        }
        clippy = next(c for c in check.CHECKS if c.name == "clippy")

        def linted(builds):
            out: set[str] = set()
            for argv in builds:
                out |= features_on(meta, *build_of(argv, members))
            return out

        libraries = [argv for name, argv in check.alone_builds(self.packages) if "/" not in name]
        everything = linted([clippy.argv, *(argv for _, argv in check.alone_builds(self.packages))])
        self.assertEqual(sorted(declared - everything - set(check.UNLINTED_FEATURES)), [])
        # An exemption names a declared feature nothing lints, and says why.
        self.assertLessEqual(set(check.UNLINTED_FEATURES), declared)
        self.assertEqual(sorted(set(check.UNLINTED_FEATURES) & everything), [])
        self.assertTrue(all(why.strip() for why in check.UNLINTED_FEATURES.values()))
        # Each set is this workspace's, and the reason its features are linted.
        self.assertEqual(check.feature_sets(self.packages), list(check.FEATURE_BUILDS))
        without = linted([clippy.argv, *libraries])
        for s in check.FEATURE_BUILDS:
            self.assertLessEqual({f"{s.package}/{f}" for f in s.features}, declared, s.name)
        self.assertLessEqual(
            {"pgdt/introspect", "pgdt-core/introspect", "pgdt/jemalloc", "pgdt-gzip-seek/zlib-ng"},
            declared - without,
        )
        self.assertLessEqual({"pgdt-io/http", "pgdt-io/test-support", "pgdt-gzip-seek/zlib-rs"}, without)

    def test_the_commit_editing_only_doc_comments_runs_no_test(self):
        # 5c835d4b filed two deficiencies by marker comments in two library
        # files; path rules alone ran the whole suite for it.
        commit = "5c835d4b"
        head, tree = (git(check.REPO, "rev-parse", f"{c}^{{tree}}") for c in (f"{commit}^", commit))
        changed = check.changed_paths(check.REPO, head, tree)
        rs = [p for p in changed if p.endswith(".rs")]
        self.assertEqual(rs, ["pgdump_query/src/decode.rs", "pgdump_query/src/preamble.rs"])
        self.assertEqual(check.comment_only_paths(check.REPO, head, tree, changed), set(rs))


if __name__ == "__main__":
    unittest.main()
