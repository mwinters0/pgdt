#!/usr/bin/env python3
"""Unit tests for floor_mapping.py, run with
`uv run python -m unittest test_floor_mapping`.

Stdlib `unittest`, no dependency added and no container, in the idiom
`test_oracle_register.py` set.

Two halves. The unit tests run the parse and the verdict against synthetic
sources, so a change to either fails with a five-line input in front of the
reader. `CommittedTree` at the bottom runs the whole reconciliation over the
real `pgtype.rs`, the real `fixtures/` tree and the real `STATUS.md`, and is
the slice's suite assertion.

**The parse is the half worth testing hardest**, for the reason that file
gives: it reads Rust, so its failure mode is silence. A renamed function or a
restructured `match` yields *fewer* arms, and fewer arms is a check that
passes. Every anchor therefore has a test that removes it, and an unrenderable
`DataType` has one that proves it is reported rather than assumed to meet.
"""

from __future__ import annotations

import io
import unittest
from dataclasses import replace
from pathlib import Path
from tempfile import TemporaryDirectory

import adbc_floor
import floor_mapping as fm

#: A mapping with one of everything the parse has to handle: two names sharing
#: an arm, a tuple written on the line after the `=>`, a `DataType` carrying its
#: own commas and parentheses, an arm that delegates instead of opening a tuple,
#: and the fallthrough.
MAPPING_RS = """
fn builtin_scalar(base: &str, typmod: Option<&str>) -> Option<(DataType, ComparisonPlan)> {
    Some(match base.to_ascii_lowercase().as_str() {
        "integer" => (Int32, agrees(K::Int)),
        // A comment with a "quoted" word in it.
        "text" | "character varying" => {
            (Utf8View, collated_text(K::Text, collation, TypeCollation::Database))
        }
        "timestamp with time zone" => {
            (Timestamp(Microsecond, Some("UTC".into())), agrees(K::Timestamp { with_tz: true }))
        }
        "numeric" => map_numeric(typmod),
        _ => return None,
    })
}
"""

PYPROJECT_TOML = """
[project]
name = "pgdq-fixtures"
dependencies = [
    "adbc-driver-postgresql==1.12.0",
    "pyarrow==25.0.1",
]
"""

#: A `STATUS.md` holding the two pointers a disposition can carry.
STATUS_MD = """# Status

## P12 progress

- [ ] **12.3** `interval`.
- [ ] **12.6** `int2vector`.

## Known deficiencies

<!-- deficiency-watermark: KD13 -->

- **KD13** — `money` is below the floor. **(a) deliberate tradeoff**. Detail:
  [`../design/decisions.md`](../design/decisions.md), "D38".
"""


def floor_row(declared, arrow, *, status="ok", extension=None, version="18", driver="1.12.0"):
    """One `floor.tsv` row, `FLOOR_COLUMNS`-keyed the way `read_floor` hands
    them back."""
    return {
        "driver": driver,
        "server": version,
        "declared": declared,
        "typname": declared,
        "oid": "1",
        "typtype": "b",
        "status": status,
        "extension": extension,
        "arrow": arrow,
    }


def write_tree(root: Path, rows_by_version: dict[str, list[dict]]) -> None:
    """A `fixtures/` tree holding just the floor files, so a test can state the
    whole apparatus in one literal."""
    for version, rows in rows_by_version.items():
        adbc_floor.write_floor(
            root,
            version,
            [[row[column] for column in adbc_floor.FLOOR_COLUMNS] for row in rows],
        )


class ParsingTheMapping(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name) / "pgtype.rs"

    def parse(self, text: str = MAPPING_RS) -> fm.Mapping:
        self.path.write_text(text)
        return fm.parse_mapping(self.path)

    def test_a_name_sharing_an_arm_is_its_own_arm(self) -> None:
        """`character varying` being closable separately from `text` is the
        same reason `oracle_register.py` reads names rather than arms: a floor
        row is per declared name."""
        mapping = self.parse()
        self.assertEqual(mapping.arms["text"], "string")
        self.assertEqual(mapping.arms["character varying"], "string")

    def test_a_quoted_word_in_a_comment_is_not_an_arm(self) -> None:
        self.assertNotIn("quoted", self.parse().arms)

    def test_a_datatype_carrying_commas_is_read_whole(self) -> None:
        """The first tuple element ends at the *top-level* comma, so a type
        whose own arguments hold commas and parens survives."""
        mapping = self.parse()
        self.assertEqual(
            mapping.expressions["timestamp with time zone"],
            'Timestamp(Microsecond, Some("UTC".into()))',
        )
        self.assertEqual(mapping.arms["timestamp with time zone"], "timestamp[us, tz=UTC]")

    def test_an_arm_that_opens_no_tuple_is_unreadable_rather_than_a_problem(self) -> None:
        """`numeric` delegates to `map_numeric(typmod)`. That is silent here
        and reported only where the floor actually needs our type — which for
        `numeric` it does not, the driver answering `arrow.opaque`."""
        mapping = self.parse()
        self.assertIsNone(mapping.arms["numeric"])
        self.assertIsNone(mapping.expressions["numeric"])
        self.assertEqual(mapping.problems, [])

    def test_a_renamed_function_is_a_problem_not_a_shorter_list(self) -> None:
        mapping = self.parse(MAPPING_RS.replace("fn builtin_scalar(", "fn map_scalar("))
        self.assertEqual(mapping.arms, {})
        self.assertTrue(any("has moved" in p for p in mapping.problems))

    def test_a_missing_anchor_is_a_problem(self) -> None:
        mapping = self.parse(MAPPING_RS.replace("_ => return None,", "_ => None,"))
        self.assertTrue(any("out of date" in p for p in mapping.problems))

    def test_a_datatype_the_table_cannot_render_is_reported(self) -> None:
        """The failure this table exists for: a new arm must be *placed*
        against the driver's answer, and a fallback of "assume it meets" would
        let it land silently."""
        mapping = self.parse(MAPPING_RS.replace("(Int32, agrees", "(Decimal128(38, 9), agrees"))
        self.assertIsNone(mapping.arms["integer"])
        self.assertTrue(any("ARROW_RENDERING" in p for p in mapping.problems))


class PlacingAFloorRow(unittest.TestCase):
    def setUp(self) -> None:
        self.mapping = fm.Mapping(
            arms={"integer": "int32", "money": "string", "oid": "uint32", "numeric": None}
        )

    def kind(self, row) -> str:
        return fm.verdict_for(row, self.mapping).kind

    def test_the_same_type_meets(self) -> None:
        self.assertEqual(self.kind(floor_row("integer", "int32")), "meets")

    def test_an_unmapped_name_falls_back_to_text_and_can_still_meet(self) -> None:
        """`"char"` and `refcursor` meet precisely because the driver's own
        answer for them is a string too."""
        self.assertEqual(self.kind(floor_row("refcursor", "string")), "meets")

    def test_text_against_a_real_type_is_below(self) -> None:
        self.assertEqual(self.kind(floor_row("money", "int64")), "below")

    def test_two_real_types_that_disagree_are_a_difference_not_a_widening(self) -> None:
        self.assertEqual(self.kind(floor_row("oid", "int32")), "differs")

    def test_an_opaque_answer_leaves_the_floor_undefined(self) -> None:
        row = floor_row("inet", "binary", extension=fm.OPAQUE)
        verdict = fm.verdict_for(row, self.mapping)
        self.assertEqual(verdict.kind, "undefined")
        self.assertIn(fm.OPAQUE, verdict.why)

    def test_a_refusal_leaves_the_floor_undefined(self) -> None:
        row = floor_row("aclitem", None, status="E42883")
        verdict = fm.verdict_for(row, self.mapping)
        self.assertEqual(verdict.kind, "undefined")
        self.assertIn("E42883", verdict.why)

    def test_an_unreadable_arm_is_told_from_a_met_one(self) -> None:
        """`numeric` is opaque in the real file, so this state is unreachable
        today — but a driver release that gave it a real type must fail loudly
        rather than pass on an unread mapping."""
        self.assertEqual(self.kind(floor_row("numeric", "decimal128(38, 9)")), "unreadable")


class Reconciling(unittest.TestCase):
    def setUp(self) -> None:
        self.dir = TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.root = Path(self.dir.name)
        self.mapping_path = self.root / "pgtype.rs"
        self.mapping_path.write_text(MAPPING_RS)
        self.pyproject = self.root / "pyproject.toml"
        self.pyproject.write_text(PYPROJECT_TOML)
        self.status = self.root / "STATUS.md"
        self.status.write_text(STATUS_MD)
        self.fixtures = self.root / "fixtures"
        write_tree(
            self.fixtures,
            {"18": [floor_row("integer", "int32"), floor_row("text", "string")]},
        )

    def reconcile(self, **kwargs) -> fm.Reconciliation:
        return fm.reconcile(
            kwargs.get("mapping", self.mapping_path),
            kwargs.get("fixtures", self.fixtures),
            kwargs.get("pyproject", self.pyproject),
            kwargs.get("status", self.status),
        )

    def test_an_arm_with_no_floor_row_is_named(self) -> None:
        """The direction that decays: a type mapped with nothing behind it."""
        found = self.reconcile()
        self.assertIn("character varying", found.unevidenced)
        self.assertIn("numeric", found.unevidenced)

    def test_a_row_below_the_floor_with_no_disposition_is_unanswered(self) -> None:
        write_tree(self.fixtures, {"18": [floor_row("text", "int64")]})
        found = self.reconcile()
        self.assertEqual([v.declared for v in found.unanswered], ["text"])

    def test_a_driver_that_does_not_match_the_pin_is_a_problem(self) -> None:
        """D8: the pin and the rows are one claim about one release."""
        write_tree(self.fixtures, {"18": [floor_row("integer", "int32", driver="1.13.0")]})
        found = self.reconcile()
        self.assertTrue(any("obliges" in p for p in found.problems))

    def test_a_pyproject_pinning_no_exact_version_is_a_problem(self) -> None:
        self.pyproject.write_text(
            PYPROJECT_TOML.replace('"adbc-driver-postgresql==1.12.0"', '"adbc-driver-postgresql"')
        )
        self.assertTrue(any("D8" in p for p in self.reconcile().problems))

    def test_majors_that_disagree_about_a_type_are_a_problem(self) -> None:
        """A verdict is per type across the whole tree, so a row that is opaque
        at one major and answered at another is read rather than merged."""
        write_tree(
            self.fixtures,
            {
                "17": [floor_row("integer", "int32", version="17")],
                "18": [floor_row("integer", "int64", version="18")],
            },
        )
        self.assertTrue(any("disagree" in p for p in self.reconcile().problems))

    def test_the_check_fails_on_either_direction(self) -> None:
        out = io.StringIO()
        self.assertEqual(
            fm.check(self.mapping_path, self.fixtures, self.pyproject, self.status, out=out), 1
        )
        self.assertIn("no evidence", out.getvalue())


class Dispositions(unittest.TestCase):
    """The half that makes a waiting row close itself."""

    #: A `waiting` row, which the committed table no longer carries: `interval`
    #: and `int2vector` were the two, and both have closed. The mechanism
    #: stays because it is what the next below-floor row a slice intends to
    #: close will be held to, so it is exercised against a synthetic row rather
    #: than deleted along with the last real one.
    WAITING = fm.Disposition(
        "int2vector",
        "waiting",
        "`List<Int16>`; `int2vectorout` writes space-separated int16",
        closes="12.6",
    )

    def setUp(self) -> None:
        self.dir = TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.root = Path(self.dir.name)
        self.status = self.root / "STATUS.md"
        self.status.write_text(STATUS_MD)

    def citations(self, *dispositions: fm.Disposition) -> list[str]:
        return fm._citation_problems(dispositions, self.status)

    def test_the_committed_dispositions_resolve(self) -> None:
        """Against the real STATUS.md: every slice named is listed and every
        `KD<k>` cited is indexed."""
        self.assertEqual(fm._citation_problems(fm.DISPOSITIONS, fm.STATUS), [])

    def test_a_slice_no_checklist_lists_is_a_problem(self) -> None:
        problems = self.citations(replace(self.WAITING, closes="12.9"))
        self.assertTrue(any("re-sliced" in p for p in problems))

    def test_a_waiting_row_naming_no_slice_is_a_problem(self) -> None:
        problems = self.citations(replace(self.WAITING, closes=None))
        self.assertTrue(any("names none" in p for p in problems))

    def test_a_deficiency_the_register_does_not_index_is_a_problem(self) -> None:
        problems = self.citations(replace(fm.DISPOSITIONS[0], deficiency="KD99"))
        self.assertTrue(any("does not index" in p for p in problems))

    def test_a_stance_this_check_does_not_know_is_a_problem(self) -> None:
        problems = self.citations(replace(fm.DISPOSITIONS[0], stance="probably-fine"))
        self.assertTrue(any("not one of" in p for p in problems))


class DispositionsAgainstVerdicts(unittest.TestCase):
    """A disposition goes stale from both sides, so both are reported."""

    def setUp(self) -> None:
        self.dir = TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.root = Path(self.dir.name)
        (self.root / "pgtype.rs").write_text(MAPPING_RS)
        (self.root / "pyproject.toml").write_text(PYPROJECT_TOML)
        (self.root / "STATUS.md").write_text(STATUS_MD)
        self.fixtures = self.root / "fixtures"

    def problems(self, rows) -> list[str]:
        write_tree(self.fixtures, {"18": rows})
        return fm.reconcile(
            self.root / "pgtype.rs",
            self.fixtures,
            self.root / "pyproject.toml",
            self.root / "STATUS.md",
        ).problems

    def test_a_row_that_started_meeting_the_floor_drops_its_disposition(self) -> None:
        """This is what closes a waiting row: when the slice maps
        `int2vector`, its stance is reported as stale rather than sitting
        there excusing a type that no longer needs it."""
        problems = self.problems([floor_row("money", "string")])
        self.assertTrue(any("drop it" in p for p in problems))

    def test_a_disposition_for_a_type_the_floor_lost_is_a_problem(self) -> None:
        problems = self.problems([floor_row("integer", "int32")])
        self.assertTrue(any("was written about is gone" in p for p in problems))

    def test_a_disposition_over_an_opaque_row_is_a_problem(self) -> None:
        """The opaque tail is placed by the file's own column, so a hand-written
        line there is one nothing reads."""
        problems = self.problems(
            [floor_row("money", "binary", extension=fm.OPAQUE)]
        )
        self.assertTrue(any("placed by its own columns" in p for p in problems))

    def test_a_below_stance_over_a_row_we_model_a_type_for_is_a_problem(self) -> None:
        """`money` claims we answer no Arrow type. A mapping that gave it one
        without dropping the stance would otherwise pass."""
        mapping = self.root / "pgtype.rs"
        mapping.write_text(MAPPING_RS.replace('"integer" =>', '"money" => (Int64, x), "integer" =>'))
        write_tree(self.fixtures, {"18": [floor_row("money", "int32")]})
        found = fm.reconcile(
            mapping, self.fixtures, self.root / "pyproject.toml", self.root / "STATUS.md"
        )
        self.assertTrue(any("we answer" in p for p in found.problems))

    def test_a_narrower_stance_over_a_text_row_is_a_problem(self) -> None:
        """`oid` claims both sides are real types."""
        problems = self.problems([floor_row("oid", "int32")])
        self.assertTrue(any("both sides are real" in p for p in problems))


class CommittedTree(unittest.TestCase):
    """The real mapping, the real fixtures and the real STATUS.md."""

    def test_the_check_passes(self) -> None:
        out = io.StringIO()
        self.assertEqual(fm.check(out=out), 0, out.getvalue())

    def test_every_arm_of_the_real_mapping_renders(self) -> None:
        """Except `numeric`, whose floor row is opaque — asserted by name so
        that a second unreadable arm is a test failure rather than a silent
        widening of the exception."""
        mapping = fm.parse_mapping()
        self.assertEqual(mapping.problems, [])
        unreadable = sorted(n for n, a in mapping.arms.items() if a is None)
        self.assertEqual(unreadable, ["numeric"])

    def test_the_stances_are_the_three_the_doc_states(self) -> None:
        """Three, since `interval` and `int2vector` were `waiting` rows and
        both closed. The fourth stance the doc names — `waiting` — is exercised
        in `Dispositions` against a synthetic row."""
        self.assertEqual(
            sorted(d.declared for d in fm.DISPOSITIONS),
            ["money", "oid", "regproc"],
        )

    def test_the_opaque_tail_carries_no_hand_written_line(self) -> None:
        """The property that keeps coverage unbounded and work bounded: every
        row outside the rule is placed by `status` or `extension`."""
        found = fm.reconcile()
        undefined = [v.declared for v in found.verdicts.values() if v.kind == "undefined"]
        self.assertGreater(len(undefined), 50)
        self.assertEqual(set(undefined) & set(found.dispositions), set())


if __name__ == "__main__":
    unittest.main()
