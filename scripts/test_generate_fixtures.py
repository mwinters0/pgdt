#!/usr/bin/env python3
"""Unit tests for generate_fixtures.py's pure parts, run with
`uv run python -m unittest test_generate_fixtures`.

The generator's runs need a container and are checked by what they write;
what is tested here is what decides which DDL a major loads and what each
flag set is, which a run would only show as a load failure or a missing file.
"""

from __future__ import annotations

import unittest
from pathlib import Path
from tempfile import TemporaryDirectory

import generate_fixtures as gf


def tree(*names: str) -> TemporaryDirectory:
    tmp = TemporaryDirectory()
    for name in names:
        (Path(tmp.name) / name).write_text("")
    return tmp


class Sidecars(unittest.TestCase):
    def names(self, directory: str, schema: str, version: str) -> list[str]:
        return [p.name for p in gf.schema_files(schema, version, Path(directory))]

    def test_a_sidecar_loads_after_the_base_at_its_major_and_above(self):
        with tree("fixture_schema_types.sql", "fixture_schema_types.18.sql") as d:
            self.assertEqual(self.names(d, "types", "17"), ["fixture_schema_types.sql"])
            self.assertEqual(
                self.names(d, "types", "18"),
                ["fixture_schema_types.sql", "fixture_schema_types.18.sql"],
            )

    def test_sidecars_load_lowest_major_first_and_numerically(self):
        with tree(
            "fixture_schema_types.sql",
            "fixture_schema_types.18.sql",
            "fixture_schema_types.9.sql",
            "fixture_schema_types.16.sql",
        ) as d:
            self.assertEqual(
                self.names(d, "types", "18")[1:],
                [
                    "fixture_schema_types.9.sql",
                    "fixture_schema_types.16.sql",
                    "fixture_schema_types.18.sql",
                ],
            )

    def test_another_schema_s_files_are_not_sidecars(self):
        with tree(
            "fixture_schema_edge_cases.sql",
            "fixture_schema_edge_cases_tenant.sql",
            "fixture_schema_types.18.sql",
        ) as d:
            self.assertEqual(self.names(d, "edge_cases", "18"), ["fixture_schema_edge_cases.sql"])

    def test_a_sidecar_name_without_a_major_is_an_error(self):
        with tree("fixture_schema_types.sql", "fixture_schema_types.v18.sql") as d:
            with self.assertRaises(ValueError):
                gf.schema_files("types", "18", Path(d))


class CommittedSchemas(unittest.TestCase):
    def test_every_committed_sidecar_names_a_schema_and_a_routine_major(self):
        for path in gf.SCRIPT_DIR.glob("fixture_schema_*.*.sql"):
            schema, major = path.name.removeprefix("fixture_schema_").split(".")[:2]
            with self.subTest(path=path.name):
                self.assertIn(schema, gf.SCHEMAS)
                self.assertIn(major, gf.ROUTINE_VERSIONS)

    def test_a_setting_variant_names_one_setting_and_carries_no_flags(self):
        settings = [
            spec
            for sets in gf.SCHEMAS.values()
            for spec in sets.values()
            if isinstance(spec, gf.Setting)
        ]
        self.assertTrue(settings)
        for spec in settings:
            with self.subTest(setting=spec.name):
                self.assertNotIn("=", spec.name)
                self.assertNotIn("'", spec.value)


if __name__ == "__main__":
    unittest.main()
