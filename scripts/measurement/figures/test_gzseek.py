"""Tests of `measurement.figures.gzseek`."""

from __future__ import annotations

import re
import unittest
import unittest.mock

from measurement import borrowing
from measurement import config
from measurement import shapes
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import declaration
from measurement.figures import gzseek
from measurement.figures import register


class GzseekBackend(unittest.TestCase):
    """`pgdt-gzip-seek`'s first pass and resumed unit decodes, an engine a build."""

    CRATE = config.REPO / "crates/pgdt-gzip-seek"
    INSTRUMENT = CRATE / "examples/gz_read.rs"

    def manifest(self) -> dict:
        import tomllib

        return tomllib.loads((self.CRATE / "Cargo.toml").read_text())

    def test_it_is_taken_and_borrows_nothing(self):
        # Standing in no sharing edge is what lets it publish outside a sweep,
        # from the commit carrying its instrument.
        fig = register.SELECTABLE_BY_ID["gzseek-backend"]
        self.assertIn(fig, register.FIGURES)
        self.assertNotIn(fig, register.UNTAKEN)
        self.assertEqual(fig.shares, ())
        self.assertEqual(borrowing.entangled_with("gzseek-backend"), [])
        self.assertEqual(fig.subtracts, ())
        self.assertEqual(fig.stage, "warm")
        self.assertEqual(fig.warm_inputs, ("control_gz",))

    def test_the_engines_are_the_crate_s_features_its_default_first(self):
        features = self.manifest()["features"]
        self.assertEqual(features["default"], [shapes.GZSEEK_ENGINES[0]])
        self.assertEqual(set(features) - {"default"}, set(shapes.GZSEEK_ENGINES))

    def test_what_can_move_it_is_declared(self):
        fig = register.SELECTABLE_BY_ID["gzseek-backend"]
        for path in (
            "crates/pgdt-gzip-seek/src/",
            "crates/pgdt-gzip-seek/examples/gz_read.rs",
            # Where the engines' versions are pinned.
            "crates/pgdt-gzip-seek/Cargo.toml",
            # The reference rows' instrument.
            "crates/gzip-inflate/src/",
            "crates/gzip-inflate/Cargo.toml",
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
        ):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)

    def test_every_shape_names_its_stage_and_the_spacing(self):
        for engine in shapes.GZSEEK_ENGINES:
            for stage in shapes.GZSEEK_STAGES:
                with self.subTest(engine=engine, stage=stage):
                    script = shapes._script(f"{shapes.GZSEEK_FAMILY}{stage}-{engine}")
                    self.assertIn(f"--shape {stage}", script)
                    self.assertIn(f"--spacing {shapes.GZSEEK_SPACING}", script)
                    self.assertEqual(script.count("time "), 1)
                    # Its stdout is where the delivered count comes back.
                    self.assertNotIn(">", script)

    def test_a_stage_or_engine_the_figure_does_not_carry_is_an_error(self):
        for command in ("gzseek-forward-miniz-oxide", "gzseek-index-zlib", "gzseek-", "gzseek-units"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_the_instrument_spells_every_stage_the_harness_asks_for(self):
        source = self.INSTRUMENT.read_text()
        accepted = set(re.findall(r'^\s*"([a-z]+)" => Shape::', source, re.MULTILINE))
        # The spacing figure's stages are this example's too.
        self.assertEqual(accepted, {*shapes.GZSEEK_STAGES, *shapes.GZSPACING_STAGES})
        self.assertIn('eq(["--engine"])', source)

    def test_a_leg_is_held_to_the_engine_it_names(self):
        # Each engine's `zlibVersion`, as the three builds report it.
        versions = {"zlib-rs": "1.3.0-zlib-rs-0.6.8", "zlib-ng": "1.3.1.zlib-ng", "zlib": "1.3.2"}
        for engine, version in versions.items():
            for leg in shapes.GZSEEK_ENGINES:
                with self.subTest(engine=engine, leg=leg):
                    problem = image.gzseek_engine_problem(leg, version)
                    if leg == engine:
                        self.assertIsNone(problem)
                    else:
                        self.assertIn(version, problem)

    def test_every_leg_is_its_own_build_and_the_references_the_inflate_instrument(self):
        specs = gzseek._gzseek_specs()
        self.assertEqual(len(specs), len(gzseek.GZSEEK_REFERENCES) + len(shapes.GZSEEK_SHAPES))
        self.assertEqual({s.regime for s in specs}, {"warm"})
        self.assertEqual({s.input for s in specs}, {"control_gz"})
        legs = {s.binary for s in specs if s.command.startswith(shapes.GZSEEK_FAMILY)}
        self.assertEqual(legs, {f"gzseek:{e}" for e in shapes.GZSEEK_ENGINES})
        references = [s for s in specs if s.binary == "gzinflate"]
        self.assertEqual(
            [s.command for s in references],
            [f"{shapes.INFLATE_FAMILY}{b}" for b in gzseek.GZSEEK_REFERENCES],
        )
        # zlib-ng's serial inflate is the floor each first pass is set against.
        self.assertIn("zlib-ng", gzseek.GZSEEK_REFERENCES)
        for backend in gzseek.GZSEEK_REFERENCES:
            self.assertIn(backend, shapes.INFLATE_BACKENDS)

    def test_each_leg_builds_into_a_target_dir_of_its_own(self):
        cfg = config.Config(dry_run=True)
        dirs = {image.image_target_dir(cfg, f"gzseek-{e}") for e in shapes.GZSEEK_ENGINES}
        self.assertEqual(len(dirs), len(shapes.GZSEEK_ENGINES))
        for leg in leg_binaries.ALLOCATOR_LEGS:
            self.assertNotIn(image.image_target_dir(cfg, leg), dirs)


class GzseekSpacing(unittest.TestCase):
    """gzip's own costs at each candidate spacing, on the shipped engine."""

    INSTRUMENT = config.REPO / "crates/pgdt-gzip-seek/examples/gz_read.rs"

    def test_it_is_taken_and_borrows_nothing(self):
        # Standing in no sharing edge is what lets it publish outside a sweep,
        # from the commit carrying its instrument.
        fig = register.SELECTABLE_BY_ID["gzseek-spacing"]
        self.assertIn(fig, register.FIGURES)
        self.assertNotIn(fig, register.UNTAKEN)
        self.assertEqual(fig.shares, ())
        self.assertEqual(borrowing.entangled_with("gzseek-spacing"), [])
        self.assertEqual(fig.subtracts, ())
        self.assertEqual(fig.stage, "warm")
        self.assertEqual(fig.warm_inputs, ("control_gz",))

    def test_what_can_move_it_is_declared(self):
        fig = register.SELECTABLE_BY_ID["gzseek-spacing"]
        for path in (
            "crates/pgdt-gzip-seek/src/",
            "crates/pgdt-gzip-seek/examples/gz_read.rs",
            "crates/pgdt-gzip-seek/Cargo.toml",
            "crates/gzip-inflate/src/",
            "crates/gzip-inflate/Cargo.toml",
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
        ):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)

    def test_it_reads_the_engine_that_ships(self):
        # The crate's default, which `GZSEEK_ENGINES` lists first (held to the
        # manifest by `GzseekBackend`).
        self.assertEqual(gzseek.GZSPACING_ENGINE, shapes.GZSEEK_ENGINES[0])
        legs = {
            s.binary
            for s in gzseek._gzspacing_specs()
            if s.command.startswith(shapes.GZSPACING_FAMILY)
        }
        self.assertEqual(legs, {f"gzseek:{gzseek.GZSPACING_ENGINE}"})

    def test_the_spacings_rise_and_reach_xz_s_block_sizes(self):
        mib = shapes.GZSPACING_MIB
        self.assertEqual(list(mib), sorted(set(mib)))
        # The 24 and 128 MiB blocks the xz figures read lie within the range.
        self.assertLessEqual(mib[0], 24)
        self.assertGreaterEqual(mib[-1], 128)
        # The backend figure's spacing is a row, so the two tables meet.
        self.assertIn(shapes.GZSEEK_SPACING // config.MIB, mib)

    def test_every_shape_names_its_stage_and_its_spacing_in_bytes(self):
        for mib in shapes.GZSPACING_MIB:
            for stage in shapes.GZSPACING_STAGES:
                with self.subTest(mib=mib, stage=stage):
                    script = shapes._script(f"{shapes.GZSPACING_FAMILY}{stage}-{mib}")
                    self.assertIn(f"--shape {stage}", script)
                    self.assertIn(f"--spacing {mib * config.MIB} ", script)
                    self.assertEqual(script.count("time "), 1)
                    # Its stdout is where the index's counts come back.
                    self.assertNotIn(">", script)

    def test_a_stage_or_spacing_the_figure_does_not_carry_is_an_error(self):
        for command in (
            "gzspacing-forward-3",
            "gzspacing-index-32",
            "gzspacing-forward-",
            "gzspacing-starts-x",
            "gzspacing-",
        ):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_the_family_cannot_be_read_as_the_backend_figure_s(self):
        # Neither prefix is the other's, so `_script` never hands one
        # figure's shape to the other's branch.
        self.assertFalse(shapes.GZSPACING_FAMILY.startswith(shapes.GZSEEK_FAMILY))
        self.assertFalse(shapes.GZSEEK_FAMILY.startswith(shapes.GZSPACING_FAMILY))

    def test_every_stage_at_every_spacing_and_the_references_the_inflate_instrument(self):
        specs = gzseek._gzspacing_specs()
        self.assertEqual(
            len(specs), len(gzseek.GZSEEK_REFERENCES) + len(shapes.GZSPACING_SHAPES)
        )
        self.assertEqual({s.regime for s in specs}, {"warm"})
        self.assertEqual({s.input for s in specs}, {"control_gz"})
        self.assertEqual(
            {s.command for s in specs if s.binary != "gzinflate"}, set(shapes.GZSPACING_SHAPES)
        )
        references = [s.command for s in specs if s.binary == "gzinflate"]
        self.assertEqual(
            references, [f"{shapes.INFLATE_FAMILY}{b}" for b in gzseek.GZSEEK_REFERENCES]
        )
        # Every label is its own, so the per-rep lines are told apart.
        self.assertEqual(len({s.label for s in specs}), len(specs))

    def test_the_instrument_prints_what_the_table_reads(self):
        source = self.INSTRUMENT.read_text()
        for key in ("engine", "delivered", "units", "windows", "started"):
            with self.subTest(key=key):
                self.assertIn(f'"{key}=', source)


if __name__ == "__main__":
    unittest.main()
