#!/usr/bin/env python3
"""Unit tests for oracle_register.py, run with
`uv run python -m unittest test_oracle_register`.

Stdlib `unittest`, no dependency added, same as test_oracle_differences.py.

Two halves, as in that file. The unit tests below run the parse and the
placement against synthetic sources, so a change to either fails here with a
five-line input in front of the reader rather than as a list of arms nobody can
diff. The `CommittedTree` cases at the bottom run the reconciliation over the
**real** register, the real DDL and the real `fixtures/` tree, and are the
slice's suite assertion.

The parse is the half worth testing hardest. It reads Rust, so its failure mode
is silence: a renamed function or a restructured `match` yields *fewer* arms,
and fewer arms is a check that passes. Every anchor therefore has a test that
removes it.
"""

from __future__ import annotations

import io
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import comparison_oracle as co
import oracle_register as orr

#: A register with one of everything: two names sharing a built-in arm, the
#: unrecognised fallthrough, a guarded arm, a two-kind arm, and the two
#: branches that are not match arms at all.
REGISTER_RS = """
fn builtin_scalar(base: &str, typmod: Option<&str>) -> Option<(DataType, ComparisonPlan)> {
    Some(match base.to_ascii_lowercase().as_str() {
        "integer" => (Int32, agrees(K::Int)),
        // A comment with a "quoted" word in it.
        "text" | "character varying" => (Utf8View, text),
        _ => return None,
    })
}

fn comparison_user_type(name: &str, types: &[TypeDef]) -> ComparisonPlan {
    let Some(def) = types.iter().find(|t| t.name == name) else {
        return ComparisonPlan::Refused;
    };
    match &def.kind {
        TypeKind::Enum { labels } if labels.is_empty() => ComparisonPlan::Refused,
        TypeKind::Enum { .. } => ComparisonPlan::Compared { kind: K::Text },
        TypeKind::Domain { base_type } => comparison_for(base_type, types),
        TypeKind::Composite { .. } | TypeKind::Range { .. } => ComparisonPlan::Refused,
        TypeKind::Base | TypeKind::Shell => ComparisonPlan::Refused,
    }
}

pub fn comparison_for(declared: &str, types: &[TypeDef]) -> ComparisonPlan {
    let declared = declared.trim();
    if array_element(declared).is_some() {
        return ComparisonPlan::Refused;
    }
    let (base, typmod) = split_typmod(declared);
    if base.contains('.') {
        return comparison_user_type(base, types);
    }
    builtin_scalar(base, typmod).map_or(ComparisonPlan::Refused, |(_, plan)| plan)
}
"""

SCHEMA_SQL = """
CREATE TYPE public.mood AS ENUM ('sad', 'ok');
CREATE TYPE public.empty_enum AS ENUM ();
CREATE DOMAIN public.base_domain AS integer;
CREATE TYPE public.point2d AS (x integer, y text);
CREATE TYPE public.shellonly;
CREATE TYPE public.mybase;
CREATE TYPE public.mybase (INPUT = public.mybase_in,
    OUTPUT = public.mybase_out);
CREATE TYPE public.myrange AS RANGE (
    subtype = double precision,
    multirange_type_name = public.myrange_multi
);
"""


def write(tmp: Path, register: str = REGISTER_RS, schema: str = SCHEMA_SQL):
    (tmp / "pgtype.rs").write_text(register)
    (tmp / "schema.sql").write_text(schema)
    return tmp / "pgtype.rs", tmp / "schema.sql"


class ParsingTheRegister(unittest.TestCase):
    def setUp(self):
        self.dir = TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.addCleanup(self.dir.cleanup)

    def parse(self, register: str = REGISTER_RS) -> orr.Register:
        path, _ = write(self.tmp, register=register)
        return orr.parse_register(path)

    def test_a_name_sharing_an_arm_is_its_own_arm(self):
        # `text` and `character varying` answer through one match arm and are
        # separately closable, so one of them having a case does not answer
        # for the other.
        parsed = self.parse()
        self.assertEqual(
            sorted(parsed.builtin),
            ["character varying", "integer", "text"],
        )
        self.assertEqual(parsed.problems, [])

    def test_a_quoted_word_in_a_comment_is_not_an_arm(self):
        self.assertNotIn("quoted", self.parse().builtin)

    def test_the_kind_arms_keep_their_source_order(self):
        parsed = self.parse()
        self.assertEqual(parsed.kinds["Enum"], ["user/Enum(empty)", "user/Enum"])
        self.assertEqual(parsed.kinds["Composite"], ["user/Composite|Range"])
        self.assertEqual(parsed.kinds["Range"], ["user/Composite|Range"])
        self.assertEqual(parsed.kinds["Shell"], ["user/Base|Shell"])
        self.assertEqual(parsed.guarded_empty, {"user/Enum(empty)"})

    def test_the_structural_arms_are_always_present(self):
        keys = {arm.key for arm in self.parse().arms}
        self.assertLessEqual({"array", "builtin/unrecognised", "user/absent"}, keys)

    def test_a_renamed_function_is_a_problem_not_a_shorter_list(self):
        # The parse's whole failure mode: fewer arms is a check that passes.
        parsed = self.parse(REGISTER_RS.replace("fn builtin_scalar(", "fn builtins("))
        self.assertTrue(any("builtin_scalar" in p for p in parsed.problems))
        self.assertEqual(parsed.arms, [])

    def test_a_missing_anchor_is_a_problem(self):
        parsed = self.parse(REGISTER_RS.replace("_ => return None,", "_ => None,"))
        self.assertTrue(any("return None" in p for p in parsed.problems))

    def test_a_guard_this_check_cannot_read_is_refused(self):
        parsed = self.parse(
            REGISTER_RS.replace("if labels.is_empty()", "if labels.len() < 2")
        )
        self.assertTrue(any("labels.len() < 2" in p for p in parsed.problems))
        self.assertNotIn("user/Enum(empty)", parsed.kinds.get("Enum", []))


class ParsingTheSchema(unittest.TestCase):
    def setUp(self):
        self.dir = TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.addCleanup(self.dir.cleanup)
        _, self.schema = write(self.tmp)

    def test_each_create_form_gives_its_kind(self):
        declared, _, problems = orr.parse_schema(self.schema)
        self.assertEqual(problems, [])
        self.assertEqual(
            {name: d.kind for name, d in declared.items()},
            {
                "public.mood": "Enum",
                "public.empty_enum": "Enum",
                "public.base_domain": "Domain",
                "public.point2d": "Composite",
                "public.shellonly": "Shell",
                "public.mybase": "Base",
                "public.myrange": "Range",
            },
        )

    def test_a_completed_base_type_beats_its_own_shell(self):
        declared, _, _ = orr.parse_schema(self.schema)
        self.assertEqual(declared["public.mybase"].kind, "Base")
        self.assertEqual(declared["public.shellonly"].kind, "Shell")

    def test_an_empty_enum_is_told_from_a_populated_one(self):
        declared, _, _ = orr.parse_schema(self.schema)
        self.assertTrue(declared["public.empty_enum"].empty_enum)
        self.assertFalse(declared["public.mood"].empty_enum)

    def test_a_multirange_companion_is_named_but_not_declared(self):
        declared, companions, _ = orr.parse_schema(self.schema)
        self.assertEqual(companions, {"public.myrange_multi"})
        self.assertNotIn("public.myrange_multi", declared)


class PlacingACase(unittest.TestCase):
    def setUp(self):
        self.dir = TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.addCleanup(self.dir.cleanup)
        register_path, schema_path = write(self.tmp)
        self.register = orr.parse_register(register_path)
        self.schema, self.companions, _ = orr.parse_schema(schema_path)

    def place(self, declared: str) -> str | None:
        key, _ = orr.arm_for(declared, self.register, self.schema, self.companions)
        return key

    def test_an_array_is_placed_before_anything_else_is_asked(self):
        # Including one whose element is a domain over an array: the walk
        # never looks past the brackets.
        for declared in ("integer[]", "public.point2d[]", "integer ARRAY"):
            self.assertEqual(self.place(declared), "array", declared)

    def test_a_typmod_does_not_change_the_arm(self):
        self.assertEqual(self.place("character varying(10)"), "builtin/character varying")

    def test_a_builtin_the_table_does_not_name_is_the_fallthrough(self):
        for declared in ("xml", "money", "int4range"):
            self.assertEqual(self.place(declared), "builtin/unrecognised", declared)

    def test_a_keyword_is_a_keyword_in_either_case(self):
        self.assertEqual(self.place("INTEGER"), "builtin/integer")

    def test_each_user_kind_reaches_its_own_arm(self):
        self.assertEqual(self.place("public.mood"), "user/Enum")
        self.assertEqual(self.place("public.empty_enum"), "user/Enum(empty)")
        self.assertEqual(self.place("public.base_domain"), "user/Domain")
        self.assertEqual(self.place("public.point2d"), "user/Composite|Range")
        self.assertEqual(self.place("public.myrange"), "user/Composite|Range")
        self.assertEqual(self.place("public.mybase"), "user/Base|Shell")

    def test_a_multirange_companion_reaches_the_absent_arm(self):
        self.assertEqual(self.place("public.myrange_multi"), "user/absent")

    def test_a_type_nothing_declares_is_placed_nowhere(self):
        # The hazard `comparison_oracle` warns about in prose: a case needing
        # a type the fixture schema does not declare answers "no such type" in
        # every cell and looks like coverage.
        key, why = orr.arm_for(
            "public.nosuchtype", self.register, self.schema, self.companions
        )
        self.assertIsNone(key)
        self.assertIn("not declared", why)


class Reconciling(unittest.TestCase):
    """The two directions, against a tree built to fail each of them."""

    def setUp(self):
        self.dir = TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.addCleanup(self.dir.cleanup)
        self.register_path, self.schema_path = write(self.tmp)

    def fixtures(self, cell: str) -> Path:
        """A two-major tree whose every comparison cell reads `cell`."""
        root = self.tmp / f"fixtures-{cell}"
        rows = [
            [case[0], case[1], case[2], case[3]] + [cell] * len(co.OPERATORS)
            for case in co.comparison_cases()
        ]
        for version in ("13", "14"):
            oracle = root / version / co.ORACLE_DIRNAME
            oracle.mkdir(parents=True)
            (oracle / "comparisons.tsv").write_text(co.format_tsv(rows))
        return root

    def test_every_arm_this_register_names_is_covered_by_the_real_cases(self):
        # The baseline the two failure tests below are perturbations of: the
        # synthetic register names `integer`, `text` and `character varying`,
        # and the real case table has all three.
        found = orr.reconcile(self.register_path, self.schema_path, self.fixtures("t"))
        self.assertEqual(found.problems, [])
        self.assertEqual([arm.key for arm in found.uncovered], [])

    def test_an_arm_with_no_case_is_named(self):
        register_path, _ = write(
            self.tmp,
            register=REGISTER_RS.replace(
                '"integer" =>', '"money" => (Utf8View, text),\n        "integer" =>'
            ),
        )
        found = orr.reconcile(register_path, self.schema_path, self.fixtures("t"))
        self.assertEqual([arm.key for arm in found.uncovered], ["builtin/money"])

    def test_a_case_no_server_has_the_type_for_exercises_nothing(self):
        found = orr.reconcile(
            self.register_path, self.schema_path, self.fixtures(orr.UNDEFINED_OBJECT)
        )
        # Every case type is unplaced, and the ones the synthetic schema does
        # declare are unplaced for the evidence rule rather than for want of a
        # `CREATE TYPE`.
        self.assertEqual(len(found.unplaced), len({c.type for c in co.TYPE_CASES}))
        for declared in ("integer", "text", "public.mood", "integer[]"):
            self.assertIn("exercises nothing", found.unplaced[declared])

    def test_a_missing_answer_file_is_a_problem(self):
        root = self.tmp / "empty"
        (root / "13").mkdir(parents=True)
        found = orr.reconcile(self.register_path, self.schema_path, root)
        self.assertTrue(any("comparisons.tsv" in p for p in found.problems))

    def test_the_check_fails_on_either_direction(self):
        out = io.StringIO()
        code = orr.check(
            self.register_path, self.schema_path, self.fixtures(orr.UNDEFINED_OBJECT),
            out=out,
        )
        self.assertEqual(code, 1)
        self.assertIn("resolve to no arm", out.getvalue())


class CommittedTree(unittest.TestCase):
    """The real register, the real DDL, the real fixtures."""

    def test_every_arm_has_a_case_and_every_case_an_arm(self):
        found = orr.reconcile()
        self.assertEqual(found.problems, [])
        self.assertEqual(found.unplaced, {})
        self.assertEqual([arm.key for arm in found.uncovered], [])

    def test_the_check_passes(self):
        out = io.StringIO()
        self.assertEqual(orr.check(out=out), 0, out.getvalue())

    def test_the_register_parses_into_every_group(self):
        # A parse that found nothing would satisfy "every arm has a case"
        # vacuously, so the shape of what it found is asserted too.
        found = orr.reconcile()
        groups = {arm.group for arm in found.arms}
        self.assertEqual(groups, {"builtin", "user", "structural"})
        self.assertGreaterEqual(len(found.arms), 20)


if __name__ == "__main__":
    unittest.main()
