#!/usr/bin/env python3
"""Unit tests for deficiencies.py, run with
`uv run python -m unittest test_deficiencies`.

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

import deficiencies

INDEX_HEAD = """# Status

## Known deficiencies

The deficiency register. Prose above the entries, which the parser must skip.
<!-- deficiency-watermark: KD9 -->
`KD1`–`KD9` are allocated.

"""

#: The roadmap's phase index, which is where a `(b)` entry's owner is resolved.
#: A range, a state read through its emphasis, and a phase that has wrapped.
ROADMAP = """# Roadmap

Prose above the table, which the parser must skip.

| Phase | State | Where it is |
|---|---|---|
| P1–P3, P9 | **Struck** at a keystone review | architecture.md |
| P11 — typed predicates | **Specified**; open | roadmap-P11-typed-predicates.md |
| P7 — scan performance | Sketched; design doc ahead of its phase | this file |
| P12 — a phase that wrapped | Complete | this file |

Prose below the table.
"""

#: The same index with P11 sliced, which is what a status carrying the checklist
#: below has to be read against: the checklist and the `Current` cell are the two
#: halves of a phase in flight.
ROADMAP_SLICED = ROADMAP.replace("**Specified**; open", "**Current**; in flight")

ENTRY_D1 = """- **KD1** — a thing that costs something. **(c) unowned**; promoted by a
  dump in hand. Detail:
  [`../design/architecture.md`](../design/architecture.md), "A mechanism".

"""

ENTRY_D2 = """- **KD2** — another thing. **(b) owned by P7**, whose plans rework it.
  Detail: [`../design/architecture.md`](../design/architecture.md), "Another".

"""

ENTRY_D3 = """- **KD3** — a thing P11 is going to fix. **(b) owned by P11, struck at
  11.6** — 11.5 closes the first row and 11.6 the last. Detail:
  [`../design/architecture.md`](../design/architecture.md), "A mechanism".

"""

#: A phase that has been sliced, with the pairing intact: the entry names 11.5
#: and 11.6, and both lines name the entry back.
CHECKLIST = """## P11 progress

Prose above the boxes, which the parser must skip.

- [x] **11.1** A slice that landed. Notes:
      [`../design/roadmap-P11.1-x-notes.md`](../design/roadmap-P11.1-x-notes.md)
- [ ] **11.5** The first row. Closes `KD3`'s first row.
- [ ] **11.6** The last row, and **strikes `KD3`**.

"""

#: An entry whose owning phase has wrapped: `(b)` naming no live destination.
ENTRY_D4 = """- **KD4** — a thing P12 was going to fix. **(b) owned by P12**, whose
  wrap left it stranded. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Another".

"""

ARCH_D3 = """<!-- deficiency: KD3 -->
Why KD3 costs what it costs.
"""

ARCH_D4 = """<!-- deficiency: KD4 -->
Why KD4 costs what it costs.
"""

ARCH_D1_D3 = """## A mechanism

<!-- deficiency: KD1 -->
Why KD1 costs what it costs.

## Another mechanism

<!-- deficiency: KD3 -->
Why KD3 costs what it costs.
"""

TAIL = """## Decisions worth another look

*Nothing is open.*
"""


def roadmap_for(status: str) -> str:
    """The fixture index that agrees with `status` about P11.

    P11 is the phase the checklist fixture slices, and the index has to agree
    with it or every test that carries a checklist also breaks the
    checklist/state pairing and stops naming one thing. The tests for *that*
    pairing pass their own roadmap.
    """
    return ROADMAP_SLICED if 11 in deficiencies.parse_checklists(status) else ROADMAP


def build(
    tmp: Path,
    *,
    status: str,
    arch: str = "",
    code: str = "",
    roadmap: str | None = None,
    extra=None,
) -> Path:
    """A repo shaped like this one: an index, a doc tree, a crate source dir."""
    roadmap = roadmap_for(status) if roadmap is None else roadmap
    (tmp / "docs" / "status").mkdir(parents=True)
    (tmp / "docs" / "design").mkdir(parents=True)
    (tmp / "pgdump_query" / "src").mkdir(parents=True)
    (tmp / "docs" / "status" / "STATUS.md").write_text(status)
    (tmp / "docs" / "design" / "roadmap.md").write_text(roadmap)
    (tmp / "docs" / "design" / "architecture.md").write_text(arch)
    (tmp / "pgdump_query" / "src" / "lib.rs").write_text(code)
    for rel, text in (extra or {}).items():
        path = tmp / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
    return tmp


def run(tmp: Path) -> tuple[int, str]:
    out = io.StringIO()
    code = deficiencies.check(tmp, out=out)
    return code, out.getvalue()


class Parsing(unittest.TestCase):
    def test_an_entry_yields_its_stance_and_detail(self):
        entries, problems = deficiencies.parse_index(INDEX_HEAD + ENTRY_D1 + ENTRY_D2 + TAIL)
        self.assertEqual(problems, [])
        self.assertEqual([e.id for e in entries], ["KD1", "KD2"])
        self.assertEqual(entries[0].stance, "c")
        self.assertEqual(entries[0].destination, "")
        self.assertEqual(entries[1].stance, "b")
        self.assertEqual(entries[1].destination, "P7")
        self.assertEqual(entries[0].detail, "../design/architecture.md")

    def test_prose_above_the_entries_is_not_an_entry(self):
        entries, problems = deficiencies.parse_index(INDEX_HEAD + ENTRY_D1 + TAIL)
        self.assertEqual(len(entries), 1)
        self.assertEqual(problems, [])

    def test_the_section_ends_at_the_next_heading(self):
        text = INDEX_HEAD + ENTRY_D1 + "## Elsewhere\n\n" + ENTRY_D2
        entries, _ = deficiencies.parse_index(text)
        self.assertEqual([e.id for e in entries], ["KD1"])

    def test_a_deleted_section_is_a_problem_not_an_empty_register(self):
        entries, problems = deficiencies.parse_index("# Status\n\n## Not started\n\nnothing\n")
        self.assertEqual(entries, [])
        self.assertIn("the register is gone", problems[0])

    def test_a_stanceless_entry_is_named(self):
        text = INDEX_HEAD + "- **KD1** — a thing. Detail: [`a`](../design/architecture.md).\n" + TAIL
        _, problems = deficiencies.parse_index(text)
        self.assertTrue(any("KD1 declares no stance" in p for p in problems))

    def test_stance_c_must_say_unowned_in_that_word(self):
        text = INDEX_HEAD + ENTRY_D1.replace("unowned", "nobody is on it") + TAIL
        _, problems = deficiencies.parse_index(text)
        self.assertTrue(any('does not say "unowned"' in p for p in problems))

    def test_stance_b_must_name_a_destination(self):
        text = INDEX_HEAD + ENTRY_D2.replace("owned by P7", "owned") + TAIL
        _, problems = deficiencies.parse_index(text)
        self.assertTrue(any("names no destination" in p for p in problems))

    def test_stance_a_must_say_it_is_a_tradeoff(self):
        text = INDEX_HEAD + ENTRY_D1.replace("(c) unowned", "(a) fine as it is") + TAIL
        _, problems = deficiencies.parse_index(text)
        self.assertTrue(any("deliberate tradeoff" in p for p in problems))

    def test_an_entry_with_no_detail_pointer_is_named(self):
        text = INDEX_HEAD + "- **KD1** — a thing. **(c) unowned**; promoted by nothing.\n" + TAIL
        _, problems = deficiencies.parse_index(text)
        self.assertTrue(any("names no detail entry" in p for p in problems))

    def test_a_repeated_identifier_is_named(self):
        text = INDEX_HEAD + ENTRY_D1 + ENTRY_D1 + TAIL
        _, problems = deficiencies.parse_index(text)
        self.assertTrue(any("indexed 2 times" in p for p in problems))

    def test_a_bullet_that_is_not_an_entry_is_named(self):
        text = INDEX_HEAD + "- a deficiency someone forgot to number.\n" + TAIL
        _, problems = deficiencies.parse_index(text)
        self.assertTrue(any("does not open" in p for p in problems))


class Markers(unittest.TestCase):
    def test_the_same_token_is_found_in_both_file_kinds(self):
        with tempfile.TemporaryDirectory() as d:
            tmp = Path(d)
            (tmp / "a.md").write_text("text\n<!-- deficiency: KD3 -->\nmore\n")
            (tmp / "a.rs").write_text("/// Deficiency register: `deficiency: KD3`\nfn f() {}\n")
            md = deficiencies.markers_in(tmp / "a.md", tmp)
            rs = deficiencies.markers_in(tmp / "a.rs", tmp)
            self.assertEqual([(m.id, m.line) for m in md], [("KD3", 2)])
            self.assertEqual([(m.id, m.line) for m in rs], [("KD3", 1)])


class Reconciliation(unittest.TestCase):
    def test_a_well_formed_register_resolves(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + TAIL,
                arch="## A mechanism\n\n<!-- deficiency: KD1 -->\nWhy it costs what it costs.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)
            self.assertIn("all resolve", text)

    def test_an_indexed_entry_with_no_detail_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(Path(d), status=INDEX_HEAD + ENTRY_D1 + TAIL, arch="## A mechanism\n")
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("nothing carries its detail", text)

    def test_a_detail_entry_with_no_index_line_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + TAIL,
                arch=(
                    "## A mechanism\n\n<!-- deficiency: KD1 -->\ntext\n\n"
                    "## Another\n\n<!-- deficiency: KD4 -->\norphan\n"
                ),
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("carries a detail entry for KD4", text)

    def test_a_code_marker_with_no_index_line_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + TAIL,
                arch="## A mechanism\n\n<!-- deficiency: KD1 -->\ntext\n",
                code="/// Deficiency register: `deficiency: KD9`\nfn f() {}\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("marks KD9, which the index does not list", text)

    def test_a_detail_entry_in_the_wrong_file_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + TAIL,
                arch="## A mechanism\n",
                extra={"docs/design/layering.md": "<!-- deficiency: KD1 -->\nfiled elsewhere\n"},
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("but the index names", text)

    def test_two_detail_entries_for_one_entry_fail(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + TAIL,
                arch="<!-- deficiency: KD1 -->\none\n\n<!-- deficiency: KD1 -->\ntwo\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("has 2 detail entries", text)

    def test_the_detail_paragraph_may_not_live_in_the_index(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + "<!-- deficiency: KD1 -->\n" + TAIL,
                arch="<!-- deficiency: KD1 -->\ntext\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("the index is not where a detail entry lives", text)

    def test_a_detail_file_that_does_not_exist_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1.replace("architecture.md", "gone.md") + TAIL,
                arch="",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("names a detail file that does not exist", text)


class WatermarkParsing(unittest.TestCase):
    def test_the_marker_carries_the_allocated_range(self):
        mark, problems = deficiencies.parse_watermark(INDEX_HEAD + ENTRY_D1 + TAIL)
        self.assertEqual(mark, "KD9")
        self.assertEqual(problems, [])

    def test_the_prose_around_it_is_not_read(self):
        """The sentence is rewritten at every strike and again at the keystone
        that deletes the named struck entries; the marker is not."""
        text = INDEX_HEAD.replace(
            "`KD1`–`KD9` are allocated.", "`KD1`–`KD9` are allocated; `KD3` is struck."
        )
        self.assertEqual(deficiencies.parse_watermark(text)[0], "KD9")

    def test_a_missing_marker_is_a_problem(self):
        text = INDEX_HEAD.replace("<!-- deficiency-watermark: KD9 -->\n", "")
        mark, problems = deficiencies.parse_watermark(text)
        self.assertIsNone(mark)
        self.assertIn("deficiency-watermark", problems[0])

    def test_two_markers_are_a_problem(self):
        text = INDEX_HEAD + "<!-- deficiency-watermark: KD12 -->\n"
        mark, problems = deficiencies.parse_watermark(text)
        self.assertIsNone(mark)
        self.assertIn("the allocated range is one number", problems[0])

    def test_an_entry_indexed_above_the_watermark_fails(self):
        """Allocation is what the marker records, so a new entry bumps it."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1.replace("KD1", "KD12") + TAIL,
                arch="<!-- deficiency: KD12 -->\ntext\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("KD12 is indexed above the watermark (KD9)", text)


class PhaseIndexParsing(unittest.TestCase):
    def test_a_range_and_a_list_are_expanded(self):
        phases, problems = deficiencies.parse_phase_index(ROADMAP)
        self.assertEqual(problems, [])
        self.assertEqual(
            phases,
            {
                1: "struck",
                2: "struck",
                3: "struck",
                9: "struck",
                11: "specified",
                7: "sketched",
                12: "complete",
            },
        )

    def test_the_state_is_read_through_its_emphasis_and_its_caption(self):
        self.assertEqual(deficiencies.phase_state("**Specified**; open"), "specified")
        self.assertEqual(deficiencies.phase_state("Sketched; not grilled"), "sketched")
        self.assertEqual(deficiencies.phase_state("**Struck** at a keystone"), "struck")

    def test_an_unknown_state_is_named(self):
        """The vocabulary is closed: a word the check does not know would
        otherwise read as "still running", which is the silence `Complete` was
        added to end."""
        phases, problems = deficiencies.parse_phase_index(
            ROADMAP.replace("| Complete |", "| Done |")
        )
        self.assertNotIn(12, phases)
        self.assertTrue(any("carries state 'Done'" in p for p in problems))

    def test_a_missing_table_is_a_problem(self):
        phases, problems = deficiencies.parse_phase_index("# Roadmap\n\nNo table.\n")
        self.assertEqual(phases, {})
        self.assertIn("no `| Phase | State", problems[0])

    def test_the_table_ends_at_the_first_non_row(self):
        phases, _ = deficiencies.parse_phase_index(
            ROADMAP + "\n| P13 — later | Sketched | elsewhere |\n"
        )
        self.assertNotIn(13, phases)


class OwningPhaseState(unittest.TestCase):
    """A `(b)` stance names a destination, and a finished phase is not one."""

    def test_an_entry_owned_by_a_complete_phase_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(Path(d), status=INDEX_HEAD + ENTRY_D4 + TAIL, arch=ARCH_D4)
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("KD4 is (b) owned by P12, which is complete", text)
            self.assertIn("drops to (c) unowned", text)

    def test_an_entry_owned_by_a_struck_phase_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D4.replace("P12", "P2") + TAIL,
                arch=ARCH_D4,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("KD4 is (b) owned by P2, which is struck", text)

    def test_an_entry_owned_by_a_phase_the_index_does_not_list_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D4.replace("P12", "P42") + TAIL,
                arch=ARCH_D4,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("which the roadmap's phase index does not list", text)

    def test_a_complete_phase_may_not_carry_a_checklist(self):
        """Half of the phase-index read's discipline closes mechanically: the
        wrap deletes the checklist and sets the state in one change."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D1
                + "## P12 progress\n\n- [x] **12.1** A slice that landed.\n\n"
                + TAIL,
                arch="<!-- deficiency: KD1 -->\ntext\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn('P12 is complete', text)
            self.assertIn('still carries a "## P12 progress" checklist', text)

    def test_a_live_owner_of_any_open_state_is_accepted(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D2 + TAIL,
                arch="<!-- deficiency: KD2 -->\nAnother.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)


class PhaseChecklistPairing(unittest.TestCase):
    """A phase carrying a slice checklist is `Current`, and a `Current` phase
    carries one. Either half alone is a transition that half happened."""

    def test_a_current_phase_carrying_its_checklist_resolves(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + ENTRY_D3 + CHECKLIST + TAIL,
                arch=ARCH_D1_D3,
                roadmap=ROADMAP_SLICED,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)
            self.assertIn("all resolve", text)

    def test_a_checklist_under_a_specified_row_fails(self):
        """The slicing wrote the checklist and left the state."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + ENTRY_D3 + CHECKLIST + TAIL,
                arch=ARCH_D1_D3,
                roadmap=ROADMAP,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                'P11 carries a "## P11 progress" checklist and the roadmap\'s '
                "phase index calls it specified",
                text,
            )

    def test_a_current_row_with_no_checklist_fails(self):
        """The wrap deleted the checklist and left the state."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + TAIL,
                arch="<!-- deficiency: KD1 -->\nWhy KD1 costs what it costs.\n",
                roadmap=ROADMAP_SLICED,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                'P11 is current in the roadmap\'s phase index and carries no '
                '"## P11 progress" checklist',
                text,
            )

    def test_a_checklist_under_a_phase_the_index_does_not_list_fails(self):
        """A phase carrying a checklist is `Current`, and one the index does not
        list is nothing."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D1
                + "## P42 progress\n\n- [ ] **42.1** A slice.\n\n"
                + TAIL,
                arch="<!-- deficiency: KD1 -->\nWhy KD1 costs what it costs.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                'P42 carries a "## P42 progress" checklist and the roadmap\'s '
                "phase index does not list it",
                text,
            )


class ChecklistParsing(unittest.TestCase):
    def test_a_checklist_yields_its_slices_with_their_state(self):
        checklists = deficiencies.parse_checklists(INDEX_HEAD + CHECKLIST + TAIL)
        self.assertEqual(sorted(checklists), [11])
        self.assertEqual([s.id for s in checklists[11]], ["11.1", "11.5", "11.6"])
        self.assertEqual([s.done for s in checklists[11]], [True, False, False])
        self.assertIn("KD3", checklists[11][1].text)

    def test_a_wrapped_line_stays_with_its_slice(self):
        checklists = deficiencies.parse_checklists(CHECKLIST)
        self.assertIn("roadmap-P11.1-x-notes.md", checklists[11][0].text)

    def test_prose_under_the_heading_is_not_a_slice(self):
        checklists = deficiencies.parse_checklists(CHECKLIST)
        self.assertEqual(len(checklists[11]), 3)

    def test_a_phase_with_no_checklist_is_absent(self):
        checklists = deficiencies.parse_checklists(INDEX_HEAD + ENTRY_D3 + TAIL)
        self.assertEqual(checklists, {})

    def test_two_phases_in_flight_each_keep_their_own(self):
        text = CHECKLIST + "## P12 progress\n\n- [ ] **12.1** Something else.\n\n"
        checklists = deficiencies.parse_checklists(text)
        self.assertEqual(sorted(checklists), [11, 12])
        self.assertEqual([s.id for s in checklists[12]], ["12.1"])

    def test_a_sliced_phase_with_no_slices_yet_is_present_and_empty(self):
        """The heading is what says the phase has been sliced, so an
        as-yet-unfilled checklist still turns the obligation on."""
        checklists = deficiencies.parse_checklists("## P11 progress\n\nComing.\n")
        self.assertEqual(checklists, {11: []})

    def test_a_slice_reference_is_scoped_to_its_own_phase(self):
        text = "owned by P11, struck at 11.6 — 11.5 first, see roadmap-P11.3-x.md"
        self.assertEqual(deficiencies.slice_refs(text, 11), ["11.6", "11.5"])
        self.assertEqual(deficiencies.slice_refs(text, 7), [])

    def test_a_third_level_slice_is_one_reference(self):
        self.assertEqual(
            deficiencies.slice_refs("11.11.2 and 11.11", 11), ["11.11.2", "11.11"]
        )


class SlicePairing(unittest.TestCase):
    def test_a_paired_entry_and_slice_resolve(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD + ENTRY_D1 + ENTRY_D3 + CHECKLIST + TAIL,
                arch=ARCH_D1_D3,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)
            self.assertIn("paired with 11.5, 11.6", text)

    def test_an_entry_owned_by_an_unsliced_phase_owes_no_slice(self):
        """`KD2` is owned by P7, which has no checklist. The obligation lands
        when that phase is sliced, not before."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D2
                + CHECKLIST.replace("Closes `KD3`'s first row.", "").replace(
                    ", and **strikes `KD3`**", ""
                )
                + TAIL,
                arch="<!-- deficiency: KD2 -->\nAnother.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)

    def test_an_entry_owned_by_a_sliced_phase_must_name_a_slice(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D3.replace(", struck at\n  11.6", "").replace(
                    "— 11.5 closes the first row and 11.6 the last.", "It will."
                )
                + CHECKLIST.replace("Closes `KD3`'s first row.", "").replace(
                    ", and **strikes `KD3`**", ""
                )
                + TAIL,
                arch=ARCH_D3,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("which is sliced, and names no slice of it", text)

    def test_an_entry_naming_a_slice_that_no_longer_exists_fails(self):
        """The re-slice failure this relation exists for: 11.6 is renumbered
        and the entry keeps pointing at a number that has changed meaning."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D3
                + CHECKLIST.replace("**11.6**", "**11.6.1**")
                + TAIL,
                arch=ARCH_D3,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                "KD3 names slice 11.6, which the P11 checklist does not list", text
            )

    def test_an_entry_whose_slice_stopped_naming_it_fails(self):
        """The other half of a split: the slice still exists, but the closure
        moved off it and nothing re-aimed the entry."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D3
                + CHECKLIST.replace(", and **strikes `KD3`**", "")
                + TAIL,
                arch=ARCH_D3,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                "KD3 names slice 11.6, whose checklist line does not name KD3 back",
                text,
            )

    def test_a_slice_naming_an_entry_that_does_not_name_it_back_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D1
                + ENTRY_D3
                + CHECKLIST.replace(
                    "- [ ] **11.5** The first row. Closes `KD3`'s first row.",
                    "- [ ] **11.5** The first row. Closes `KD1` too.",
                ).replace(
                    "— 11.5 closes the first row and 11.6 the last.", "11.6 closes it."
                )
                + TAIL,
                arch=ARCH_D1_D3,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                "P11 checklist line 11.5 names KD1, which does not name 11.5 back",
                text,
            )

    def test_a_slice_naming_an_unindexed_entry_fails(self):
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D1
                + ENTRY_D3
                + CHECKLIST.replace("`KD3`'s first row", "`KD9`'s first row")
                + TAIL,
                arch=ARCH_D1_D3,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                "P11 checklist line 11.5 names KD9, which the index does not list",
                text,
            )

    def test_an_entry_naming_a_ticked_slice_fails(self):
        """Present tense, the other way from a checklist line: closing a part
        rewrites the entry in the change that ticks the box, so an entry naming
        a landed slice is stale whichever way it happened."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D3
                + CHECKLIST.replace(
                    "- [ ] **11.5** The first row.", "- [x] **11.5** The first row."
                )
                + TAIL,
                arch=ARCH_D3,
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn("KD3 names slice 11.5, which has landed", text)

    def test_a_ticked_line_may_cite_a_struck_entry(self):
        """A record's `KD<k>` is a citation: `KD8` was allocated and struck, and
        the line that closed it goes on saying so."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D1
                + CHECKLIST.replace(
                    "- [x] **11.1** A slice that landed.",
                    "- [x] **11.1** A slice that landed, striking `KD8`.",
                )
                .replace("- [ ] **11.5** The first row. Closes `KD3`'s first row.\n", "")
                .replace("- [ ] **11.6** The last row, and **strikes `KD3`**.\n", "")
                + TAIL,
                arch="<!-- deficiency: KD1 -->\nWhy KD1 costs what it costs.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)

    def test_a_ticked_line_citing_a_number_never_allocated_fails(self):
        """The other half of citation-resolves-against-the-range: a number
        above the watermark is a typo, not a struck entry."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D1
                + CHECKLIST.replace(
                    "- [x] **11.1** A slice that landed.",
                    "- [x] **11.1** A slice that landed, striking `KD12`.",
                )
                .replace("- [ ] **11.5** The first row. Closes `KD3`'s first row.\n", "")
                .replace("- [ ] **11.6** The last row, and **strikes `KD3`**.\n", "")
                + TAIL,
                arch="<!-- deficiency: KD1 -->\nWhy KD1 costs what it costs.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                "P11 checklist line 11.1 cites KD12, which was never allocated", text
            )

    def test_a_ticked_line_is_a_record_and_owes_no_pairing(self):
        """A landed slice's line still says which entry it closed; the entry
        has since been rewritten to what is still true, or struck outright.
        Holding a ticked line to the pairing would force one of the two to
        lie."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d),
                status=INDEX_HEAD
                + ENTRY_D1
                + CHECKLIST.replace(
                    "- [x] **11.1** A slice that landed.",
                    "- [x] **11.1** A slice that landed, closing `KD8` and `KD1`.",
                )
                .replace("- [ ] **11.5** The first row. Closes `KD3`'s first row.\n", "")
                .replace("- [ ] **11.6** The last row, and **strikes `KD3`**.\n", "")
                + TAIL,
                arch="<!-- deficiency: KD1 -->\nWhy KD1 costs what it costs.\n",
            )
            code, text = run(Path(d))
            self.assertEqual(code, 0, text)

    def test_an_entry_naming_a_slice_of_an_unsliced_phase_fails(self):
        """A checklist that was removed — the phase re-grilled, the entry left
        pointing into a slice list that is gone."""
        with tempfile.TemporaryDirectory() as d:
            build(
                Path(d), status=INDEX_HEAD + ENTRY_D3 + TAIL, arch=ARCH_D3
            )
            code, text = run(Path(d))
            self.assertEqual(code, 1)
            self.assertIn(
                'names slice 11.6, but STATUS carries no "## P11 progress" checklist',
                text,
            )


class ThisRepo(unittest.TestCase):
    """The register in the tree, held to its own rules."""

    def test_the_real_register_reconciles(self):
        out = io.StringIO()
        code = deficiencies.check(deficiencies.REPO, out=out)
        self.assertEqual(code, 0, out.getvalue())

    def test_every_entry_has_an_identifier_that_is_a_number(self):
        entries, problems = deficiencies.parse_index((deficiencies.STATUS).read_text())
        self.assertEqual(problems, [])
        self.assertTrue(entries)
        for entry in entries:
            self.assertTrue(entry.id.startswith("KD"))
            self.assertTrue(entry.id[2:].isdigit())

    def test_every_sliced_owner_is_paired_both_ways(self):
        """Not a restatement of the check: this asserts the tree actually
        exercises the fourth relation, so a repo where every `(b)` entry
        happens to be owned by an unsliced phase cannot pass it vacuously."""
        text = (deficiencies.STATUS).read_text()
        entries, problems = deficiencies.parse_index(text)
        self.assertEqual(problems, [])
        checklists = deficiencies.parse_checklists(text)
        self.assertTrue(checklists, "no phase is sliced")
        paired = [
            e
            for e in entries
            if e.stance == "b"
            and (m := deficiencies.DESTINATION_PHASE_RE.search(e.destination))
            and int(m.group(1)) in checklists
        ]
        self.assertTrue(paired, "no (b) entry is owned by a sliced phase")
        for entry in paired:
            phase = int(deficiencies.DESTINATION_PHASE_RE.search(entry.destination).group(1))
            self.assertTrue(deficiencies.slice_refs(entry.text, phase), entry.id)

    def test_the_watermark_covers_every_indexed_entry(self):
        """Not a restatement of the check: this asserts the marker is actually
        in the file, so a repo that lost it cannot pass the citation rule
        vacuously."""
        mark, problems = deficiencies.parse_watermark((deficiencies.STATUS).read_text())
        self.assertEqual(problems, [])
        entries, _ = deficiencies.parse_index((deficiencies.STATUS).read_text())
        for entry in entries:
            self.assertLessEqual(deficiencies._index(entry.id), deficiencies._index(mark))

    def test_every_b_entry_is_owned_by_a_phase_the_index_calls_live(self):
        text = (deficiencies.STATUS).read_text()
        entries, problems = deficiencies.parse_index(text)
        self.assertEqual(problems, [])
        phases, problems = deficiencies.parse_phase_index(
            (deficiencies.ROADMAP).read_text()
        )
        self.assertEqual(problems, [])
        self.assertTrue(phases, "the roadmap's phase index did not parse")
        owned = [e for e in entries if e.stance == "b"]
        self.assertTrue(owned, "no (b) entry is owned by a phase")
        for entry in owned:
            n = int(deficiencies.DESTINATION_PHASE_RE.search(entry.destination).group(1))
            self.assertIn(n, phases, entry.id)
            self.assertNotIn(phases[n], deficiencies.FINISHED_STATES, entry.id)

    def test_the_phase_in_flight_is_current_on_both_sides(self):
        """Not a restatement of the check: this asserts the tree actually has a
        phase in flight, so a repo with no checklist at all cannot pass the
        checklist/state pairing vacuously."""
        checklists = deficiencies.parse_checklists((deficiencies.STATUS).read_text())
        phases, problems = deficiencies.parse_phase_index(
            (deficiencies.ROADMAP).read_text()
        )
        self.assertEqual(problems, [])
        self.assertTrue(checklists, "no phase is sliced")
        for n in checklists:
            self.assertEqual(phases.get(n), deficiencies.CURRENT_STATE, f"P{n}")

    def test_the_index_carries_no_paragraph(self):
        """One line per entry, wrapped — an entry that has grown into a
        paragraph is the index becoming the document."""
        lines = deficiencies.section_lines((deficiencies.STATUS).read_text())
        for bullet in deficiencies.bullets(lines):
            self.assertLessEqual(
                len(bullet), 8, f"{bullet[0].strip()[:40]} has grown into a paragraph"
            )


if __name__ == "__main__":
    unittest.main()
