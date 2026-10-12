#!/usr/bin/env python3
"""Unit tests for seam_register.py, run with `uv run python -m unittest test_seam_register`.

As for `test_upstream`: each test breaks exactly one thing and asserts the
failure names it, and `ThisRepo` holds the real register to its rules. The
register may be empty, so what `ThisRepo` asserts is not empty is the skill's
list of antipatterns, without which every entry would be refused.
"""

from __future__ import annotations

import io
import subprocess
import tempfile
import unittest
from pathlib import Path

import seam_register

MARK = "seam" + ": "

SKILL = """# Seams

### Antipatterns

Prose.

- **One concept, several files** — the one that matters most.
- **The half-split.** Part of a concern moved out.
- **Sibling cycles.** Two modules reading each other.

**What moves with the code.** Not an antipattern.

## 4. A crate is a seam too

- **Should a crate split?** Not an antipattern either.
"""

ENTRY = """- **S1** — *The half-split.* A gatherer's heap read field by field in
  its parent.
"""

REGISTER = "# Seam findings\n\nProse.\n<!-- seam-watermark: S1 -->\n\n" + ENTRY


class Antipatterns(unittest.TestCase):
    def test_only_the_section_s_bullets_are_names(self):
        self.assertEqual(
            seam_register.antipatterns(SKILL),
            ["One concept, several files", "The half-split", "Sibling cycles"],
        )


class Parsing(unittest.TestCase):
    def check(self, register: str = REGISTER, skill: str = SKILL) -> list[str]:
        entries, watermark, problems = seam_register.parse(register)
        return problems + seam_register.check_entries(
            entries, watermark, seam_register.antipatterns(skill)
        )

    def test_a_well_formed_entry_passes(self):
        self.assertEqual(self.check(), [])

    def test_an_empty_register_passes(self):
        self.assertEqual(self.check("# Seam findings\n<!-- seam-watermark: S0 -->\n"), [])

    def test_a_malformed_entry_is_caught(self):
        broken = REGISTER.replace("*The half-split.*", "The half-split:")
        self.assertTrue(any("is not" in p for p in self.check(broken)))

    def test_an_antipattern_the_skill_does_not_name_is_caught(self):
        broken = REGISTER.replace("*The half-split.*", "*The quarter-split.*")
        self.assertTrue(any("does not" in p for p in self.check(broken)))

    def test_an_antipattern_struck_from_the_skill_is_caught(self):
        skill = SKILL.replace("- **The half-split.** Part of a concern moved out.\n", "")
        self.assertTrue(any("does not" in p for p in self.check(skill=skill)))

    def test_an_antipattern_held_elsewhere_is_refused(self):
        for name, home in (("One concept, several files", "concepts.md"), ("Sibling cycles", "module_graph.py")):
            broken = REGISTER.replace("*The half-split.*", f"*{name}.*")
            self.assertTrue(any(home in p for p in self.check(broken)), name)

    def test_an_entry_past_its_line_cap_is_caught(self):
        long = REGISTER.replace("  its parent.\n", "  its parent,\n  and\n  then\n  more.\n")
        self.assertTrue(any("past" in p for p in self.check(long)))

    def test_an_entry_past_the_watermark_is_caught(self):
        second = ENTRY.replace("S1", "S2")
        self.assertTrue(any("past the watermark" in p for p in self.check(REGISTER + second)))

    def test_an_entry_twice_is_caught(self):
        self.assertTrue(any("twice" in p for p in self.check(REGISTER + ENTRY)))

    def test_a_missing_watermark_is_caught(self):
        broken = REGISTER.replace("<!-- seam-watermark: S1 -->", "")
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
            "docs/status/seam-findings.md": REGISTER,
            ".claude/skills/seams/SKILL.md": SKILL,
        }
        out = io.StringIO()
        code = seam_register.check(self.repo(base | files), out=out)
        return code, out.getvalue()

    def test_a_marked_entry_reconciles(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// {MARK}S1\n"})
        self.assertEqual(code, 0, out)

    def test_an_unmarked_entry_is_caught(self):
        code, out = self.run_check({"crate/src/lib.rs": "fn f() {}\n"})
        self.assertEqual(code, 1)
        self.assertIn("has no", out)

    def test_a_marker_naming_no_entry_is_caught(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// {MARK}S1\n// {MARK}S9\n"})
        self.assertEqual(code, 1)
        self.assertIn("S9", out)

    def test_a_marker_outside_rust_marks_nothing(self):
        code, out = self.run_check({"crate/src/lib.rs": f"// {MARK}S1\n", "notes.md": f"{MARK}S9\n"})
        self.assertEqual(code, 0, out)

    def test_an_untracked_file_is_scanned(self):
        """A marker in a file not yet added is found: a change is checked before it is committed."""
        code, out = self.run_check({"crate/tests/new.rs": f"// {MARK}S1\n"})
        self.assertEqual(code, 0, out)


class ThisRepo(unittest.TestCase):
    """The register in the tree, held to its own rules."""

    def test_the_real_register_reconciles(self):
        out = io.StringIO()
        self.assertEqual(seam_register.check(out=out), 0, out.getvalue())

    def test_the_skill_names_its_antipatterns(self):
        names = seam_register.antipatterns(seam_register.SKILL.read_text())
        self.assertIn("The half-split", names)
        self.assertTrue(set(seam_register.ELSEWHERE) <= set(names))


if __name__ == "__main__":
    unittest.main()
