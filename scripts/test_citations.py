#!/usr/bin/env python3
"""Unit tests for citations.py, run with
`uv run python -m unittest test_citations`.

Stdlib `unittest`, no dependency added, same as test_deficiencies.py. A citation
check has two ways to be worthless and they pull in opposite directions, so both
halves are tested here rather than the one that is easy:

* It can **miss**, and a matcher that misses reports the tree cleaner than it
  is. Every grammar the project actually writes -- a citation wrapped across
  lines, a `///` leader inside one, a bold `- **…**` list item as the target, a
  blockquote's `> `, an elided leading clause -- has a test that breaks the
  target and asserts the citation is reported.
* It can **cry wolf**, and a check that fails on prose stops being read. The
  false-positive surfaces are tested too: a string literal that is not a
  citation, a fenced transcript, a truncated clause that stops mid-word.

The structural test at the bottom runs the real check over the real tree. It
asserts what must hold whatever the tree's citations currently say -- the four
declared ids, and that the extractor still sees hundreds of citations -- and
deliberately *not* that the tree is clean, because this check lands red by
design and the repairs are a separate change.
"""

from __future__ import annotations

import io
import tempfile
import unittest
from pathlib import Path

import citations

ARCH = """# Architecture

## The map is built once, from the preamble

Prose.

- **A slice that anticipates closing an entry, and that entry, name each
  other.** A bold lead spanning two lines, cited as a section.

<!-- section: parse-profile -->

### `parse`: three-quarters of the wall is the kernel

A marked section: the heading states a finding, the id holds still.

## The bar: "the dump alone determines the value"

A heading holding a quoted phrase, which a citation cannot nest.

## The census on array-bearing rows more than triples a warm scan
"""


class Repo:
    """A small tree with one target document and whatever cites it."""

    def __init__(self, tmp: Path):
        self.root = tmp
        #: The day the check believes it is. Default is far enough ahead that
        #: every dated fixture entry counts as closed; a test about a *fresh*
        #: entry sets it to that entry's own date.
        self.today = "2999-01-01"
        (tmp / "docs" / "design").mkdir(parents=True)
        (tmp / "docs" / "status" / "history").mkdir(parents=True)
        (tmp / "pgdump_query" / "src").mkdir(parents=True)
        (tmp / "scripts").mkdir(parents=True)
        self.write("docs/design/architecture.md", ARCH)

    def write(self, rel: str, text: str) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def problems(self, today: str | None = None) -> list[str]:
        cites, docs, ids, problems, _read = citations.collect(
            self.root, today or self.today
        )
        return list(problems) + citations.reconcile(cites, docs, ids)

    def run(self, today: str | None = None) -> tuple[int, str]:
        out = io.StringIO()
        code = citations.check(self.root, out=out, today=today or self.today)
        return code, out.getvalue()


class CitationTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Repo(Path(self._tmp.name))

    def assertClean(self) -> None:
        problems = self.repo.problems()
        self.assertEqual(problems, [], "\n".join(problems))

    def assertNames(self, *needles: str) -> list[str]:
        problems = self.repo.problems()
        self.assertTrue(problems, "expected a problem, got none")
        joined = "\n".join(problems)
        for needle in needles:
            self.assertIn(needle, joined)
        return problems

    # -- what must resolve ---------------------------------------------------

    def test_exact_heading(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            'See `architecture.md`, "The map is built once, from the preamble".\n',
        )
        self.assertClean()

    def test_truncated_leading_clause(self) -> None:
        self.repo.write(
            "docs/design/notes.md", 'See `architecture.md`, "The map is built once".\n'
        )
        self.assertClean()

    def test_truncation_stops_at_a_word_boundary(self) -> None:
        """"The map is built on" is not a citation of "…built once"."""
        self.repo.write(
            "docs/design/notes.md", 'See `architecture.md`, "The map is built on".\n'
        )
        self.assertNames("The map is built on", "no section of it resolves")

    def test_bold_lead_as_a_list_item_spanning_lines(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            'See `architecture.md`, "A slice that anticipates closing an entry".\n',
        )
        self.assertClean()

    def test_citation_wraps_across_lines(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            "The fixture tree has one (`docs/design/architecture.md`, \"The map\n"
            'is built once, from the preamble") and found the opposite.\n',
        )
        self.assertClean()

    def test_wrapped_citation_is_still_checked_when_it_dangles(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            'See `docs/design/architecture.md`, "Fixtures\ntree".\n',
        )
        self.assertNames("Fixtures tree", "no section of it resolves")

    def test_quoted_phrase_in_a_heading(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            'See `architecture.md`, "The bar: the dump alone determines the value".\n',
        )
        self.assertClean()

    def test_elided_leading_clause(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            'See `architecture.md`, "…on array-bearing rows".\n',
        )
        self.assertClean()

    def test_elision_does_not_match_mid_word(self) -> None:
        self.repo.write(
            "docs/design/notes.md", 'See `architecture.md`, "…earing rows".\n'
        )
        self.assertNames("earing rows", "no section of it resolves")

    def test_second_target_on_one_citation(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            '(`architecture.md`, "The map is built once" and "Fixtures tree")\n',
        )
        problems = self.assertNames("Fixtures tree")
        self.assertEqual(len(problems), 1, problems)

    # -- how a document is named ---------------------------------------------

    def test_relative_path_reads_from_the_citing_file(self) -> None:
        self.repo.today = "2026-01-01"
        self.repo.write(
            "docs/status/history/2026-01-01.md",
            'See [`../../design/architecture.md`](../../design/architecture.md),\n'
            '"The map is built once".\n',
        )
        self.assertClean()

    def test_a_relative_path_at_the_wrong_depth_is_reported(self) -> None:
        # A wrong-depth path never resolved, so it is a typo rather than a
        # stale pointer -- caught on the day the entry is written.
        self.repo.today = "2026-01-01"
        self.repo.write(
            "docs/status/history/2026-01-01.md",
            'See `../design/architecture.md`, "The map is built once".\n',
        )
        self.assertNames("../design/architecture.md", "names no document in the tree")

    def test_document_named_only_by_a_link_target(self) -> None:
        """The out-of-band ledger's shape: the link text is a date."""
        self.repo.write(
            "docs/design/roadmap.md",
            "| `M50` | 2026-01-01 | A check | | "
            "[2026-01-01](architecture.md), \"The map is built once\" |\n",
        )
        self.assertClean()

    def test_link_target_shape_is_checked_when_it_dangles(self) -> None:
        self.repo.write(
            "docs/design/roadmap.md",
            "| `M50` | 2026-01-01 | A check | | "
            "[2026-01-01](architecture.md), \"Fixtures tree\" |\n",
        )
        problems = self.assertNames("Fixtures tree")
        self.assertEqual(len(problems), 1, problems)

    def test_a_deleted_document_is_reported(self) -> None:
        self.repo.write(
            "docs/design/notes.md", 'See `roadmap-P11-struck.md`, "Anything".\n'
        )
        self.assertNames("roadmap-P11-struck.md", "names no document in the tree")

    def test_a_gitignored_local_document_is_accepted_unchecked(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            '| `CLAUDE.local.md`, "The profiler on this machine" | paths |\n',
        )
        self.assertClean()

    # -- the marked set ------------------------------------------------------

    def test_marked_section_cited_by_id(self) -> None:
        self.repo.write(
            "docs/design/notes.md", 'See `architecture.md`, "parse-profile".\n'
        )
        self.assertClean()

    def test_marked_section_cited_bare(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            'This file names `architecture.md` once, then "parse-profile" bare.\n',
        )
        self.assertClean()

    def test_marked_section_cited_by_its_heading_fails(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            'See `architecture.md`, "`parse`: three-quarters of the wall is the '
            'kernel".\n',
        )
        self.assertNames("carries a `section:` marker", 'cite "parse-profile" instead')

    def test_marked_heading_is_not_rescued_by_truncation(self) -> None:
        """A truncated clause of a marked heading is the same lapse.

        Lenience here would leave the marker exactly as worthless as lenience on
        the whole heading: what the citation names is still a string rewritten at
        the next re-measure. So it fails, and it fails with the id.
        """
        self.repo.write(
            "docs/design/notes.md", 'See `architecture.md`, "`parse`: three-quarters".\n'
        )
        self.assertNames("carries a `section:` marker", 'cite "parse-profile" instead')

    def test_id_cited_against_the_wrong_document(self) -> None:
        self.repo.write("docs/design/other.md", "# Other\n\n## A heading\n")
        self.repo.write(
            "docs/design/notes.md", 'See `other.md`, "parse-profile".\n'
        )
        self.assertNames("that id is declared in docs/design/architecture.md")

    def test_two_documents_declaring_one_id(self) -> None:
        self.repo.write(
            "docs/design/other.md",
            "# Other\n\n<!-- section: parse-profile -->\n\n## A heading\n",
        )
        self.assertNames("already declares", "resolves by set membership")

    def test_a_one_word_id_is_refused(self) -> None:
        """Kebab-case is what stops a bare id colliding with English."""
        self.repo.write(
            "docs/design/other.md",
            "# Other\n\n<!-- section: fixtures -->\n\n## Fixtures\n",
        )
        self.assertNames("not kebab-case", "indistinguishable")

    def test_an_unmarked_heading_is_still_cited_by_heading(self) -> None:
        """The marked set is four sections, not every section."""
        self.repo.write(
            "docs/design/notes.md",
            'See `architecture.md`, "The census on array-bearing rows more than '
            'triples a warm scan".\n',
        )
        self.assertClean()

    # -- what is prose, and what is not --------------------------------------

    def test_rust_doc_comment_leader_inside_a_citation(self) -> None:
        self.repo.write(
            "pgdump_query/src/map.rs",
            "/// A shape written ahead of the archive proper\n"
            '/// (`docs/design/architecture.md`, "The map is built\n'
            "/// once, from the preamble\").\n"
            "pub fn classify() {}\n",
        )
        self.assertClean()

    def test_rust_code_is_not_prose(self) -> None:
        self.repo.write(
            "pgdump_query/src/map.rs",
            'const DOC: &str = "docs/design/architecture.md";\n'
            'const SECTION: &str = "Fixtures tree";\n',
        )
        self.assertClean()

    def test_a_citation_cannot_be_assembled_across_code(self) -> None:
        self.repo.write(
            "pgdump_query/src/map.rs",
            "/// See `docs/design/architecture.md`,\n"
            "pub fn f() {}\n"
            '/// "Fixtures tree" is not a citation of the line above it.\n'
            "pub fn g() {}\n",
        )
        self.assertClean()

    def test_python_docstring_is_prose(self) -> None:
        self.repo.write(
            "scripts/measure.py",
            '"""A module.\n\n'
            'Two builds can differ (`docs/design/architecture.md`, "Fixtures\n'
            'tree").\n"""\n',
        )
        self.assertNames("Fixtures tree")

    def test_python_comment_is_prose(self) -> None:
        self.repo.write(
            "scripts/measure.py",
            '#: Published (`docs/design/architecture.md`, "Fixtures tree").\n'
            "FIGURE = 1\n",
        )
        self.assertNames("Fixtures tree")

    def test_python_string_literal_is_not_prose(self) -> None:
        """`test_deficiencies.py`'s synthetic fixtures must stay out of scope."""
        self.repo.write(
            "scripts/test_deficiencies.py",
            "ENTRY = (\n"
            '    \'Detail: [`../design/architecture.md`](../design/architecture.md), \'\n'
            "    '\"A mechanism\".'\n"
            ")\n",
        )
        self.assertClean()

    def test_fenced_code_is_not_prose(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            "A transcript:\n\n```\n"
            '`docs/design/architecture.md`, "Fixtures tree"\n'
            "```\n",
        )
        self.assertClean()

    def test_blockquote_leader_inside_a_citation(self) -> None:
        self.repo.write(
            "docs/design/notes.md",
            "> Follow `docs/design/architecture.md`, \"The map is built once,\n"
            "> from the preamble\", step 4.\n",
        )
        self.assertClean()

    # -- the run -------------------------------------------------------------

    def test_exit_code_and_report(self) -> None:
        self.repo.write(
            "docs/design/notes.md", 'See `architecture.md`, "The map is built once".\n'
        )
        code, text = self.repo.run()
        self.assertEqual(code, 0, text)
        self.assertIn("Every citation resolves to the section it names.", text)
        self.assertIn("parse-profile", text)

        self.repo.write(
            "docs/design/notes.md", 'See `architecture.md`, "Fixtures tree".\n'
        )
        code, text = self.repo.run()
        self.assertEqual(code, 1, text)
        self.assertIn("Citations that do not resolve:", text)


class RealTreeTests(unittest.TestCase):
    """The real check over the real tree.

    Not that it passes: it lands red, and the repairs it reports are their own
    change. What is asserted is that it still *reads* the tree -- a matcher that
    silently stopped finding citations would report a clean tree and look like
    success.
    """

    def test_the_declared_ids(self) -> None:
        """The tree declares no `section:` id right now, and that is a state
        rather than a failure: the convention exists for a heading that states
        a measured finding, and the document that held five of them is gone.
        What is asserted is that every id the tree *does* declare names a
        document the walk actually opened -- a marker in a file it never reads
        would otherwise resolve against nothing."""
        _cites, docs, ids, problems, _read = citations.collect(citations.REPO)
        self.assertEqual(problems, [], "\n".join(problems))
        for name, doc in sorted(ids.items()):
            self.assertIn(doc, docs, name)

    def test_the_tree_is_still_being_read(self) -> None:
        cites, docs, _ids, _problems, _read = citations.collect(citations.REPO)
        self.assertGreater(len(cites), 400)
        self.assertGreater(len(docs), 40)
        self.assertIn("docs/design/decisions.md", docs)
        # Every file kind contributes, so a broken extractor for one of them
        # cannot hide behind the other two.
        suffixes = {Path(c.path).suffix for c in cites}
        self.assertLessEqual({".md", ".rs", ".py"}, suffixes)


class DatedEntryTests(unittest.TestCase):
    """A dated entry whose day has closed is not read for citations.

    The rule is `docs/status/history/README.md`, "An entry carries yesterday's
    truth": an entry says what was true on its date and is not maintained, so a
    keystone that deletes a phase doc leaves its citations dangling by rule. What
    is asserted here is the exemption *and* its two edges -- today's entry is
    still read, and a dated entry is still a resolvable target -- because losing
    either turns the exemption into a hole.
    """

    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.repo = Repo(Path(self._tmp.name))

    def test_a_closed_day_is_not_read(self) -> None:
        self.repo.write(
            "docs/status/history/2026-08-24.md",
            '# Aug 24\n\nFull writeup: `docs/design/gone.md`, "Verification".\n',
        )
        self.assertEqual(self.repo.problems(today="2026-08-25"), [])

    def test_todays_entry_is_read(self) -> None:
        self.repo.write(
            "docs/status/history/2026-08-24.md",
            '# Aug 24\n\nFull writeup: `docs/design/gone.md`, "Verification".\n',
        )
        problems = self.repo.problems(today="2026-08-24")
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("gone.md", problems[0])

    def test_the_readme_beside_them_is_read(self) -> None:
        self.repo.write(
            "docs/status/history/README.md",
            '# History\n\nSee `docs/design/gone.md`, "Verification".\n',
        )
        problems = self.repo.problems(today="2999-01-01")
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("gone.md", problems[0])

    def test_a_dated_entry_is_still_a_target(self) -> None:
        # The out-of-band ledger cites a dated entry by heading, so the
        # exemption is on the reading side only. Were it applied to
        # `documents()`, every ledger row would dangle at once.
        self.repo.write(
            "docs/status/history/2026-08-24.md",
            "# Aug 24\n\n## The gate needed a third opener\n\nBody.\n",
        )
        self.repo.write(
            "docs/design/roadmap.md",
            "# Roadmap\n\n| `M9` | [2026-08-24](../status/history/2026-08-24.md), "
            '"The gate needed a third opener" |\n',
        )
        self.assertEqual(self.repo.problems(today="2999-01-01"), [])

    def test_reads_citations_directly(self) -> None:
        cases = {
            ("docs/status/history/2026-08-24.md", "2026-08-25"): False,
            ("docs/status/history/2026-08-24.md", "2026-08-24"): True,
            ("docs/status/history/README.md", "2026-08-25"): True,
            ("docs/design/architecture.md", "2026-08-25"): True,
            # Not a dated entry, so not exempt -- the shape is the rule.
            ("docs/status/history/notes.md", "2026-08-25"): True,
        }
        for (rel, today), want in cases.items():
            with self.subTest(rel=rel, today=today):
                self.assertIs(citations.reads_citations(rel, today), want)


if __name__ == "__main__":
    unittest.main()
