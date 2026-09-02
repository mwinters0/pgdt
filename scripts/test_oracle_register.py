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
import unittest.mock
from pathlib import Path
from tempfile import TemporaryDirectory

import comparison_oracle as co
import oracle_register as orr

#: The arms the collation dimension adds, for the tests that assert on them as
#: a set. One of them is exempt from needing a case, so nothing here may assume
#: every key has an entry in `cases_by_arm`.
COLLATION_KEYS = [arm.key for arm in orr.COLLATION_ARMS]

#: A register with one of everything: two names sharing a built-in arm, the
#: unrecognised fallthrough, a guarded arm, a two-kind arm, the two branches
#: that are not match arms at all, and one built-in arm that branches on the
#: column's collation where the other does not.
REGISTER_RS = """
fn collated_text(
    collation: Option<&str>,
    type_default: TypeCollation,
    collations: &[CollationDef],
) -> ComparisonPlan {
    if collation.is_some_and(|reference| states_non_deterministic(reference, collations)) {
        return ComparisonPlan::text_diverging(
            ComparisonDivergence::NonDeterministicCollation,
        );
    }
    let bytewise = match collation {
        Some(reference) => collation_is_bytewise(reference),
        None => type_default == TypeCollation::Bytewise,
    };
    if bytewise {
        return ComparisonPlan::agrees(CompareKind::Text);
    }
    ComparisonPlan::text_diverging(match collation {
        Some(_) => ComparisonDivergence::NonBytewiseCollation,
        None => ComparisonDivergence::UnknownCollation,
    })
}

fn builtin_scalar(base: &str, typmod: Option<&str>) -> Option<(DataType, ComparisonPlan)> {
    Some(match base.to_ascii_lowercase().as_str() {
        "integer" => (Int32, agrees(K::Int)),
        // A comment with a "quoted" word in it.
        "text" | "character varying" => {
            (Utf8View, collated_text(collation, TypeCollation::Database))
        }
        _ => return None,
    })
}

fn builtin_range_subtype(name: &str) -> Option<BuiltinRange> {
    let (subtype, multi, discrete) = match name {
        "int4range" => ("integer", false, true),
        "numrange" => ("numeric", false, false),
        "int4multirange" => ("integer", true, true),
        _ => return None,
    };
    Some(BuiltinRange { subtype, multi, discrete })
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
    if let Some((_, plan)) = builtin_scalar(base, typmod) {
        return plan;
    }
    match builtin_range_subtype(&base.to_ascii_lowercase()) {
        Some(range) => range_comparison(Some(range.subtype), range.discrete, range.multi, types),
        None => ComparisonPlan::Refused,
    }
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

    def test_only_the_arm_whose_body_branches_on_the_clause_is_collatable(self):
        # It is read from the arm's own body, so `character` joins the set the
        # day its arm starts consulting a clause and not before.
        parsed = self.parse()
        self.assertEqual(
            parsed.collatable, {"builtin/text", "builtin/character varying"}
        )

    def test_the_collation_arms_are_always_present(self):
        keys = {arm.key for arm in self.parse().arms}
        self.assertLessEqual(
            {"collation/bytewise", "collation/other", "collation/absent"}, keys
        )

    def test_a_register_that_stopped_branching_on_the_clause_is_a_problem(self):
        parsed = self.parse(
            REGISTER_RS.replace(
                "collated_text(collation, TypeCollation::Database)", "text"
            )
        )
        self.assertTrue(any("collated_text" in p for p in parsed.problems))

    def test_a_missing_collated_text_anchor_is_a_problem(self):
        parsed = self.parse(
            REGISTER_RS.replace("type_default == TypeCollation::Bytewise", "false")
        )
        self.assertTrue(any("TypeCollation::Bytewise" in p for p in parsed.problems))


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
        for declared in ("xml", "money", "int8range"):
            self.assertEqual(self.place(declared), "builtin/unrecognised", declared)

    def test_a_builtin_range_reaches_its_own_arm(self):
        # Read out of `builtin_range_subtype`'s own table, and told apart by
        # the `multi` flag there rather than by the name's shape.
        self.assertEqual(self.place("int4range"), "builtin/range")
        self.assertEqual(self.place("numrange"), "builtin/range")
        self.assertEqual(self.place("int4multirange"), "builtin/multirange")

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

    def fixtures(self, cell: str, datcollate: str = "en_US.utf8") -> Path:
        """A two-major tree whose every comparison cell reads `cell`."""
        root = self.tmp / f"fixtures-{cell}-{datcollate}"
        rows = [
            [case[0], case[1], case[2], case[3]] + [cell] * len(co.OPERATORS)
            for case in co.comparison_cases()
        ]
        for version in ("13", "14"):
            oracle = root / version / co.ORACLE_DIRNAME
            oracle.mkdir(parents=True)
            (oracle / "comparisons.tsv").write_text(co.format_tsv(rows))
            (oracle / "meta.tsv").write_text(
                co.format_tsv([["datcollate", datcollate]])
            )
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
        # Every case is unplaced, and the ones the synthetic schema does
        # declare are unplaced for the evidence rule rather than for want of a
        # `CREATE TYPE`.
        self.assertEqual(
            len(found.unplaced), len({(c.type, c.collation) for c in co.TYPE_CASES})
        )
        for declared, collation in (
            ("integer", None),
            ("text", "C"),
            ("public.mood", None),
            ("integer[]", None),
        ):
            label = orr.case_label(declared, collation)
            self.assertIn("exercises nothing", found.unplaced[label])

    def test_a_missing_answer_file_is_a_problem(self):
        root = self.tmp / "empty"
        (root / "13").mkdir(parents=True)
        found = orr.reconcile(self.register_path, self.schema_path, root)
        self.assertTrue(any("comparisons.tsv" in p for p in found.problems))

    def test_each_collation_arm_is_reached_by_the_group_that_exercises_it(self):
        found = orr.reconcile(self.register_path, self.schema_path, self.fixtures("t"))
        self.assertEqual(
            found.cases_by_arm["collation/bytewise"], ['text COLLATE "C"']
        )
        # `default` covers both of the remaining branches, there being no third
        # collation in the file to separate them.
        for key in ("collation/other", "collation/absent"):
            self.assertIn('text COLLATE "default"', found.cases_by_arm[key])

    def test_a_case_on_a_clause_blind_arm_reaches_no_collation_arm(self):
        # A label is not coverage on its own: the arm the case lands on has to
        # read a clause. `character(10)` is labelled in the real case table and
        # reaches no arm at all in this synthetic register, so its labels buy
        # nothing here.
        found = orr.reconcile(self.register_path, self.schema_path, self.fixtures("t"))
        for key in COLLATION_KEYS:
            for label in found.cases_by_arm.get(key, []):
                self.assertNotIn("character(10)", label)

    def test_a_bytewise_database_collation_is_a_problem(self):
        # The `default` group would then exercise the agreeing arm rather than
        # the two diverging ones it is mapped onto.
        found = orr.reconcile(
            self.register_path, self.schema_path, self.fixtures("t", datcollate="C")
        )
        self.assertTrue(any("datcollate" in p for p in found.problems))

    def test_an_exempt_arm_is_not_counted_uncovered_and_says_why(self):
        # `collation/non-deterministic` can have no oracle case: an ICU case
        # would import the `collversion` drift the oracle excludes ICU to
        # avoid. So it is named with its reason and does not fail the check.
        found = orr.reconcile(self.register_path, self.schema_path, self.fixtures("t"))
        exempt = [arm.key for arm in found.exempt]
        self.assertEqual(exempt, ["collation/non-deterministic"])
        self.assertNotIn("collation/non-deterministic", [a.key for a in found.uncovered])
        out = io.StringIO()
        orr.report(found, out=out)
        self.assertIn("Arms no oracle case can cover", out.getvalue())
        self.assertIn("ICU-only", out.getvalue())
        # And the report prints where the evidence is, not the reason alone.
        self.assertIn("pgdump_query/src/pgtype.rs", out.getvalue())

    def test_an_exemption_that_acquired_a_case_is_a_problem(self):
        # The other direction, and the one that decays: if the oracle ever
        # does reach the arm, the exemption is stale and must go, or it will
        # excuse the next arm hung off the same reason.
        fixtures = self.fixtures("t")
        found = orr.reconcile(self.register_path, self.schema_path, fixtures)
        self.assertEqual(found.problems, [])
        patched = dict(orr.COLLATION_GROUPS)
        patched["C"] = tuple(patched["C"]) + ("collation/non-deterministic",)
        with unittest.mock.patch.object(orr, "COLLATION_GROUPS", patched):
            found = orr.reconcile(self.register_path, self.schema_path, fixtures)
        self.assertTrue(
            any("drop the exemption" in p for p in found.problems), found.problems
        )

    def test_a_collation_branch_deleted_from_the_source_is_a_problem(self):
        # The arm list names four branches of `collated_text`; three of them
        # were already anchored, and the fourth is anchored on the call that
        # implements it, so deleting the branch is reported rather than
        # leaving an arm nothing can reach.
        register_path, _ = write(
            self.tmp,
            register=REGISTER_RS.replace("states_non_deterministic(", "never("),
        )
        parsed = orr.parse_register(register_path)
        self.assertTrue(
            any("states_non_deterministic" in p for p in parsed.problems),
            parsed.problems,
        )

    def test_the_check_fails_on_either_direction(self):
        out = io.StringIO()
        code = orr.check(
            self.register_path, self.schema_path, self.fixtures(orr.UNDEFINED_OBJECT),
            out=out,
        )
        self.assertEqual(code, 1)
        self.assertIn("resolve to no arm", out.getvalue())


class ExemptionEvidence(unittest.TestCase):
    """An exemption says where the arm's coverage is, and the check resolves
    it. A reason alone is a claim nothing checks — which is the failure the
    reconciliation exists to catch, arriving inside the reconciliation."""

    def setUp(self):
        self.dir = TemporaryDirectory()
        self.tmp = Path(self.dir.name)
        self.addCleanup(self.dir.cleanup)
        (self.tmp / "src").mkdir()
        (self.tmp / "src" / "covered.rs").write_text("fn covers_the_arm() {}\n")

    def arm(self, *evidence: orr.Evidence, unoracled: str | None = "because") -> orr.Arm:
        return orr.Arm("an/arm", "somewhere", "collation", unoracled, evidence)

    def test_evidence_that_resolves_is_no_problem(self):
        arm = self.arm(orr.Evidence("src/covered.rs", "fn covers_the_arm("))
        self.assertEqual(orr.exemption_problems([arm], self.tmp), [])

    def test_an_exemption_naming_no_evidence_is_a_problem(self):
        problems = orr.exemption_problems([self.arm()], self.tmp)
        self.assertTrue(any("names no evidence" in p for p in problems), problems)

    def test_evidence_in_a_file_that_is_gone_is_a_problem(self):
        arm = self.arm(orr.Evidence("src/deleted.rs", "fn covers_the_arm("))
        problems = orr.exemption_problems([arm], self.tmp)
        self.assertTrue(any("does not exist" in p for p in problems), problems)

    def test_a_renamed_test_no_longer_resolves(self):
        # The state an exemption asserting its own sufficiency cannot reach:
        # the file is still there and the evidence in it is not.
        arm = self.arm(orr.Evidence("src/covered.rs", "fn covers_the_arm_now("))
        problems = orr.exemption_problems([arm], self.tmp)
        self.assertTrue(any("no longer contains" in p for p in problems), problems)

    def test_evidence_on_an_arm_that_is_not_exempt_is_a_problem(self):
        # Evidence stands in for an oracle case. An arm owed a case is not
        # answered by pointing at a unit test.
        arm = self.arm(
            orr.Evidence("src/covered.rs", "fn covers_the_arm("), unoracled=None
        )
        problems = orr.exemption_problems([arm], self.tmp)
        self.assertTrue(any("not exempt" in p for p in problems), problems)

    def test_the_committed_exemptions_resolve(self):
        found = orr.reconcile()
        self.assertTrue(found.exempt)
        for arm in found.exempt:
            self.assertTrue(arm.evidence, arm.key)
        self.assertEqual(orr.exemption_problems(found.arms), [])

    def test_the_check_fails_when_an_exemption_stands_on_nothing(self):
        # End to end, through the arm list the register parse actually builds.
        stale = tuple(
            orr.Arm(arm.key, arm.where, arm.group, arm.unoracled, ())
            for arm in orr.COLLATION_ARMS
        )
        register_path, schema_path = write(self.tmp)
        fixtures = self.tmp / "fixtures"
        rows = [
            [case[0], case[1], case[2], case[3]] + ["t"] * len(co.OPERATORS)
            for case in co.comparison_cases()
        ]
        for version in ("13", "14"):
            oracle = fixtures / version / co.ORACLE_DIRNAME
            oracle.mkdir(parents=True)
            (oracle / "comparisons.tsv").write_text(co.format_tsv(rows))
            (oracle / "meta.tsv").write_text(co.format_tsv([["datcollate", "en_US.utf8"]]))
        out = io.StringIO()
        with unittest.mock.patch.object(orr, "COLLATION_ARMS", stale):
            code = orr.check(register_path, schema_path, fixtures, out=out)
        self.assertEqual(code, 1)
        self.assertIn("names no evidence", out.getvalue())


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
        self.assertEqual(groups, {"builtin", "user", "structural", "collation"})
        self.assertGreaterEqual(len(found.arms), 20)


if __name__ == "__main__":
    unittest.main()
