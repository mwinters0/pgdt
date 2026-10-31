#!/usr/bin/env python3
"""Unit tests for generate_fixtures.py's pure parts, run with
`uv run python -m unittest test_generate_fixtures`.

The generator's runs need a container and are checked by what they write;
what is tested here is what decides which DDL a major loads and what each
flag set is, which a run would only show as a load failure or a missing file.
"""

from __future__ import annotations

import re
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
        clusters = {c.file.removeprefix("fixture_schema_").removesuffix(".sql") for c in gf.CLUSTER_SCRIPTS.values()}
        for path in gf.SCRIPT_DIR.glob("fixture_schema_*.*.sql"):
            schema, major = path.name.removeprefix("fixture_schema_").split(".")[:2]
            with self.subTest(path=path.name):
                self.assertIn(schema, set(gf.SCHEMAS) | clusters)
                self.assertIn(major, gf.ROUTINE_VERSIONS)

    def test_a_cluster_script_belongs_to_a_schema_and_makes_what_is_dropped(self):
        # What the script creates is what drop_fixture_db removes, or the next
        # schema's `pg_dumpall` in the same container would hold it.
        for schema, cluster in gf.CLUSTER_SCRIPTS.items():
            text = (gf.SCRIPT_DIR / cluster.file).read_text()
            every = "".join(path.read_text() for path in cluster.files(max(gf.ROUTINE_VERSIONS, key=int)))
            with self.subTest(schema=schema):
                self.assertEqual(
                    sorted(re.findall(r"^CREATE ROLE (\w+);", every, re.MULTILINE)), sorted(cluster.roles)
                )
                self.assertIn(schema, gf.SCHEMAS)
                self.assertIn(f"LOCATION '{cluster.tablespace_dir}'", text)
                for tablespace in cluster.tablespaces:
                    self.assertIn(f"CREATE TABLESPACE {tablespace} ", text)
                for database in cluster.databases:
                    self.assertIn(f'CREATE DATABASE "{database}"', text)

    def test_a_flag_set_runs_from_its_minimum_major(self):
        self.assertNotIn("stats", dict(gf.flag_sets("objects", "17")))
        self.assertEqual(dict(gf.flag_sets("objects", "18"))["stats"], ["--statistics"])
        self.assertEqual(dict(gf.flag_sets("objects", "13"))["verbose"], ["--verbose"])

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

    def test_a_compressing_set_is_written_as_gzip_and_every_other_as_sql(self):
        # A `.sql.gz` is outside every `*.sql` sweep, the generator's own
        # register join and `pgdt_fixtures::all_fixtures` both.
        compressing = [
            (schema, name)
            for schema, sets in gf.SCHEMAS.items()
            for name, spec in sets.items()
            if gf.compresses(spec[1] if isinstance(spec, tuple) else spec)
        ]
        self.assertEqual(compressing, [("edge_cases", "compress")])
        for flags in (["-Z", "6"], ["-Z6"], ["--compress=gzip:9"], ["--compress", "6"]):
            with self.subTest(flags=flags):
                self.assertEqual(gf.output_suffix(flags), ".sql.gz")
        for flags in ([], ["--no-toast-compression"], gf.Dumpall(("--clean",)), gf.Setting("a", "b")):
            with self.subTest(flags=flags):
                self.assertEqual(gf.output_suffix(flags), ".sql")


class TimescalePins(unittest.TestCase):
    def test_each_pin_is_on_a_routine_major_and_its_scripts_exist(self):
        for major, pin in gf.TIMESCALEDB_PINS.items():
            with self.subTest(major=major):
                self.assertIn(major, gf.ROUTINE_VERSIONS)
                self.assertIn(f"pg{major}.", pin.image)
                self.assertTrue(pin.image.endswith(f"-ts{pin.release}"), pin.image)
                for script in pin.scripts:
                    self.assertTrue((gf.SCRIPT_DIR / script).is_file(), script)
                    # Not a sidecar, which would load at every later major.
                    self.assertFalse(gf._SIDECAR.match(script), script)

    def test_every_flag_set_runs_at_pins_only_and_default_at_all(self):
        self.assertEqual(set(gf.TIMESCALEDB_FLAG_SETS["default"][0]), set(gf.TIMESCALEDB_PINS))
        for name, (majors, _) in gf.TIMESCALEDB_FLAG_SETS.items():
            with self.subTest(flag_set=name):
                self.assertLessEqual(set(majors), set(gf.TIMESCALEDB_PINS))

    def test_timescaledb_is_no_routine_schema(self):
        # It runs on its own servers, not in the routine container's loop.
        self.assertNotIn(gf.TIMESCALEDB_SCHEMA, gf.SCHEMAS)
        self.assertLessEqual(set(gf.ANSWERED_SCHEMAS), set(gf.SCHEMAS))


class Answers(unittest.TestCase):
    def test_a_restore_error_loses_its_location_and_nothing_else_is_kept(self):
        stderr = (
            "psql:<stdin>:193: ERROR:  relation \"x\" does not exist at character 85\n"
            "psql:<stdin>:200: NOTICE:  skipping\n"
            "ERROR:  schema \"public\" already exists\n"
        )
        self.assertEqual(
            gf.psql_errors(stderr),
            ['ERROR:  relation "x" does not exist at character 85', 'ERROR:  schema "public" already exists'],
        )

    def test_only_held_reads_with_the_planner_inactive(self):
        # A root's own rows as its restore holds them (TS8); the server's two
        # readings run under its defaults.
        for mode, (settings, source) in gf.READINGS.items():
            with self.subTest(mode=mode):
                expected = ["SET timescaledb.restoring = on"] if mode == "held" else []
                self.assertEqual(settings, expected)
                only = "public.metrics" if mode == "descendants" else "ONLY public.metrics"
                self.assertEqual(source.format("public.metrics"), only)

    def test_answers_sit_beside_the_dump(self):
        self.assertEqual(
            gf.answers_path("16", "inheritance", "default"),
            gf.FIXTURES_DIR / "16" / "inheritance" / "default.answers.tsv",
        )


if __name__ == "__main__":
    unittest.main()
