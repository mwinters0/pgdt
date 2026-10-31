#!/usr/bin/env python3
"""Unit tests for timescaledb_invariants.py, run with
`uv run python -m unittest test_timescaledb_invariants`.

As for `test_pg_refuses`: each test breaks exactly one thing and asserts the
failure names it, and `ThisRepo` holds the tree's markers to the invariants
and asserts the population is not empty.
"""

from __future__ import annotations

import io
import subprocess
import tempfile
import unittest
from pathlib import Path

import timescaledb_invariants as ts

MARK = "timescaledb" + ": "

INVARIANTS = """# TimescaleDB invariants we rely on

Prose naming `TS1` and `some_fn`, outside every entry.

---

## TS1 — a thing the catalog states

**Claim.** It states it.

**Relied on by:** [`spec.md`](spec.md), "A section" — a `chunk.dropped` row:
`read_thing` and `Catalog::of` (`pgdt-model`'s `catalog.rs`).

**Re-verify:** `grep -n not_a_site file`.

---

## TS2 — another

**Relied on by:** `scripts/generate.py`, `write_answers`.
"""

RUST = f"""/// Reads a thing.
// {MARK}TS1 — attached.
pub fn read_thing() {{}}

impl Catalog {{
    fn of(&self) -> u8 {{
        // {MARK}TS1 — inside.
        1
    }}
}}
"""

PYTHON = f'''def write_answers(out):
    """Writes them."""
    # {MARK}TS2 -- inside.
    return out
'''


class Parsing(unittest.TestCase):
    def test_a_site_is_a_bare_identifier_or_a_path_ending_in_one(self):
        entries = ts.sites(INVARIANTS)
        self.assertEqual(entries["TS1"], {"read_thing", "of"})
        self.assertEqual(entries["TS2"], {"write_answers"})

    def test_a_python_marker_names_its_enclosing_def(self):
        lines = PYTHON.splitlines()
        self.assertEqual(ts.pg_refuses.site_of(lines, 2, python=True), "write_answers")


class Reconciling(unittest.TestCase):
    def repo(self, files: dict[str, str]) -> Path:
        root = Path(tempfile.mkdtemp())
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        return root

    def run_check(self, files: dict[str, str]) -> tuple[int, str]:
        base = {"docs/design/timescaledb-invariants.md": INVARIANTS}
        out = io.StringIO()
        code = ts.check(self.repo(base | files), out=out)
        return code, out.getvalue()

    def test_markers_at_listed_sites_resolve(self):
        code, out = self.run_check({"crate/src/catalog.rs": RUST, "scripts/generate.py": PYTHON})
        self.assertEqual(code, 0, out)
        self.assertIn("3 marker(s)", out)

    def test_a_site_the_invariant_does_not_list_is_caught(self):
        code, out = self.run_check({
            "crate/src/catalog.rs": RUST + f"fn unlisted() {{\n    // {MARK}TS1\n}}\n",
            "scripts/generate.py": PYTHON,
        })
        self.assertEqual(code, 1)
        self.assertIn("`unlisted`", out)

    def test_a_listed_site_no_marker_marks_is_caught(self):
        code, out = self.run_check({"crate/src/catalog.rs": RUST})
        self.assertEqual(code, 1)
        self.assertIn("names `write_answers`", out)

    def test_a_marker_naming_no_invariant_is_caught(self):
        code, out = self.run_check({
            "crate/src/catalog.rs": RUST + f"// {MARK}TS9\nfn read_thing() {{}}\n",
            "scripts/generate.py": PYTHON,
        })
        self.assertEqual(code, 1)
        self.assertIn("TS9", out)

    def test_a_marker_with_no_function_is_caught(self):
        code, out = self.run_check({
            "crate/src/catalog.rs": RUST + f"// {MARK}TS1\nconst X: u8 = 1;\n",
            "scripts/generate.py": PYTHON,
        })
        self.assertEqual(code, 1)
        self.assertIn("no function", out)

    def test_a_marker_under_docs_is_not_a_site(self):
        code, out = self.run_check({
            "crate/src/catalog.rs": RUST,
            "scripts/generate.py": PYTHON,
            "docs/design/x.md": f"`{MARK}TS1`\n",
        })
        self.assertEqual(code, 0, out)


class ThisRepo(unittest.TestCase):
    """The tree's markers, held to the invariants they name."""

    def test_every_marker_resolves_both_ways(self):
        out = io.StringIO()
        self.assertEqual(ts.check(out=out), 0, out.getvalue())

    def test_the_population_is_not_empty(self):
        found = ts.markers(ts.REPO, ts.upstream.tracked_files(ts.REPO))
        self.assertTrue(found)


if __name__ == "__main__":
    unittest.main()
