#!/usr/bin/env python3
"""Unit tests for gaps.py, run with `uv run python -m unittest test_gaps`.

Stdlib `unittest`, no dependency added, same as test_measure.py. What is tested
is the half where a silent failure would be worst: not that a well-formed
register passes -- the repo's own does that on every run -- but that each way of
breaking it is *caught*. A reconciliation that quietly passes on a broken
register is worse than none, because the register is then trusted and wrong.

So every test below builds a small repo in a temp directory, breaks exactly one
thing, and asserts the failure names it. The structural test at the bottom is
the other half: it runs the real check over the real tree.
"""

from __future__ import annotations

import io
import tempfile
import unittest
from pathlib import Path

import gaps

INDEX_HEAD = """# Status

## Known gaps

The gap register. Prose above the entries, which the parser must skip.

"""

ENTRY_G1 = """- **G1** — a thing that costs something. **(c) unowned**; promoted by a
  dump in hand. Detail:
  [`../design/architecture.md`](../design/architecture.md), "A mechanism".

"""

ENTRY_G2 = """- **G2** — another thing. **(b) owned by P7**, whose plans rework it.
  Detail: [`../design/architecture.md`](../design/architecture.md), "Another".

"""

TAIL = """## Decisions worth another look

*Nothing is open.*
"""


def build(tmp: Path, *, status: str, arch: str = "", code: str = "", extra=None) -> Path:
    """A repo shaped like this one: an index, a doc tree, a crate source dir."""
    (tmp / "docs" / "status").mkdir(parents=True)
    (tmp / "docs" / "design").mkdir(parents=True)
    (tmp / "pgdump_query" / "src").mkdir(parents=True)
    (tmp / "docs" / "status" / "STATUS.md").write_text(status)
    (tmp / "docs" / "design" / "architecture.md").write_text(arch)
    (tmp / "pgdump_query" / "src" / "lib.rs").write_text(code)
    for rel, text in (extra or {}).items():
        path = tmp / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return tmp


def run(tmp: Path) -> tuple[int, str]:
    out = io.StringIO()
    code = gaps.check(tmp, out=out)
    return code, out.getvalue()


class Parsing(unittest.TestCase):
    def test_an_entry_yields_its_stance_and_detail(self):
        entries, problems = gaps.parse_index(INDEX_HEAD + ENTRY_G1 + ENTRY_G2 + TAIL)
        self.assertEqual(problems, [])
        self.assertEqual([e.id for e in entries], ["G1", "G2"])
        self.assertEqual(entries[0].stance, "c")
        self.assertEqual(entries[0].destination, "")
        self.assertEqual(entries[1].stance, "b")
        self.assertEqual(entries[1].destination, "P7")
        self.assertEqual(entries[0].detail, "../design/architecture.md")

    def test_prose_above_the_entries_is_not_an_entry(self):
        entries, problems = gaps.parse_index(INDEX_HEAD + ENTRY_G1 + TAIL)
        self.assertEqual(len(entries), 1)
        self.assertEqual(problems, [])

    def test_the_section_ends_at_the_next_heading(self):
        text = INDEX_HEAD + ENTRY_G1 + "## Elsewhere\n\n" + ENTRY_G2
        entries, _ = gaps.parse_index(text)
        self.assertEqual([e.id for e in entries], ["G1"])

    def test_a_deleted_section_is_a_problem_not_an_empty_register(self):
        entries, problems = gaps.parse_index("# Status\n\n## Not started\n\nnothing\n")
        self.assertEqual(entries, [])
        self.assertIn("the register is gone", problems[0])

    def test_a_stanceless_entry_is_named(self):
        text = INDEX_HEAD + "- **G1** — a thing. Detail: [`a`](../design/architecture.md).\n" + TAIL
        _, problems = gaps.parse_index(text)
        self.assertTrue(any("G1 declares no stance" in p for p in problems))

    def test_stance_c_must_say_unowned_in_that_word(self):
        text = INDEX_HEAD + ENTRY_G1.replace("unowned", "nobody is on it") + TAIL
        _, problems = gaps.parse_index(text)
        self.assertTrue(any('does not say "unowned"' in p for p in problems))

    def test_stance_b_must_name_a_destination(self):
        text = INDEX_HEAD + ENTRY_G2.replace("owned by P7", "owned") + TAIL
        _, problems = gaps.parse_index(text)
        self.assertTrue(any("names no destination" in p for p in problems))

    def test_stance_a_must_say_it_is_a_tradeoff(self):
        text = INDEX_HEAD + ENTRY_G1.replace("(c) unowned", "(a) fine as it is") + TAIL
        _, problems = gaps.parse_index(text)
        self.assertTrue(any("deliberate tradeoff" in p for p in problems))

    def test_an_entry_with_no_detail_pointer_is_named(self):
        text = INDEX_HEAD + "- **G1** — a thing. **(c) unowned**; promoted by nothing.\n" + TAIL
        _, problems = gaps.parse_index(text)
        self.assertTrue(any("names no detail entry" in p for p in problems))

    def test_a_repeated_identifier_is_named(self):
        text = INDEX_HEAD + ENTRY_G1 + ENTRY_G1 + TAIL
        _, problems = gaps.parse_index(text)
        self.assertTrue(any("indexed 2 times" in p for p in problems))

    def test_a_bullet_that_is_not_an_entry_is_named(self):
        text = INDEX_HEAD + "- a gap someone forgot to number.\n" + TAIL
        _, problems = gaps.parse_index(text)
        self.assertTrue(any("does not open" in p for p in problems))


class Markers(unittest.TestCase):
    def test_the_same_token_is_found_in_both_file_kinds(self):
        with tempfile.TemporaryDirectory() as d:
            tmp = Path(d)
            (tmp / "a.md").write_text("text\n<!-- gap: G3 -->\nmore\n")
            (tmp / "a.rs").write_text("/// Gap register: `gap: G3`\nfn f() {}\n")
            md = gaps.markers_in(tmp / "a.md", tmp)
            rs = gaps.markers_in(tmp / "a.rs", tmp)
            self.assertEqual([(m.id, m.line) for m in md], [("G3", 2)])
            self.assertEqual([(m.id, m.line) for m in rs], [("G3", 1)])


class Reconciliation(unittest.TestCase):
    def test_a_well_formed_register_resolves(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_G1 + TAIL,
                arch="## A mechanism\n\n<!-- gap: G1 -->\nWhy it costs what it costs.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)
            self.assertIn("all resolve", text)

    def test_an_indexed_gap_with_no_detail_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(Path(d), status=INDEX_HEAD + ENTRY_G1 + TAIL, arch="## A mechanism\n")
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("nothing carries its detail", text)

    def test_a_detail_entry_with_no_index_line_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_G1 + TAIL,
                arch=(
                    "## A mechanism\n\n<!-- gap: G1 -->\ntext\n\n"
                    "## Another\n\n<!-- gap: G4 -->\norphan\n"
                ),
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("carries a detail entry for G4", text)

    def test_a_code_marker_with_no_index_line_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_G1 + TAIL,
                arch="## A mechanism\n\n<!-- gap: G1 -->\ntext\n",
                code="/// Gap register: `gap: G9`\nfn f() {}\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("marks G9, which the index does not list", text)

    def test_a_detail_entry_in_the_wrong_file_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_G1 + TAIL,
                arch="## A mechanism\n",
                extra={"docs/design/roadmap.md": "<!-- gap: G1 -->\nfiled elsewhere\n"},
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("but the index names", text)

    def test_two_detail_entries_for_one_gap_fail(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_G1 + TAIL,
                arch="<!-- gap: G1 -->\none\n\n<!-- gap: G1 -->\ntwo\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("has 2 detail entries", text)

    def test_the_detail_paragraph_may_not_live_in_the_index(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_G1 + "<!-- gap: G1 -->\n" + TAIL,
                arch="<!-- gap: G1 -->\ntext\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("the index is not where a detail entry lives", text)

    def test_a_detail_file_that_does_not_exist_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_G1.replace("architecture.md", "gone.md") + TAIL,
                arch="",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("names a detail file that does not exist", text)


class ThisRepo(unittest.TestCase):
    """The register in the tree, held to its own rules."""

    def test_the_real_register_reconciles(self):
        out = io.StringIO()
        code = gaps.check(gaps.REPO, out=out)
        self.assertEqual(code, 0, out.getvalue())

    def test_every_gap_has_an_identifier_that_is_a_number(self):
        entries, problems = gaps.parse_index((gaps.STATUS).read_text())
        self.assertEqual(problems, [])
        self.assertTrue(entries)
        for entry in entries:
            self.assertTrue(entry.id[1:].isdigit())

    def test_the_index_carries_no_paragraph(self):
        """One line per gap, wrapped — an entry that has grown into a
        paragraph is the index becoming the document."""
        lines = gaps.section_lines((gaps.STATUS).read_text())
        for bullet in gaps.bullets(lines):
            self.assertLessEqual(
                len(bullet), 8, f"{bullet[0].strip()[:40]} has grown into a paragraph"
            )


if __name__ == "__main__":
    unittest.main()
