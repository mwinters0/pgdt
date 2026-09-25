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
import sys
import tempfile
import unittest
from pathlib import Path

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

    def test_fmt_lists_each_file_once(self):
        o = check.summarize_fmt(FMT_DIFF, 1)
        self.assertEqual(o.status, "FAILED")
        self.assertEqual(o.details, ("/repo/pgdump_query/src/lib.rs", "/repo/pgdt/src/main.rs"))
        self.assertEqual(check.summarize_fmt("", 0).status, "ok")

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
            self.assertIn("did not pass; running every check", out.getvalue())

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
            self.assertIn("did not pass; running every check", before.getvalue())

    def test_the_real_checks_are_the_rounds_six_commands(self):
        self.assertEqual(
            [c.name for c in check.CHECKS],
            ["fmt", "clippy", "nextest", "doctest", "unittest", "repoint"],
        )
        for c in check.CHECKS:
            self.assertTrue((check.REPO / c.cwd).is_dir(), c.cwd)


if __name__ == "__main__":
    unittest.main()
