#!/usr/bin/env python3
"""Unit tests for pg_refuses.py, run with `uv run python -m unittest test_pg_refuses`.

As for `test_upstream`: each test breaks exactly one thing and asserts the
failure names it, and `ThisRepo` holds the tree's markers to the invariants and
asserts the population is not empty.
"""

from __future__ import annotations

import io
import subprocess
import tempfile
import unittest
from pathlib import Path

import pg_refuses

MARK = "pg-refuses" + ": "

INVARIANTS = """# PostgreSQL invariants we rely on

Prose.

---

## I1 — a thing the server refuses

**Claim.** It refuses it.

**Relied on by:** `decisions.md` ("D1") — `nested.rs`'s
`parse_thing`, and `crate::check::bounded`.

**Re-verify.** Look, and `not_relied`.

---

## I2 — another

**Relied on by.** `other`.
"""

SOURCE = f"""/// Parses a thing.
// {MARK}I1
#[inline]
pub fn parse_thing(s: &str) -> Option<u8> {{
    s.parse().ok()
}}

impl Check {{
    pub(crate) fn bounded(&self, n: u8) -> Option<u8> {{
        let x = n.checked_add(1)?;
        // {MARK}I1
        if x > 9 {{
            return None;
        }}
        Some(x)
    }}
}}
"""


class Parsing(unittest.TestCase):
    def test_the_field_wraps_and_ends_at_the_next_field(self):
        entries = pg_refuses.relied_on_by(INVARIANTS)
        self.assertIn("parse_thing", entries["I1"])
        self.assertIn("bounded", entries["I1"])
        self.assertNotIn("not_relied", entries["I1"])
        self.assertEqual(entries["I2"], {"other"})

    def test_an_attached_marker_names_the_function_below_it(self):
        lines = SOURCE.splitlines()
        self.assertEqual(pg_refuses.site_of(lines, 1), "parse_thing")

    def test_a_marker_in_a_body_names_the_enclosing_function(self):
        lines = SOURCE.splitlines()
        self.assertEqual(pg_refuses.site_of(lines, 10), "bounded")

    def test_a_marker_outside_every_function_names_none(self):
        self.assertIsNone(pg_refuses.site_of([f"// {MARK}I1", "const X: u8 = 1;"], 0))


class Reconciling(unittest.TestCase):
    def repo(self, files: dict[str, str]) -> Path:
        root = Path(tempfile.mkdtemp())
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        return root

    def run_check(self, files: dict[str, str]) -> tuple[int, str]:
        base = {"docs/design/postgres-invariants.md": INVARIANTS}
        out = io.StringIO()
        code = pg_refuses.check(self.repo(base | files), out=out)
        return code, out.getvalue()

    def test_markers_at_listed_sites_resolve(self):
        code, out = self.run_check({"crate/src/nested.rs": SOURCE})
        self.assertEqual(code, 0, out)
        self.assertIn("2 marker(s)", out)

    def test_a_site_the_invariant_does_not_list_is_caught(self):
        code, out = self.run_check(
            {"crate/src/lib.rs": f"fn unlisted() {{\n    // {MARK}I1\n}}\n"}
        )
        self.assertEqual(code, 1)
        self.assertIn("`unlisted`", out)

    def test_a_site_listed_under_another_field_is_caught(self):
        code, out = self.run_check(
            {"crate/src/lib.rs": f"// {MARK}I1\nfn not_relied() {{}}\n"}
        )
        self.assertEqual(code, 1)
        self.assertIn("`not_relied`", out)

    def test_a_marker_naming_no_invariant_is_caught(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// {MARK}I9\nfn parse_thing() {{}}\n"})
        self.assertEqual(code, 1)
        self.assertIn("I9", out)

    def test_a_marker_with_no_function_is_caught(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// {MARK}I1\nconst X: u8 = 1;\n"})
        self.assertEqual(code, 1)
        self.assertIn("no function", out)

    def test_a_marker_outside_rust_is_caught(self):
        code, out = self.run_check({"scripts/x.py": f"# {MARK}I1\n"})
        self.assertEqual(code, 1)
        self.assertIn("outside Rust", out)

    def test_a_marker_under_docs_is_not_a_site(self):
        code, out = self.run_check({"docs/design/x.md": f"`{MARK}I1`\n"})
        self.assertEqual(code, 0, out)


class ThisRepo(unittest.TestCase):
    """The tree's markers, held to the invariants they name."""

    def test_every_marker_resolves(self):
        out = io.StringIO()
        self.assertEqual(pg_refuses.check(out=out), 0, out.getvalue())

    def test_the_population_is_not_empty(self):
        found = pg_refuses.markers(pg_refuses.REPO, pg_refuses.upstream.tracked_files(pg_refuses.REPO))
        self.assertTrue(found)


if __name__ == "__main__":
    unittest.main()
