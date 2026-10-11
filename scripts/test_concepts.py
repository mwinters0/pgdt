#!/usr/bin/env python3
"""Unit tests for concepts.py, run with `uv run python -m unittest test_concepts`.

As for `test_major_differences`: each test breaks exactly one thing and asserts
the failure names it, and `ThisRepo` holds the tree to the map.
"""

from __future__ import annotations

import io
import tempfile
import unittest
from pathlib import Path

import concepts

MAP = """# The concept map

Prose naming `a/b.rs` and `other/` claims nothing.

## Sources

- **lib-a** — `source/`: the whole concept,
  wrapped onto `extra.rs`.

**Not in one place.** Something.

## Statistics

- **lib-a** — `stats.rs`: gathering.
- **bin-b** — `report.rs`: printing.
"""

TREE = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\n',
    "crates/lib-a/Cargo.toml": "",
    "crates/lib-a/src/lib.rs": "mod source;\nmod extra;\nmod stats;\n",
    "crates/lib-a/src/source.rs": "mod local;\n#[cfg(test)]\nmod tests;\n",
    "crates/lib-a/src/source/local.rs": "fn f() {}\n",
    "crates/lib-a/src/source/tests.rs": "fn t() {}\n",
    "crates/lib-a/src/extra.rs": "fn e() {}\n",
    "crates/lib-a/src/stats.rs": "fn s() {}\n",
    "crates/lib-a/tests/it.rs": "fn i() {}\n",
    "crates/bin-b/Cargo.toml": "",
    "crates/bin-b/src/main.rs": "mod report;\nfn main() {}\n",
    "crates/bin-b/src/report.rs": "fn r() {}\n",
}


class Reading(unittest.TestCase):
    def test_claims_are_read_with_their_wrapped_lines(self):
        found, scattered, problems = concepts.claims(MAP)
        self.assertEqual(problems, [])
        self.assertEqual(scattered, ["Sources"])
        self.assertEqual(
            [(c, p) for c, p, _ in found["Sources"]],
            [("lib-a", "source/"), ("lib-a", "extra.rs")],
        )
        self.assertEqual(
            [(c, p) for c, p, _ in found["Statistics"]],
            [("lib-a", "stats.rs"), ("bin-b", "report.rs")],
        )

    def test_a_directory_claims_its_files_and_the_file_beside_it(self):
        files = {"source.rs", "source/local.rs", "sources.rs", "stats.rs"}
        self.assertEqual(concepts.resolve("source/", files), {"source.rs", "source/local.rs"})


class Checking(unittest.TestCase):
    def run_check(self, the_map: str = MAP, tree: dict[str, str] | None = None) -> tuple[int, str]:
        root = Path(tempfile.mkdtemp())
        for rel, text in ((tree or TREE) | {concepts.MAP: the_map}).items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        out = io.StringIO()
        return concepts.check(root, out=out), out.getvalue()

    def test_a_map_matching_the_tree_passes(self):
        code, out = self.run_check()
        self.assertEqual(code, 0, out)
        self.assertIn("2 concepts claim 5 files once each; 1 span crates, 1 not in one place", out)

    def test_an_unclaimed_file_is_caught(self):
        code, out = self.run_check(tree=TREE | {"crates/lib-a/src/new.rs": "fn n() {}\n"})
        self.assertEqual(code, 1)
        self.assertIn("crates/lib-a/src/new.rs is claimed by no concept", out)

    def test_a_crate_root_need_not_be_claimed_but_may_be_once(self):
        code, out = self.run_check(MAP.replace("`report.rs`: printing", "`report.rs`, `main.rs`"))
        self.assertEqual(code, 0, out)
        code, out = self.run_check(MAP.replace("`stats.rs`: gathering", "`stats.rs`, `main.rs`").replace(
            "`report.rs`: printing", "`report.rs`, `main.rs`"))
        self.assertEqual(code, 1)
        self.assertIn("lib-a's `main.rs` names no production file", out)

    def test_a_file_claimed_twice_is_caught(self):
        code, out = self.run_check(MAP.replace("`stats.rs`: gathering", "`stats.rs`, `extra.rs`"))
        self.assertEqual(code, 1)
        self.assertIn("crates/lib-a/src/extra.rs is claimed by 'Sources' and 'Statistics'", out)

    def test_a_directory_overlapping_a_file_claim_is_caught(self):
        code, out = self.run_check(MAP.replace("`stats.rs`: gathering", "`stats.rs`, `source/local.rs`"))
        self.assertEqual(code, 1)
        self.assertIn("source/local.rs is claimed by 'Sources' and 'Statistics'", out)

    def test_a_missing_path_is_caught(self):
        code, out = self.run_check(MAP.replace("`stats.rs`: gathering", "`stats.rs`, `gone.rs`"))
        self.assertEqual(code, 1)
        self.assertIn("lib-a's `gone.rs` names no production file", out)

    def test_a_test_file_is_not_claimable(self):
        code, out = self.run_check(MAP.replace("`stats.rs`: gathering", "`stats.rs`, `source/tests.rs`"))
        self.assertEqual(code, 1)
        self.assertIn("`source/tests.rs` names no production file", out)

    def test_an_unknown_crate_is_caught(self):
        code, out = self.run_check(MAP.replace("**bin-b**", "**bin-c**"))
        self.assertEqual(code, 1)
        self.assertIn("bin-c is not a workspace member", out)

    def test_a_concept_claiming_nothing_is_caught(self):
        code, out = self.run_check(MAP + "\n## Empty\n\nProse only.\n")
        self.assertEqual(code, 1)
        self.assertIn("concept 'Empty' claims nothing", out)

    def test_a_concept_headed_twice_is_caught(self):
        code, out = self.run_check(MAP.replace("## Statistics", "## Sources"))
        self.assertEqual(code, 1)
        self.assertIn("concept 'Sources' is headed twice", out)

    def test_a_missing_map_is_caught(self):
        root = Path(tempfile.mkdtemp())
        out = io.StringIO()
        self.assertEqual(concepts.check(root, out=out), 1)
        self.assertIn("is missing", out.getvalue())


class ThisRepo(unittest.TestCase):
    """The tree held to the map."""

    def test_the_map_holds(self):
        out = io.StringIO()
        self.assertEqual(concepts.check(out=out), 0, out.getvalue())


if __name__ == "__main__":
    unittest.main()
