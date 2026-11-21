"""Tests of `measurement.recipes.profile`."""

from __future__ import annotations

import re
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import host
from measurement import parsing
from measurement import shapes
from measurement import test_support
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import release_profile
from measurement.recipes import profile


class ProfileRecipe(unittest.TestCase):
    """The sampling profile is printed here for the same reason koji's scan is,
    but its failure mode is the opposite one. koji's mistakes lose a run
    loudly; every mistake below returns a profile that looks fine and describes
    something else."""

    def _recipe(self) -> str:
        return profile.profile_recipe(config.Config())

    @staticmethod
    def _binary(cfg: config.Config) -> Path:
        return profile.PROFILE_BUILD.binary(image.image_target_dir(cfg, "profiling"))

    def test_the_profiling_binary_is_the_one_profiled(self):
        # `release` carries no line tables and no frame pointers, so a profile
        # of it is a flat list of unnameable addresses. The published figures
        # stay on `release`, which is why this is a second binary.
        cfg = config.Config()
        recipe = profile.profile_recipe(cfg)
        self.assertEqual(self._binary(cfg).parent.name, "profiling")
        self.assertIn(f"{self._binary(cfg)} parse", recipe)
        self.assertNotIn("target/release/pgdt", recipe)

    def test_frame_pointers_come_from_the_build_line(self):
        # Cargo has no profile key for them, so `[profile.profiling]` alone
        # gives line tables and a call graph that stops at the leaf.
        recipe = self._recipe()
        self.assertIn("'--rustflags=-C force-frame-pointers=yes'", recipe)
        self.assertIn("--profile=profiling", recipe)
        self.assertIn("uv run release.py build", recipe)
        self.assertNotIn("cargo build", recipe)

    def test_the_unwinder_matches_the_build(self):
        # `dwarf` needs debug info this profile does not carry, and its 8 KiB
        # stack copy per sample would change the thing being measured.
        self.assertIn("--call-graph fp", self._recipe())
        self.assertNotIn("--call-graph dwarf", self._recipe())

    def test_the_input_is_warm(self):
        # Off the SSD this profiles `pread` waiting for a device; the
        # proportions the phase reads are CPU proportions. The cold cache dir
        # may therefore appear only as the source of the staging copy.
        cfg = config.Config()
        recipe = profile.profile_recipe(cfg)
        for name in profile.PROFILE_INPUTS:
            with self.subTest(input=name):
                self.assertIn(f"--source {cfg.warm_dir / f'{name}.sql'}", recipe)
        for line in recipe.splitlines():
            if str(cfg.cache_dir) in line:
                with self.subTest(line=line):
                    self.assertTrue(line.startswith("cp "), line)

    def test_the_image_names_libc_s_frames(self):
        # Without symbols, ~48% of a warm `parse` profile is bare addresses in
        # libc.so.6 — and they are memmove and memset, which is the half of a
        # zero-copy phase's answer. The image's `libc6-dbg` is held to its
        # `libc6` by apt, and every recording and report runs there.
        dockerfile = (config.REPO / "release/Dockerfile").read_text()
        for package in ("libc6-dbg", "linux-perf", "heaptrack", "strace", "valgrind"):
            with self.subTest(package=package):
                self.assertIn(f" {package}", dockerfile)
        recipe = self._recipe()
        self.assertNotIn("debuginfod", recipe)
        self.assertNotIn("ldd ", recipe)

    def test_the_step_numbering_has_no_hole(self):
        numbered = [
            int(ln.split(".")[0][2:]) for ln in self._recipe().splitlines()
            if re.match(r"^# \d+\. ", ln)
        ]
        self.assertEqual(numbered, list(range(len(numbered))))

    def test_every_profiled_invocation_states_its_worker_count(self):
        # The sixth silent failure: a sampling profile's buckets are per
        # thread, so a profile that inherited the CLI's default would attribute
        # a scan among workers the figure it explains never ran.
        lines = self._recipe().splitlines()
        recorded = [
            (lines[i - 1], ln) for i, ln in enumerate(lines) if ln.strip().startswith("-- ")
        ]
        self.assertEqual(
            len(recorded),
            len(profile.PROFILE_INPUTS) * len(profile.PROFILE_SHAPES)
            + len(profile.DFCLI_ACCOUNT_LEGS)
            + len(release_profile.RELEASE_PROFILE_SHAPES) * len(profile.RELEASE_DIFF_LEGS),
        )
        for record, line in recorded:
            with self.subTest(line=line):
                # `pgdt sql` states its count as the figure does,
                # in the environment `perf` hands on.
                self.assertTrue(
                    f"--jobs {shapes.SWEEP_JOBS}" in line
                    or f"{shapes.DFCLI_PARTITIONS}={shapes.SWEEP_JOBS} " in record,
                    line,
                )

    def test_every_binary_runs_in_a_container(self):
        # Every run of a built binary is a reading in a memory-limited
        # container (`contained.py`), a profile's included: the limit guards
        # the machine and leaves the proportions as they were.
        cfg = config.Config()
        prefix = profile.contained_prefix(cfg, profile.PROFILE_MEMORY)
        self.assertIn(f"--memory {profile.PROFILE_MEMORY} ", prefix)
        self.assertEqual(test_support.recipe_uncontained(profile.profile_recipe(cfg)), [])
        for line in profile.profile_recipe(cfg).splitlines():
            if line.startswith(("perf ", str(self._binary(cfg)))):
                self.fail(line)

    def test_every_shape_is_profiled_over_every_input(self):
        recipe = self._recipe()
        for name in profile.PROFILE_INPUTS:
            for shape in profile.PROFILE_SHAPES:
                with self.subTest(input=name, shape=shape):
                    self.assertIn(f"profile-{shape}-{name}.data", recipe)
                    self.assertIn(f"profile-{shape}-{name}.txt", recipe)

    def test_a_profiled_shape_is_the_shape_the_sweep_times(self):
        """The reconciliation that keeps a profile readable against a figure.

        `_script` builds a container command line and `profile_argv` a host
        argv, so the two cannot be one function — but a flag that moves in one
        and not the other gives a profile of something no figure measures, and
        nothing else would notice."""
        wrapper = parsing.PEAK_RSS
        for shape in profile.PROFILE_SHAPES:
            with self.subTest(shape=shape):
                # A resident leg's wrapper is a second process a profile leaves
                # off, and nothing else of the timed line.
                # A query shape's cache builder runs ahead of the timer, and
                # is held to `profile_builder_argv` below.
                script = shapes._script(shape).rpartition(" && ")[2]
                timed = script.replace(f"{wrapper} ", "", 1).split()
                # Drop `time /pgdt`, the trailing redirect, and the container's
                # own paths; what is left is the flags both must agree on.
                self.assertEqual(timed[:2], ["time", "/pgdt"])
                timed = [w for w in timed[2:] if w != ">/dev/null"]
                profiled = profile.profile_argv(shape, "/dump.sql", "/tmp/x.pgdt")
                self.assertEqual(profiled, timed)

    def test_a_profiled_query_is_preceded_by_the_builder_its_figure_runs(self):
        """A query shape is timed over a data-level cache, so its profile is
        taken over one too: the builder `_script` runs ahead of the timer,
        flag for flag, written after the cache is removed and before `perf
        record`, and never recorded itself."""
        cfg = config.Config()
        lines = profile.profile_recipe(cfg).splitlines()
        binary = self._binary(cfg)
        c = profile.contained_prefix(cfg, profile.PROFILE_MEMORY)
        for shape in profile.PROFILE_SHAPES:
            builder = profile.profile_builder_argv(shape, "/dump.sql", "/tmp/x.pgdt")
            with self.subTest(shape=shape):
                if not shape.startswith(shapes.DATA_LEVEL_QUERIES):
                    self.assertIsNone(builder)
                    continue
                timed = shapes._script(shape).partition(" && ")[0].split()
                self.assertEqual(timed[0], "/pgdt")
                self.assertEqual(builder, [w for w in timed[1:] if w != ">/dev/null"])
                for name in profile.PROFILE_INPUTS:
                    source = cfg.warm_dir / f"{name}.sql"
                    built = profile.profile_builder_argv(shape, source, cfg.warm_dir / "profile.pgdt")
                    record = next(
                        i for i, ln in enumerate(lines)
                        if ln.startswith(f"{c} {profile.PERF} record")
                        and f"profile-{shape}-{name}.data" in ln
                    )
                    self.assertEqual(lines[record - 1], f"{c} {binary} {' '.join(built)} >/dev/null")
                    self.assertTrue(lines[record - 2].startswith("rm -f "), lines[record - 2])

    def test_the_sql_pair_and_its_reading_run_the_timed_legs(self):
        """The costing row's pair, and the introspection build's runs of it,
        state what `_script` times: the same environment, arguments and SQL,
        read out of the timed line rather than restated, over a cache the
        figure's own `pgdt parse` writes where `--source` looks."""
        cfg = config.Config()
        recipe = profile.profile_recipe(cfg)
        figure, query = profile.DFCLI_ACCOUNT
        dump = cfg.warm_dir / "dynfilter.sql"
        self.assertIn(f"cp -n {cfg.cache_dir / 'dynfilter.sql'} {dump}", recipe)
        self.assertIn(
            f"parse --source {dump} --cache {dump}.pgdt --jobs {shapes.SWEEP_JOBS} "
            f"{shapes.GATHER_STATISTICS} >/dev/null",
            recipe,
        )
        profiled = f"{self._binary(cfg)} sql"
        introspect_target = image.image_target_dir(cfg, "dfcli-introspect")
        introspected = f"{profile.DFCLI_INTROSPECT_BUILD.binary(introspect_target)} sql"
        c = profile.contained_prefix(cfg, profile.PROFILE_MEMORY)
        # The default leg against the rows leg: the setting alone apart, the
        # comparison "D93" reads.
        self.assertEqual(profile.DFCLI_ACCOUNT_LEGS, ("on", shapes.DYNFILTER_ROWS_LEG))
        self.assertNotIn(f"{figure}-{query}-off", recipe)
        for leg in profile.DFCLI_ACCOUNT_LEGS:
            with self.subTest(leg=leg):
                timed = shapes._script(f"{shapes.DYNFILTER_FAMILY}{figure}-{query}-{leg}")
                run = timed.split(" && time ", 1)[1].split(" >/tmp/result.csv", 1)[0]
                env, _, rest = run.partition(f"{shapes.SQL_SHELL} ")
                rest = rest.replace("=/dump.sql ", f"={dump} ")
                self.assertIn(f"{c} {'env ' + env if env else ''}{profile.PERF} record", recipe)
                self.assertIn(f"  -- {profiled} {rest} >/dev/null", recipe)
                for rep in range(1, profile.DFCLI_INTROSPECT_REPS + 1):
                    report = cfg.out_dir / f"introspect-dfcli-{figure}-{query}-{leg}-{rep}.txt"
                    self.assertIn(
                        f"{parsing.INSTRUMENT_OUT_VAR}={report} {env}{introspected} {rest} "
                        ">/dev/null",
                        recipe,
                    )
        self.assertIn("uv run release.py build --target x86_64-unknown-linux-gnu --features=introspect", recipe)
        self.assertIn(f"--target-dir {introspect_target})", recipe)
        self.assertIn(f"{dump}.pgdt", recipe.splitlines()[-1])

    def test_the_differential_runs_each_shape_as_the_figure_times_it(self):
        """The downstream-build rule's differential is read against a verdict,
        so each `release-profile` shape runs `_script`'s timed line and its
        builder flag for flag: a moved flag would attribute a gap the figure
        never measured."""
        for command, _ in release_profile.RELEASE_PROFILE_SHAPES:
            with self.subTest(shape=command):
                env, argv, builder = profile.release_diff_invocation(
                    command, "/dump.sql", "/tmp/x.pgdt"
                )
                script = shapes._script(command)
                before, _, timed = script.partition("time ")
                timed = timed.split(" >", 1)[0]
                self.assertEqual(f"{' '.join([*env, '/pgdt'])} {' '.join(argv)}".lstrip(),
                                 timed.replace("'", "").lstrip())
                if builder is None:
                    self.assertEqual(before, "")
                    continue
                built = before.removesuffix(" && ").split()
                self.assertEqual(built[0], "/pgdt")
                self.assertEqual(builder, [w for w in built[1:] if w != ">/dev/null"])

    def test_the_differential_profiles_the_sittings_own_legs(self):
        """No rebuild: a profile of a rebuilt leg is of another binary than
        the one the verdict timed. Each leg is the file the figure leaves,
        asked which leg it is before any recording, writes its own cache,
        which a cache from another build is refused as, and is recorded flat,
        the legs carrying no frame pointers."""
        cfg = config.Config()
        lines = profile.profile_recipe(cfg).splitlines()
        c = profile.contained_prefix(cfg, profile.PROFILE_MEMORY)
        self.assertEqual(
            profile.RELEASE_DIFF_LEGS,
            (leg_binaries.SHIPPED_PROFILE_LEG, leg_binaries.PROFILE_LEGS[0][0]),
        )
        first_record = next(
            i for i, ln in enumerate(lines) if "release-diff-" in ln and " record " in ln
        )
        for leg in profile.RELEASE_DIFF_LEGS:
            binary = leg_binaries.profile_leg_binary(cfg, leg)
            with self.subTest(leg=leg):
                self.assertEqual(binary, cfg.out_dir / f"pgdt-{leg}")
                asked = next(
                    i for i, ln in enumerate(lines)
                    if ln.startswith(f"{c} {binary} --version")
                )
                self.assertIn(f"'(profile: {leg})'", lines[asked])
                self.assertLess(asked, first_record)
                recorded = [
                    i for i, line in enumerate(lines)
                    if "release-diff-" in line and f"-{leg}.data \\" in line
                ]
                self.assertEqual(len(recorded), len(release_profile.RELEASE_PROFILE_SHAPES))
                for i in recorded:
                    line = lines[i]
                    self.assertNotIn("--call-graph", line)
                    self.assertTrue(lines[i + 1].startswith(f"  -- {binary} "), lines[i + 1])
                    if lines[i - 1].startswith(f"{c} {binary} parse "):
                        self.assertTrue(lines[i - 2].startswith("rm -f "), lines[i - 2])
                    else:
                        self.assertTrue(lines[i - 1].startswith("rm -f "), lines[i - 1])
        self.assertFalse(any("--profile release-" in ln for ln in lines))

    def test_the_differential_reads_the_default_leg_against_the_shipped_one(self):
        # `perf diff`'s first file is its baseline, so a symbol the default leg
        # alone leaves out of line is one the shipped leg's profile lacks.
        cfg = config.Config()
        recipe = profile.profile_recipe(cfg)
        shipped, default = profile.RELEASE_DIFF_LEGS
        for command, _ in release_profile.RELEASE_PROFILE_SHAPES:
            with self.subTest(shape=command):
                self.assertIn(
                    f"{profile.contained_prefix(cfg, profile.PROFILE_MEMORY)} {profile.PERF} diff --sort symbol "
                    f"{cfg.out_dir / f'release-diff-{command}-{shipped}.data'} "
                    f"{cfg.out_dir / f'release-diff-{command}-{default}.data'} > "
                    f"{cfg.out_dir / f'release-diff-{command}.txt'}",
                    recipe,
                )

    def test_the_differential_reads_the_figures_input_warm(self):
        cfg = config.Config()
        lines = profile.profile_recipe(cfg).splitlines()
        inputs = {
            spec.input
            for spec in release_profile._release_profile_specs()
            if spec.binary.startswith("profile:")
        }
        self.assertEqual(len(inputs), 1)
        (name,) = inputs
        dump = cfg.warm_dir / f"{name}.sql"
        self.assertIn(f"cp -n {cfg.cache_dir / f'{name}.sql'} {dump}", lines)
        for line in lines:
            if line.startswith("  -- ") and "pgdt-release-" in line:
                with self.subTest(line=line):
                    self.assertTrue(f"--source {dump} " in line or f"={dump} " in line, line)
        self.assertIn(f"{dump}.pgdt", lines[-1].split())

    def test_the_recipe_never_runs_anything(self):
        # The same rule koji's recipe obeys: this prints, and a session runs it
        # by hand. A harness that ran it would be taking a figure.
        with unittest.mock.patch.object(host, "run") as ran:
            with unittest.mock.patch("sys.stdout"):
                profile.cmd_profile()
        ran.assert_not_called()


if __name__ == "__main__":
    unittest.main()
