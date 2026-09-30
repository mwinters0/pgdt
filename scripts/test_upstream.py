#!/usr/bin/env python3
"""Unit tests for upstream.py, run with `uv run python -m unittest test_upstream`.

As for `test_deficiencies`: each test breaks exactly one thing and asserts the
failure names it, and `ThisRepo` holds the real register to its rules and
asserts the populations it resolves are not empty.
"""

from __future__ import annotations

import io
import subprocess
import tempfile
import unittest
from pathlib import Path

import upstream

MARK = "upstream" + ": "

ENTRY = """## UF1 — a thing upstream gets wrong

- **The issue.** It is wrong. Ours: `KD1`.
- **Upstream.** [issue](https://example.org/1).
- **Fixed when.** A release carries it.
- **Workaround.** We avoid it.
- **Watch.** A test.
- **When it lands.** Undo the workaround.
"""

REGISTER = "# Upstream fixes\n\nProse.\n<!-- upstream-watermark: UF1 -->\n\n" + ENTRY

KD_REGISTER = """# Known deficiencies

Prose above the entries.
<!-- deficiency-watermark: KD1 -->

- **KD1** — a thing. **(c) unowned**; promoted by a release. Detail:
  `crate/src/lib.rs`.
"""


class Parsing(unittest.TestCase):
    def check(self, register: str = REGISTER, live=frozenset({"KD1"})) -> list[str]:
        entries, watermark, problems = upstream.parse(register)
        return problems + upstream.check_entries(entries, watermark, set(live))

    def test_a_well_formed_entry_passes(self):
        self.assertEqual(self.check(), [])

    def test_a_missing_field_is_caught(self):
        broken = REGISTER.replace("- **Watch.** A test.\n", "")
        self.assertTrue(any("fields are" in p for p in self.check(broken)))

    def test_fields_out_of_order_are_caught(self):
        broken = REGISTER.replace("- **Watch.** A test.\n", "").replace(
            "- **The issue.**", "- **Watch.** A test.\n- **The issue.**"
        )
        self.assertTrue(any("fields are" in p for p in self.check(broken)))

    def test_an_upstream_field_with_no_link_and_no_search_is_caught(self):
        broken = REGISTER.replace("[issue](https://example.org/1).", "Somewhere.")
        self.assertTrue(any("none was found" in p for p in self.check(broken)))

    def test_saying_none_was_found_passes(self):
        searched = REGISTER.replace(
            "[issue](https://example.org/1).", "No issue or discussion found (searched X)."
        )
        self.assertEqual(self.check(searched), [])

    def test_a_struck_deficiency_is_caught(self):
        self.assertTrue(any("KD1" in p for p in self.check(live=frozenset())))

    def test_an_entry_past_the_watermark_is_caught(self):
        second = ENTRY.replace("UF1", "UF2")
        self.assertTrue(any("past the watermark" in p for p in self.check(REGISTER + "\n" + second)))

    def test_a_malformed_heading_is_caught(self):
        broken = REGISTER.replace("## UF1 — a thing", "## UF1: a thing")
        self.assertTrue(any("heading" in p for p in self.check(broken)))

    def test_a_missing_watermark_is_caught(self):
        broken = REGISTER.replace("<!-- upstream-watermark: UF1 -->", "")
        self.assertTrue(any("watermark" in p for p in self.check(broken)))


class Reconciling(unittest.TestCase):
    def repo(self, files: dict[str, str]) -> Path:
        root = Path(tempfile.mkdtemp())
        for rel, text in files.items():
            (root / rel).parent.mkdir(parents=True, exist_ok=True)
            (root / rel).write_text(text)
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        return root

    def run_check(self, files: dict[str, str]) -> tuple[int, str]:
        base = {
            "docs/status/upstream.md": REGISTER,
            "docs/status/deficiencies.md": KD_REGISTER,
        }
        out = io.StringIO()
        code = upstream.check(self.repo(base | files), out=out)
        return code, out.getvalue()

    def test_a_marked_entry_reconciles(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// {MARK}UF1\n"})
        self.assertEqual(code, 0, out)

    def test_an_unmarked_entry_is_caught(self):
        code, out = self.run_check({"crate/src/lib.rs": "fn f() {}\n"})
        self.assertEqual(code, 1)
        self.assertIn("has no", out)

    def test_a_marker_naming_no_entry_is_caught(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// {MARK}UF1\n// {MARK}UF9\n"})
        self.assertEqual(code, 1)
        self.assertIn("UF9", out)

    def test_a_marker_under_docs_is_caught(self):
        code, out = self.run_check(
            {"crate/src/lib.rs": f"// {MARK}UF1\n", "docs/design/x.md": f"{MARK}UF1\n"}
        )
        self.assertEqual(code, 1)
        self.assertIn("under docs/", out)

    def test_an_untracked_file_is_scanned(self):
        """A marker in a file not yet added is found: a change is checked before it is committed."""
        code, out = self.run_check({"crate/tests/new.rs": f"// {MARK}UF1\n"})
        self.assertEqual(code, 0, out)


class ThisRepo(unittest.TestCase):
    """The register in the tree, held to its own rules."""

    def test_the_real_register_reconciles(self):
        out = io.StringIO()
        self.assertEqual(upstream.check(out=out), 0, out.getvalue())

    def test_the_populations_are_not_empty(self):
        entries, _, _ = upstream.parse(upstream.REGISTER.read_text())
        self.assertTrue(entries)
        found = upstream.markers(upstream.REPO, upstream.tracked_files(upstream.REPO))
        self.assertTrue({e.id for e in entries} <= {m.id for m in found})


if __name__ == "__main__":
    unittest.main()
