#!/usr/bin/env python3
"""Unit tests for repoint.py, run with `uv run python -m unittest test_repoint`.

Each cap and each exemption has a case, because a check that misses reports the
tree cleaner than it is and one that cries wolf stops being run. The meter's
cases build a real git repository in a temporary directory: what it counts is
defined by `git diff`, so a fake would test the fake.
"""

from __future__ import annotations

import io
import subprocess
import tempfile
import unittest
from pathlib import Path

import repoint

FILES = {
    "docs/design/decisions.md": "# Decisions\n\n## A (`a.rs`)\n### D1 x\nbody\n",
    "docs/status/STATUS.md": (
        "# Status\n\n<!-- repointed: STAMP -->\n\n## What exists\n\n"
        "| a | b |\n|---|---|\n| x | y |\n\n## Known deficiencies\n\n- **KD1** — 5 MiB. Detail: x\n"
    ),
    "docs/status/history/README.md": "# History\n",
    "docs/design/roadmap.md": "# Roadmap\n",
    "CLAUDE.md": "# C\n",
    "docs/process.md": "# P\n",
    ".claude/skills/x/SKILL.md": "---\nname: x\n---\n",
    "pgdump_query/src/lib.rs": "//! lib\nfn f() {}\n",
}


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-c", "user.name=t", "-c", "user.email=t@t", *args],
        cwd=repo, check=True, capture_output=True, text=True,
    ).stdout.strip()


def write(repo: Path, rel: str, text: str) -> None:
    path = repo / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def make_repo(tmp: str) -> Path:
    repo = Path(tmp)
    for rel, text in FILES.items():
        write(repo, rel, text)
    git(repo, "init", "-q", "-b", "main")
    git(repo, "add", "-A")
    git(repo, "commit", "-q", "-m", "base")
    sha = git(repo, "rev-parse", "--short", "HEAD")
    write(repo, "docs/status/STATUS.md", FILES["docs/status/STATUS.md"].replace("STAMP", sha))
    git(repo, "commit", "-qam", "stamp")
    return repo


class Register(unittest.TestCase):
    def check(self, text: str) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            write(Path(tmp), "docs/design/decisions.md", text)
            return repoint.register_problems(Path(tmp))

    def test_an_entry_over_seven_body_lines_is_named(self):
        text = "# D\n\n## S\n### D3 t\n" + "l\n" * 8 + "### D4 t\nl\n"
        self.assertEqual([p for p in self.check(text) if "D3" in p and "8 lines" in p].__len__(), 1)
        self.assertFalse([p for p in self.check(text) if "D4" in p])

    def test_blank_lines_do_not_count(self):
        text = "# D\n\n## S\n### D3 t\n" + "l\n\n" * 7
        self.assertEqual(self.check(text), [])

    def test_the_file_cap_is_held(self):
        text = "# D\n" + "\n" * (repoint.REGISTER_LINES + 1)
        self.assertTrue(any("cap of" in p for p in self.check(text)))

    def test_a_measured_quantity_is_named(self):
        for bad in ("runs 5.6× faster", "holds 384 MiB", "costs 0.113 s", "20% headroom"):
            self.assertTrue(any("quantity" in p for p in self.check(f"# D\n### D1 t\n{bad}\n")), bad)

    def test_a_handle_or_a_constant_name_is_not_a_quantity(self):
        self.assertEqual(self.check("# D\n### D1 t\n`POOL_DEPTH` slots, D46, I3, `scan-throughput-*`\n"), [])


class History(unittest.TestCase):
    def test_an_entry_after_the_cutoff_is_held_to_the_cap(self):
        with tempfile.TemporaryDirectory() as tmp:
            long = "x\n" * (repoint.HISTORY_LINES + 1)
            write(Path(tmp), "docs/status/history/2026-09-14.md", long)
            write(Path(tmp), "docs/status/history/2026-09-12.md", long)
            write(Path(tmp), "docs/status/history/README.md", long)
            problems = repoint.history_problems(Path(tmp))
        self.assertEqual(len(problems), 1)
        self.assertIn("2026-09-14", problems[0])


class Orientation(unittest.TestCase):
    def test_a_skill_and_claude_md_are_capped(self):
        with tempfile.TemporaryDirectory() as tmp:
            write(Path(tmp), ".claude/skills/big/SKILL.md", "x\n" * (repoint.SKILL_LINES + 1))
            write(Path(tmp), ".claude/skills/ok/SKILL.md", "x\n")
            write(Path(tmp), "CLAUDE.md", "x\n" * (repoint.ORIENTATION_CAPS["CLAUDE.md"] + 1))
            problems = repoint.orientation_problems(Path(tmp))
        self.assertEqual(len(problems), 2)
        self.assertTrue(any("big/SKILL.md" in p for p in problems))
        self.assertTrue(any("CLAUDE.md" in p for p in problems))


class Status(unittest.TestCase):
    def test_a_quantity_in_the_capability_table_fails_and_elsewhere_does_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            write(Path(tmp), "docs/status/STATUS.md", FILES["docs/status/STATUS.md"])
            self.assertEqual(repoint.status_problems(Path(tmp)), [])
            write(Path(tmp), "docs/status/STATUS.md",
                  FILES["docs/status/STATUS.md"].replace("| x | y |", "| x | 5.60× |"))
            problems = repoint.status_problems(Path(tmp))
        self.assertEqual(len(problems), 1)


class Comments(unittest.TestCase):
    def check(self, line: str) -> list[str]:
        with tempfile.TemporaryDirectory() as tmp:
            write(Path(tmp), "pgdump_query/src/a.rs", line + "\n")
            return repoint.comment_problems(Path(tmp))

    def test_provenance_in_a_comment_is_named(self):
        for bad in ("// landed in P3.2", "/// see M12", "//! since 2026-09-01"):
            self.assertEqual(len(self.check(bad)), 1, bad)

    def test_handles_dates_as_data_and_code_are_not(self):
        for ok in ("// D12, KD3, I34, RT7", "/// epoch 2000-01-01", 'let s = "P3.2";'):
            self.assertEqual(self.check(ok), [], ok)


class Stamp(unittest.TestCase):
    def test_one_stamp_reads_and_zero_or_two_do_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            write(Path(tmp), "docs/status/STATUS.md", "<!-- repointed: abc1234 -->\n")
            self.assertEqual(repoint.read_stamp(Path(tmp)), "abc1234")
            write(Path(tmp), "docs/status/STATUS.md", "none\n")
            self.assertIsNone(repoint.read_stamp(Path(tmp)))
            write(Path(tmp), "docs/status/STATUS.md", "<!-- repointed: abc1234 -->\n<!-- repointed: abc1235 -->\n")
            self.assertIsNone(repoint.read_stamp(Path(tmp)))


class Meter(unittest.TestCase):
    def test_the_fixture_is_green_and_at_zero(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp)
            out = io.StringIO()
            self.assertEqual(repoint.check(repo, out=out), 0)
            self.assertIn("net +0", out.getvalue())
            self.assertIn("repoint: green", out.getvalue())

    def test_live_docs_count_and_centering_and_history_do_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp)
            write(repo, "docs/design/roadmap.md", "# Roadmap\n" + "x\n" * 10)
            write(repo, "docs/design/roadmap-P9-thing.md", "x\n" * 100)
            write(repo, "docs/status/history/2026-09-14.md", "x\n" * 50)
            write(repo, "docs/design/new.md", "x\n" * 5)  # untracked
            g = repoint.growth_since(repo, repoint.read_stamp(repo))
        self.assertEqual(g.docs, 15)

    def test_comment_lines_count_in_both_directions_and_code_does_not(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp)
            write(repo, "pgdump_query/src/lib.rs", "/// a\n/// b\n// c\nfn f() {}\nfn g() {}\n")
            write(repo, "pgdump_query/src/new.rs", "//! new\nfn h() {}\n")  # untracked
            g = repoint.growth_since(repo, repoint.read_stamp(repo))
        self.assertEqual(g.comments, 3)  # +3 -1 committed, +1 untracked

    def test_past_the_budget_is_red(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp)
            write(repo, "docs/design/roadmap.md", "x\n" * 20)
            self.assertEqual(repoint.check(repo, budget=10, out=io.StringIO()), 1)
            self.assertEqual(repoint.check(repo, budget=100, out=io.StringIO()), 0)

    def test_a_stamp_that_is_not_an_ancestor_is_red(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = make_repo(tmp)
            problems = repoint.meter_problems(repo, "0000000", 10, io.StringIO())
        self.assertEqual(len(problems), 1)
        self.assertIn("ancestor", problems[0])


class ThisRepo(unittest.TestCase):
    def test_the_stamp_is_readable_and_the_scope_is_as_documented(self):
        stamp = repoint.read_stamp(repoint.REPO)
        self.assertIsNotNone(stamp)
        self.assertTrue(repoint.is_live_doc("docs/design/decisions.md"))
        self.assertTrue(repoint.is_live_doc(".claude/skills/go/SKILL.md"))
        self.assertFalse(repoint.is_live_doc("docs/status/history/2026-09-14.md"))
        self.assertFalse(repoint.is_live_doc("docs/design/roadmap-P10-row-group-statistics-inbox.md"))
        self.assertFalse(repoint.is_live_doc("vendor/xz-seek/README.md"))
        self.assertTrue(repoint.is_source("pgdump_query/tests/layering.rs"))
        self.assertFalse(repoint.is_source("vendor/xz-seek/src/lib.rs"))


if __name__ == "__main__":
    unittest.main()
