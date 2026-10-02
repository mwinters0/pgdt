#!/usr/bin/env python3
"""Unit tests for value_oracle.py, run with
`uv run python -m unittest test_value_oracle`.

The walk is tested over a hand-built catalog, since what it emits is SQL a
server runs; that the SQL reads what it should is the committed
`values.tsv` and `pgdump_query/tests/value_oracle.rs`, which holds every typed
read to it.
"""

from __future__ import annotations

import io
import json
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import value_oracle as vo

INT4, BOOL, TEXT, INT2, INT2VECTOR = 23, 16, 25, 21, 22
INT4_ARRAY, DOMAIN, COMPOSITE, COMPOSITE_ARRAY, RANGE, MULTIRANGE = 1007, 9001, 9002, 9003, 9004, 9005
COMPOSITE_REL, TABLE_REL = 8001, 8002


def pg_type(oid, typname, typtype="b", base=0, elem=0, is_array=False, relid=0, base_name=None):
    return {
        "oid": oid,
        "typname": typname,
        "typtype": typtype,
        "base": base,
        "base_typmod": -1,
        "base_name": base_name,
        "elem": elem,
        "is_array": is_array,
        "relid": relid,
    }


def attribute(relid, name, type_, num, generated=False):
    return {
        "relid": relid,
        "name": name,
        "type": type_,
        "typmod": -1,
        "num": num,
        "generated": generated,
    }


def catalog() -> vo.Catalog:
    return vo.parse_catalog(
        json.dumps(
            {
                "types": [
                    pg_type(INT4, "int4"),
                    pg_type(BOOL, "bool"),
                    pg_type(TEXT, "text"),
                    pg_type(INT2, "int2"),
                    pg_type(INT2VECTOR, "int2vector", elem=INT2),
                    pg_type(INT4_ARRAY, "_int4", elem=INT4, is_array=True),
                    pg_type(DOMAIN, "dom", typtype="d", base=INT4, base_name="integer"),
                    pg_type(COMPOSITE, "pair", typtype="c", relid=COMPOSITE_REL),
                    pg_type(COMPOSITE_ARRAY, "_pair", elem=COMPOSITE, is_array=True),
                    pg_type(RANGE, "r", typtype="r"),
                    pg_type(MULTIRANGE, "rm", typtype="m"),
                ],
                "ranges": [{"rngtypid": RANGE, "rngsubtype": INT4, "rngmultitypid": MULTIRANGE}],
                "attributes": [
                    attribute(COMPOSITE_REL, "a", INT4, 1),
                    attribute(COMPOSITE_REL, "b", TEXT, 2),
                    attribute(TABLE_REL, "v", INT4, 1),
                    attribute(TABLE_REL, "g", INT4, 2, generated=True),
                ],
                "tables": [{"relid": TABLE_REL, "name": "t"}],
            }
        )
    )


def walk(oid: int) -> list[vo.Node]:
    w = vo.Walk(catalog())
    w.walk("r.c", oid, -1, vo.Frame())
    return w.nodes


def paths(nodes: list[vo.Node]) -> list[str]:
    return [n.frame.path for n in nodes]


class TheWalk(unittest.TestCase):
    def test_a_scalar_is_its_reading_at_the_column_itself(self):
        [node] = walk(INT4)
        self.assertEqual(node.reading_sql, "encode(int4send(r.c), 'hex')")
        self.assertEqual(node.frame.path, "''")
        self.assertEqual(node.type_sql, f"format_type({INT4}, NULL)")

    def test_a_domain_is_read_as_its_base_through_a_cast(self):
        [node] = walk(DOMAIN)
        self.assertEqual(node.reading_sql, "encode(int4send((r.c)::integer), 'hex')")

    def test_a_leaf_no_reading_names_contributes_nothing(self):
        self.assertEqual(walk(TEXT), [])

    def test_an_array_is_its_lengths_then_each_element_by_unnest(self):
        container, element = walk(INT4_ARRAY)
        self.assertIn("array_length(r.c, d)", container.reading_sql)
        self.assertIn("'0'", container.reading_sql)
        self.assertEqual(
            element.frame.froms,
            ("CROSS JOIN LATERAL unnest(r.c) WITH ORDINALITY AS u1(v, i)",),
        )
        self.assertEqual(element.reading_sql, "encode(int4send(u1.v), 'hex')")
        self.assertEqual(element.frame.path, "'' || '[' || u1.i || ']'")

    def test_int2vector_is_read_as_the_array_it_casts_to(self):
        container, element = walk(INT2VECTOR)
        self.assertIn("(r.c)::int2[]", container.reading_sql)
        self.assertIn("unnest((r.c)::int2[])", element.frame.froms[0])

    def test_an_array_of_composites_is_read_by_subscript(self):
        """`unnest` of a composite expands into its columns in `FROM`."""
        nodes = walk(COMPOSITE_ARRAY)
        self.assertIn("generate_subscripts(r.c, 1) AS u1(s)", nodes[1].frame.froms[0])
        self.assertEqual(nodes[2].reading_sql, "encode(int4send(((r.c)[u1.s]).\"a\"), 'hex')")

    def test_a_composite_is_a_marker_and_its_fields_guarded_on_it(self):
        container, field = walk(COMPOSITE)
        self.assertEqual(container.reading_sql, "CASE WHEN num_nulls(r.c) = 0 THEN '()' END")
        self.assertEqual(field.frame.guards, ("num_nulls(r.c) = 0",))
        self.assertEqual(field.frame.path, "'' || '.a'")

    def test_a_range_is_its_bounds_and_three_flags(self):
        self.assertEqual(
            paths(walk(RANGE)),
            ["''"]
            + [
                f"'' || '.{p}'"
                for p in ("lower", "upper", "lower_inclusive", "upper_inclusive", "empty")
            ],
        )

    def test_a_multirange_is_its_count_then_each_range(self):
        nodes = walk(MULTIRANGE)
        self.assertIn("count(*)", nodes[0].reading_sql)
        self.assertEqual(nodes[1].frame.path, "'' || '[' || u1.i || ']'")
        self.assertEqual(len(nodes), 2 + 5)


class TheScript(unittest.TestCase):
    def test_a_generated_column_is_not_read(self):
        script = vo.values_script(catalog())
        self.assertIn("'v' AS col", script)
        self.assertNotIn("'g' AS col", script)

    def test_rows_are_numbered_in_ctid_order(self):
        self.assertIn("row_number() OVER (ORDER BY ctid)", vo.values_script(catalog()))


class Reconciliation(unittest.TestCase):
    def tree(self, rows: str) -> TemporaryDirectory:
        tmp = TemporaryDirectory()
        path = vo.values_path("18", Path(tmp.name))
        path.parent.mkdir(parents=True)
        path.write_text(rows)
        return tmp

    def test_a_typmod_is_dropped_wherever_it_sits(self):
        self.assertEqual(vo.base_type("timestamp(3) without time zone"), "timestamp without time zone")
        self.assertEqual(vo.base_type("numeric(38,10)"), "numeric")

    def test_every_arm_read_is_no_problem(self):
        with self.tree("public.t\tv\t1\t\tnumeric(10,2)\t-150 2\n") as d:
            self.assertEqual(vo.problems(["numeric"], Path(d), ["18"]), [])

    def test_an_arm_read_only_as_null_is_a_problem(self):
        with self.tree("public.t\tv\t1\t\tinteger\t\\N\n") as d:
            [problem] = vo.problems(["integer"], Path(d), ["18"])
            self.assertIn("`integer`", problem)

    def test_a_major_without_the_file_is_a_problem(self):
        with TemporaryDirectory() as d:
            [problem] = vo.problems([], Path(d), ["18"])
            self.assertIn("--version 18", problem)

    def test_a_short_row_is_a_problem(self):
        with self.tree("public.t\tv\t1\n") as d:
            self.assertEqual(len(vo.problems([], Path(d), ["18"])), 1)


class CommittedTree(unittest.TestCase):
    def test_the_check_passes(self):
        out = io.StringIO()
        self.assertEqual(vo.check(out=out), 0, out.getvalue())


if __name__ == "__main__":
    unittest.main()
