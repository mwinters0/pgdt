#!/usr/bin/env python3
"""Unit tests for major_differences.py, run with `uv run python -m unittest test_major_differences`.

As for `test_pg_refuses`: each test breaks exactly one thing and asserts the
failure names it, and `ThisRepo` holds the tree to the register's rules.
"""

from __future__ import annotations

import io
import subprocess
import tempfile
import unittest
from pathlib import Path

import major_differences

V = "VD"

REGISTER = f"""# Differences between PostgreSQL majors

Prose naming `{V}<n>`, which is no citation.
<!-- difference-watermark: {V}3 -->

---

## {V}1 — one

**Claim.** It differs.

**Proof.** Source.

**Observed.** Not observed.

**Re-verify.**

```sh
true
```

---

## {V}3 — another

**Claim.** It differs too, unlike {V}1.

**Proof.** Source.

**Observed.** Seen.

**Re-verify.** Look.
"""


class Reading(unittest.TestCase):
    def test_entries_and_their_fields_are_read(self):
        found, watermark, problems = major_differences.entries(REGISTER)
        self.assertEqual(problems, [])
        self.assertEqual(watermark, 3)
        self.assertEqual(set(found), {f"{V}1", f"{V}3"})
        self.assertEqual(found[f"{V}1"], set(major_differences.FIELDS))


class Checking(unittest.TestCase):
    def repo(self, files: dict[str, str]) -> Path:
        root = Path(tempfile.mkdtemp())
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        return root

    def run_check(self, files: dict[str, str], register: str = REGISTER) -> tuple[int, str]:
        base = {major_differences.REGISTER: register}
        out = io.StringIO()
        code = major_differences.check(self.repo(base | files), out=out)
        return code, out.getvalue()

    def test_a_well_formed_register_passes(self):
        code, out = self.run_check({})
        self.assertEqual(code, 0, out)
        self.assertIn("2 entries", out)

    def test_a_citation_from_code_is_caught(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// see {V}1\nfn f() {{}}\n"})
        self.assertEqual(code, 1)
        self.assertIn(f"crate/src/lib.rs:1: cites {V}1", out)

    def test_a_citation_from_a_standing_doc_is_caught(self):
        code, out = self.run_check({"docs/design/decisions.md": f"Evidence: {V}3.\n"})
        self.assertEqual(code, 1)
        self.assertIn("decisions.md:1", out)

    def test_a_dated_entry_may_name_one(self):
        code, out = self.run_check({"docs/status/history/2026-10-04.md": f"Filed {V}1.\n"})
        self.assertEqual(code, 0, out)

    def test_a_missing_field_is_caught(self):
        code, out = self.run_check({}, REGISTER.replace("**Observed.** Seen.\n", ""))
        self.assertEqual(code, 1)
        self.assertIn(f"{V}3 lacks Observed", out)

    def test_an_entry_past_the_watermark_is_caught(self):
        code, out = self.run_check({}, REGISTER.replace(f"watermark: {V}3", f"watermark: {V}2"))
        self.assertEqual(code, 1)
        self.assertIn("past the watermark", out)

    def test_a_missing_watermark_is_caught(self):
        code, out = self.run_check({}, REGISTER.replace(f"<!-- difference-watermark: {V}3 -->\n", ""))
        self.assertEqual(code, 1)
        self.assertIn("no `difference-watermark`", out)

    def test_a_number_headed_twice_is_caught(self):
        code, out = self.run_check({}, REGISTER.replace(f"## {V}3 — another", f"## {V}1 — another"))
        self.assertEqual(code, 1)
        self.assertIn("headed twice", out)

    def test_a_malformed_heading_is_caught(self):
        code, out = self.run_check({}, REGISTER.replace(f"## {V}3 — another", f"### {V}3: another"))
        self.assertEqual(code, 1)
        self.assertIn("is not `## VD<n> — <title>`", out)


class ThisRepo(unittest.TestCase):
    """The tree held to the register's rules."""

    def test_the_register_holds(self):
        out = io.StringIO()
        self.assertEqual(major_differences.check(out=out), 0, out.getvalue())

    def test_the_population_is_not_empty(self):
        text = (major_differences.REPO / major_differences.REGISTER).read_text()
        found, _, _ = major_differences.entries(text)
        self.assertTrue(found)


if __name__ == "__main__":
    unittest.main()
