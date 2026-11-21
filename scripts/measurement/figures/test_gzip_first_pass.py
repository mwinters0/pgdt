"""Tests of `measurement.figures.gzip_first_pass`."""

from __future__ import annotations

import contextlib
import ctypes
import ctypes.util
import io
import re
import tempfile
import tomllib
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import gzip_charge
from measurement import host
from measurement import inputs
from measurement import memory_model
from measurement import replay
from measurement import shapes
from measurement import staging
from measurement.builds import leg_binaries
from measurement.figures import declaration
from measurement.figures import gzip_first_pass
from measurement.figures import register


class GzipFirstPass(unittest.TestCase):
    """A gzip first pass's wall clock against the worker count, over four
    writers' single members and at two chunk lengths."""

    FIGURE = "gzip-first-pass"
    WRITERS = tuple(leg for leg, _, _ in register.GZPASS_WRITERS)

    def test_it_is_taken_and_borrows_nothing(self):
        # Standing in no sharing edge is what lets it publish from its own
        # commit, outside a sweep; taken, it is in `FIGURES`, where the
        # doc-side checks apply.
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        self.assertIn(fig, register.FIGURES)
        self.assertNotIn(fig, register.UNTAKEN)
        self.assertEqual(fig.shares, ())
        self.assertEqual(fig.stage, "warm-parallel")
        self.assertEqual(fig.memory, shapes.GZPASS_MEMORY)
        # The floor is read against each file's pass, so each pair is one sweep's.
        self.assertEqual(
            [set(sub.inputs) for sub in fig.subtracts],
            [{"control", leg} for leg in self.WRITERS],
        )
        self.assertEqual(set(fig.warm_inputs), {"control", *self.WRITERS})
        # A sweep a writer, each beside the plain file its floor is read on.
        self.assertEqual(fig.warm_groups, tuple(("control", leg) for leg in self.WRITERS))

    def test_each_sweep_fits_the_tmpfs_budget(self):
        cfg = config.Config()
        for group in register.SELECTABLE_BY_ID[self.FIGURE].staging_groups:
            with self.subTest(group=group):
                self.assertLessEqual(
                    sum(staging.nominal_size(cfg, name) for name in group), staging.warm_bound(cfg)
                )

    def test_what_can_move_it_is_declared_and_exists(self):
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        for path in (
            *register.GZIP_READ,
            *declaration.READ,
            *declaration.SCAN,
            *declaration.QUERY_CLI,
            "scripts/generate_gzip_input.py",
            *declaration.GEN_PERF,
            # Where the chunk leg's feature is declared, through both crates.
            "crates/pgdt/Cargo.toml",
            "crates/pgdt-io/Cargo.toml",
        ):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)
                self.assertTrue((config.REPO / path).exists(), path)

    def test_every_writer_s_file_is_a_compression_of_the_control(self):
        # So every leg's plaintext is one file's, and the files differ by
        # their writer's blocks alone.
        writers = []
        for leg in self.WRITERS:
            with self.subTest(leg=leg):
                spec = inputs.INPUTS[leg]
                self.assertEqual(spec.derives_from, "control")
                self.assertEqual(spec.generator, "generate_gzip_input.py")
                self.assertEqual(inputs.input_file(leg), f"{leg}.gz")
                self.assertNotIn("--bgzf", spec.args)
                writers.append(spec.args[spec.args.index("--writer") + 1] if "--writer" in spec.args else "zlib")
        # pg_dump's own zlib, Fedora's zlib-ng-compat, GNU gzip's, and a
        # level-1 file by zlib-ng's.
        self.assertEqual(set(writers), {"zlib", "zlib-ng-compat", "zlib-ng", "gzip"})
        self.assertIn("zlib-ng-compat", inputs.INPUTS["control_gz_ng"].args)
        self.assertEqual(inputs.INPUTS["control_gz"].args, ("--from-dump", "@SOURCE@"))
        self.assertIn("1", inputs.INPUTS["control_gz_quick"].args)
        self.assertEqual(len({inputs.INPUTS[leg].args for leg in self.WRITERS}), len(self.WRITERS))

    def test_the_counts_are_the_spec_s(self):
        # Serial, the optimization baseline, and the three a default is read
        # off: the physical count, the logical count less one, the logical.
        self.assertEqual(
            shapes.GZPASS_JOBS,
            (1, 4, shapes.GZPASS_PHYSICAL, shapes.GZPASS_LOGICAL - 1, shapes.GZPASS_LOGICAL),
        )
        self.assertEqual(shapes.GZPASS_LOGICAL, 2 * shapes.GZPASS_PHYSICAL)
        # The chunk leg is read at the default count, the constant it prices
        # being one every default pass runs.
        self.assertIn(gzip_first_pass.GZPASS_CHUNK_JOBS, shapes.GZPASS_JOBS)
        self.assertEqual(gzip_first_pass.GZPASS_CHUNK_JOBS, shapes.GZPASS_PHYSICAL)

    def test_another_topology_is_refused(self):
        self.assertIsNone(
            gzip_first_pass.gzpass_topology_problem((shapes.GZPASS_LOGICAL, shapes.GZPASS_PHYSICAL))
        )
        for topology in ((16, 8), (24, 24), (12, 12)):
            with self.subTest(topology=topology):
                self.assertIn("GZPASS_LOGICAL", gzip_first_pass.gzpass_topology_problem(topology))

    def test_the_topology_is_read_off_each_cpu_s_siblings(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for cpu in range(8):
                topo = root / f"cpu{cpu}" / "topology"
                topo.mkdir(parents=True)
                topo.joinpath("thread_siblings_list").write_text(f"{cpu % 4},{cpu % 4 + 4}\n")
            self.assertEqual(gzip_first_pass.gzpass_topology(root), (8, 4))

    def test_every_shape_states_its_count_and_the_figure_s_budget(self):
        for jobs in shapes.GZPASS_JOBS:
            with self.subTest(jobs=jobs):
                script = shapes._script(f"{shapes.GZPASS_FAMILY}{jobs}")
                self.assertIn(f"--jobs {jobs} ", script)
                self.assertIn(
                    f"--memory {memory_model.stated_allowance(shapes.GZPASS_BUDGET)}", script
                )
                self.assertIn(shapes.NO_STATISTICS, script)
                self.assertEqual(script.count("time "), 1)
                self.assertIn(f"{shapes.GZPASS_FAMILY}{jobs}", shapes.command_shapes())

    def test_a_count_the_figure_does_not_carry_is_an_error(self):
        for command in ("parse-gzpass-jobs-2", "parse-gzpass-jobs-", "parse-gzpass-jobs-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_the_budget_affords_the_widest_row_at_either_chunk(self):
        # `Parallelism::fit` solves the count against the budget and
        # `margin_allowance` both, a worker charged a task; `PARALLEL_BUDGET`'s
        # 2 GiB affords fewer than twenty-four at the shipped chunk, which would
        # clamp the top rows silently were the run's own count not checked too.
        # So `gzip-resident`, whose top row is twenty-four, states this budget
        # too.
        def afforded(budget: int, chunk: int) -> int:
            allowance = memory_model.stated_allowance(budget)
            ceiling = (allowance // 100) * (
                100 - memory_model.LIBRARY_MEMORY_MARGIN_PERCENT
            ) - memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND
            per_worker = max(gzip_charge.gzip_pass_bytes(), gzip_charge.gzip_task_bytes(chunk))
            return min(budget, ceiling) // per_worker

        for chunk in (gzip_charge.GZIP_CHUNK, leg_binaries.GZPASS_CHUNK_BYTES):
            with self.subTest(chunk=chunk):
                self.assertGreaterEqual(
                    afforded(shapes.GZPASS_BUDGET, chunk), shapes.GZPASS_LOGICAL
                )
        self.assertLess(
            afforded(shapes.PARALLEL_BUDGET, gzip_charge.GZIP_CHUNK), shapes.GZPASS_LOGICAL
        )
        self.assertEqual(
            shapes.GZPASS_MEMORY,
            f"{(shapes.GZPASS_BUDGET + shapes.PARALLEL_HEADROOM) // config.GIB}g",
        )

    def test_the_chunk_leg_is_the_library_s_feature_and_its_marker(self):
        # One vocabulary in four places: the feature in both manifests, the
        # constant it selects, the marker `--version` prints and the harness
        # reads, and the lint build that turns it on.
        repo = config.REPO
        leg = leg_binaries.GZPASS_CHUNK_LEG
        pgdt = tomllib.loads((repo / "crates/pgdt/Cargo.toml").read_text())["features"]
        io_ = tomllib.loads((repo / "crates/pgdt-io/Cargo.toml").read_text())["features"]
        self.assertEqual(pgdt[leg], [f"pgdt-io/{leg}"])
        self.assertEqual(io_[leg], [])
        ahead = (
            repo / "crates/pgdt-io/src/source/compressed/deflate/first_pass/ahead.rs"
        ).read_text()
        self.assertIn(f'#[cfg(feature = "{leg}")]', ahead)
        # The leg cuts the input chunk, and says so by its name.
        self.assertEqual(leg_binaries.GZPASS_CHUNK_BYTES, gzip_charge.GZIP_INPUT_CHUNK)
        self.assertIn("const CHUNK_BYTES: NonZeroU64 = NonZeroU64::new(INPUT_CHUNK).unwrap();", ahead)
        version = (repo / "crates/pgdt/src/version.rs").read_text()
        marker = re.search(rf'cfg!\(feature = "{leg}"\) {{ " (\([^"]*\))" }}', version)
        self.assertIsNotNone(marker, "the marker is where the harness reads it")
        found = leg_binaries.CHUNK_RE.search(marker.group(1))
        self.assertEqual(int(found.group(1)) * config.MIB, leg_binaries.GZPASS_CHUNK_BYTES)
        import check

        self.assertIn(check.FeatureBuild("pgdt", (leg,)), check.FEATURE_BUILDS)

    def test_a_binary_naming_no_chunk_cuts_the_shipped_one(self):
        for out, want in (
            ("pgdt 0.2.0 (allocator: mimalloc) (profile: release) (datafusion: 55.1.0)",
             gzip_charge.GZIP_CHUNK),
            ("pgdt 0.2.0 (allocator: mimalloc) (profile: release) (gzip chunk: 1 MiB) "
             "(datafusion: 55.1.0)", leg_binaries.GZPASS_CHUNK_BYTES),
        ):
            with self.subTest(out=out):
                with unittest.mock.patch.object(host, "run", lambda *a, **k: out):
                    self.assertEqual(leg_binaries.binary_chunk(Path("pgdt")), want)

    def test_the_chunk_leg_is_built_apart_and_announced_in_a_dry_run(self):
        cfg = config.Config(dry_run=True)
        logged = []
        with unittest.mock.patch.object(leg_binaries, "_CHUNK_ANNOUNCED", set()):
            out = leg_binaries.ensure_chunk_binary(cfg, leg_binaries.GZPASS_CHUNK_LEG, logged.append)
        self.assertEqual(out.name, f"pgdt-{leg_binaries.GZPASS_CHUNK_LEG}")
        self.assertNotEqual(out, cfg.bin_pgdt)
        self.assertEqual(len(logged), 1)
        with self.assertRaises(ValueError):
            leg_binaries.ensure_chunk_binary(cfg, "gzip-8mib-chunks", logged.append)

    def test_every_sweep_reads_its_floor_its_counts_and_the_chunk_leg(self):
        specs = gzip_first_pass._gzpass_specs()
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        parts = staging.split_specs(self.FIGURE, specs, fig.warm_groups)
        self.assertEqual([gi for gi, _ in parts], list(range(len(self.WRITERS))))
        for (gi, part), leg in zip(parts, self.WRITERS):
            with self.subTest(leg=leg):
                self.assertEqual({s.input for s in part}, {"control", leg})
                floor = [s for s in part if s.input == "control"]
                self.assertEqual([s.command for s in floor], [f"{shapes.GZPASS_FAMILY}1"])
                self.assertEqual(floor[0].sweep, gi or None)
                shipped = [s for s in part if s.input == leg and s.binary == "pgdt"]
                self.assertEqual(
                    [s.command for s in shipped],
                    [f"{shapes.GZPASS_FAMILY}{j}" for j in shapes.GZPASS_JOBS],
                )
                chunk = [s for s in part if s.binary == leg_binaries.GZPASS_CHUNK_BINARY]
                self.assertEqual(
                    [s.command for s in chunk],
                    [f"{shapes.GZPASS_FAMILY}{gzip_first_pass.GZPASS_CHUNK_JOBS}"],
                )
        self.assertEqual({s.regime for s in specs}, {"warm-parallel"})
        self.assertEqual(len({s.key(self.FIGURE) for s in specs}), len(specs))
        self.assertEqual(len({s.label for s in specs}), len(specs))

    # -- the rule ------------------------------------------------------------

    def test_a_leg_beats_another_in_all_its_pairs_but_one(self):
        fast, slow = [1.0] * 7, [2.0] * 7
        self.assertTrue(gzip_first_pass.gzpass_beats(fast, slow))
        one_lost = [3.0] + [1.0] * 6
        self.assertTrue(gzip_first_pass.gzpass_beats(one_lost, slow))
        two_lost = [3.0, 3.0] + [1.0] * 5
        self.assertFalse(gzip_first_pass.gzpass_beats(two_lost, slow))
        self.assertTrue(gzip_first_pass.gzpass_level(two_lost, slow))
        # Ties win nothing either way.
        self.assertTrue(gzip_first_pass.gzpass_level(fast, fast))
        # A short sitting still needs more than half.
        self.assertFalse(gzip_first_pass.gzpass_beats([1.0, 3.0], [2.0, 2.0]))
        with self.assertRaises(ValueError):
            gzip_first_pass.gzpass_wins([1.0], [1.0, 2.0])

    def test_a_leg_beats_another_only_by_the_margin(self):
        # Seven wins of seven by under the margin are level, not a win.
        base = [1.0] * 7
        close = [1.0 + gzip_first_pass.GZPASS_MARGIN / 2] * 7
        clear = [1.0 + gzip_first_pass.GZPASS_MARGIN * 2] * 7
        self.assertEqual(gzip_first_pass.gzpass_wins(base, close), 7)
        self.assertFalse(gzip_first_pass.gzpass_beats(base, close))
        self.assertTrue(gzip_first_pass.gzpass_level(base, close))
        self.assertTrue(gzip_first_pass.gzpass_beats(base, clear))
        # It is a share of the median, so it scales with the leg.
        self.assertFalse(gzip_first_pass.gzpass_beats([10.0] * 7, [10.0 + gzip_first_pass.GZPASS_MARGIN * 2] * 7))

    def test_each_outcome(self):
        n = 7
        serial, fast, mid, slow = [4.0] * n, [1.0] * n, [2.0] * n, [3.0] * n
        o = gzip_first_pass.gzpass_outcome
        self.assertEqual(o(serial, fast, fast, slow), "oversubscription")
        self.assertEqual(o(serial, fast, slow, slow), "shared")
        self.assertEqual(o(serial, fast, fast, fast), "level")
        # The mixed orders settle: 12 > 23 > 24 is a core shared, and 23
        # level with 12 and both beating 24 oversubscription.
        self.assertEqual(o(serial, fast, mid, slow), "shared")
        self.assertEqual(o(serial, mid, mid, slow), "oversubscription")
        # Either upper count beating the physical count reopens, whatever
        # the other does: the physical count is the incumbent.
        self.assertEqual(o(serial, mid, slow, fast), "reopens")
        self.assertEqual(o(serial, mid, fast, slow), "reopens")
        self.assertEqual(o(serial, mid, fast, fast), "reopens")
        # ... on a file it or the physical count lifts, even where the
        # physical count lifts on none.
        self.assertEqual(o(serial, serial, fast, serial), "reopens")
        self.assertEqual(o(serial, serial, serial, serial), "no lift")
        # An upper count lifting a file without beating the physical count
        # leaves it `no lift`: the other outcomes read the default's lift.
        near, nearer = [3.95] * n, [3.85] * n
        self.assertTrue(gzip_first_pass.gzpass_beats(nearer, serial))
        self.assertTrue(gzip_first_pass.gzpass_level(near, serial))
        self.assertTrue(gzip_first_pass.gzpass_level(nearer, near))
        self.assertEqual(o(serial, near, nearer, nearer), "no lift")
        # Beating a physical count that regresses, on a file no count lifts,
        # is no reopening: the regression is speculation's, not the cap's.
        worse, less_worse = [4.4] * n, [4.05] * n
        self.assertTrue(gzip_first_pass.gzpass_beats(less_worse, worse))
        self.assertTrue(gzip_first_pass.gzpass_level(less_worse, serial))
        self.assertEqual(o(serial, worse, less_worse, less_worse), "regresses")
        # Serial beating any upper count is a regression, named apart.
        self.assertEqual(o(fast, slow, fast, fast), "regresses")
        self.assertEqual(o(mid, mid, mid, slow), "regresses")
        self.assertEqual(
            set(gzip_first_pass.GZPASS_OUTCOMES),
            {"reopens", "level", "oversubscription", "shared", "no lift", "regresses"},
        )
        self.assertLess(gzip_first_pass.GZPASS_UNLIFTED, set(gzip_first_pass.GZPASS_OUTCOMES))
        self.assertNotIn("reopens", gzip_first_pass.GZPASS_UNLIFTED)

    def test_the_cap_holds_unless_a_file_reopens_it(self):
        v = gzip_first_pass.gzpass_verdict
        holds = "**The physical cap holds**"
        got = v({"a": "shared", "b": "level", "c": "no lift"})
        self.assertIn(holds, got)
        # Each lifted file's outcome is reported, whatever it names.
        self.assertIn("a: shared; b: level", got)
        self.assertNotIn("c: no lift", got)
        # Files naming different costs still hold the cap: only a challenger
        # beating it moves the default.
        self.assertIn(holds, v({"a": "shared", "b": "oversubscription"}))
        got = v({"a": "shared", "b": "reopens", "c": "reopens"})
        self.assertIn("reopens D2's gzip clause", got)
        self.assertIn("on b, c", got)
        self.assertNotIn(holds, got)
        self.assertIn("settles nothing", v({"a": "no lift"}))
        # A regression blocks nothing and is named whatever the rest settle.
        got = v({"a": "shared", "b": "regresses", "c": "no lift"})
        self.assertIn(holds, got)
        self.assertIn("regresses on b", got)
        self.assertNotIn("regresses on b, c", got)
        self.assertIn("regresses on a", v({"a": "regresses"}))
        self.assertIn("settles nothing", v({"a": "regresses"}))
        self.assertNotIn("regresses on", v({"a": "level"}))

    # -- the renderer ------------------------------------------------------

    TIMES = {1: 4.0, 4: 2.5, shapes.GZPASS_PHYSICAL: 1.0, 23: 1.0, 24: 2.0}

    def _raw(self, times=None, chunk=0.8, reported=None):
        """A sitting whose every gzip leg reads `times` at its count, every
        rep alike, and whose every run reports the count it states."""
        rss, readings, reports = {}, {}, {}
        for spec in gzip_first_pass._gzpass_specs():
            key = spec.key(self.FIGURE)
            jobs = int(spec.command.removeprefix(shapes.GZPASS_FAMILY))
            if spec.input == "control":
                value = 0.8
            elif spec.binary == leg_binaries.GZPASS_CHUNK_BINARY:
                value = chunk
            else:
                value = (times or self.TIMES)[jobs]
            readings[key] = [value] * 7
            report = {"resolved_jobs": str(jobs)}
            if spec.input != "control":
                report["first_pass_workers"] = str(jobs)
            report.update((reported or {}).get((spec.input, spec.binary, jobs), {}))
            reports[key] = {k: v for k, v in report.items() if v is not None}
        return {
            "readings": readings,
            "rss": rss,
            "killed": {},
            "reported": reports,
            "input_sizes": {name: 1 << 30 for name in inputs.INPUTS},
            "runs": [],
        }

    def _render(self, raw):
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            return gzip_first_pass.run_gzip_first_pass(session)

    def test_a_logical_count_less_one_level_with_the_physical_holds_the_cap(self):
        body = self._render(self._raw())
        self.assertIn("**The physical cap holds**", body)
        self.assertIn("level 6: oversubscription", body)
        # The header, a row a count, and the chunk leg's.
        rows = [ln for ln in body.splitlines() if ln.startswith("| `")]
        self.assertEqual(len(rows), 1 + len(shapes.GZPASS_JOBS) + 1)
        self.assertIn("4.00× serial", body)
        # The chunk leg, at 0.8 s against the shipped chunk's 1.0 s at the
        # physical count, on every writer's file.
        self.assertIn("**beats** · faster in 7/7 · -0.200 s, -25.0%", body)
        self.assertIn("**The chunk leg replaces the shipped chunk**", body)
        self.assertIn(f"1 MiB against 4 MiB chunks, `{shapes.GZPASS_PHYSICAL}`", body)

    def test_an_upper_count_beating_the_physical_reopens_the_default(self):
        times = {**self.TIMES, shapes.GZPASS_LOGICAL: 0.8}
        body = self._render(self._raw(times))
        self.assertIn("**The figure reopens D2's gzip clause**", body)
        self.assertNotIn("**The physical cap holds**", body)
        self.assertIn("**reopens**", body)

    def test_the_chunk_leg_beaten_anywhere_leaves_the_shipped_chunk(self):
        body = self._render(self._raw(chunk=1.5))
        self.assertIn("**The shipped chunk stands**", body)
        self.assertIn("beats the shipped 4 MiB on 0 of the 3", body)

    def test_the_chunk_verdict_is_d121_s_bar(self):
        v = gzip_first_pass.gzpass_chunk_verdict
        labels = {leg: leg for leg, _, _ in register.GZPASS_WRITERS}
        quick = "control_gz_quick"
        self.assertNotIn(quick, gzip_first_pass.GZPASS_CHUNK_WRITERS)
        self.assertEqual(set(gzip_first_pass.GZPASS_CHUNK_WRITERS) | {quick}, set(labels))

        def marks(**over):
            return {leg: over.get(leg, "level") for leg in labels}

        replaces = "replaces the shipped chunk"
        # Two of the three level-6 files, beaten on none: it ships.
        self.assertIn(replaces, v(marks(control_gz="beats", control_gz_gnu="beats"), labels))
        # One file's gain does not move a default.
        self.assertIn("stands", v(marks(control_gz_gnu="beats"), labels))
        # Beaten anywhere, the level-1 file's included, it does not ship.
        self.assertIn(
            "stands",
            v(marks(control_gz="beats", control_gz_ng="beats", control_gz_gnu="beats",
                    control_gz_quick="beaten"), labels),
        )
        # A win on the level-1 file counts toward nothing.
        self.assertIn("stands", v(marks(control_gz="beats", control_gz_quick="beats"), labels))

    def test_a_count_other_than_the_row_s_refuses_the_figure(self):
        raw = self._raw(reported={("control_gz_gnu", "pgdt", 23): {"resolved_jobs": "20"}})
        with self.assertRaisesRegex(RuntimeError, "resolved 20 worker"):
            self._render(raw)

    def test_a_gzip_leg_without_its_first_pass_refuses_the_figure(self):
        raw = self._raw(
            reported={("control_gz", "pgdt", 4): {"first_pass_workers": None}}
        )
        with self.assertRaisesRegex(RuntimeError, "ran no worker"):
            self._render(raw)

    def test_a_floor_that_opened_a_first_pass_refuses_the_figure(self):
        raw = self._raw(reported={("control", "pgdt", 1): {"first_pass_workers": "1"}})
        with self.assertRaisesRegex(RuntimeError, "a plain dump opened a first pass"):
            self._render(raw)

    def test_unpaired_legs_refuse_the_figure(self):
        raw = self._raw()
        key = gzip_first_pass._gzpass_specs()[1].key(self.FIGURE)
        raw["readings"][key] = raw["readings"][key][:-1]
        with self.assertRaisesRegex(RuntimeError, "do not pair"):
            self._render(raw)

    # -- the generator ---------------------------------------------------

    def test_each_writer_s_header_is_pg_dump_s_at_its_level(self):
        import generate_gzip_input as g

        self.assertEqual(g.header_for(g.LEVEL), g.PG_DUMP_HEADER)
        self.assertEqual(g.header_for(1)[8], 4)
        self.assertEqual(g.header_for(9)[8], 2)
        self.assertEqual(g.header_for(1)[:8], g.PG_DUMP_HEADER[:8])

    def test_another_writer_version_is_refused(self):
        import generate_gzip_input as g

        g.check_zlib_ng(g.ZLIBNG_VERSION)
        g.check_zlib_ng_compat(g.ZLIBNG_COMPAT_VERSION)
        g.check_gnu_gzip(g.GNU_GZIP_VERSION)
        with self.assertRaises(SystemExit):
            g.check_zlib_ng("2.1.6")
        with self.assertRaises(SystemExit):
            g.check_zlib_ng_compat("2.3.3-2.fc44")
        # The image is pinned by digest, so the version it holds cannot move.
        self.assertRegex(g.FEDORA_IMAGE, r"@sha256:[0-9a-f]{64}$")
        with self.assertRaises(SystemExit):
            g.check_gnu_gzip("1.13")

    def test_each_writer_writes_one_member_holding_the_source(self):
        import gzip
        import zlib

        import generate_gzip_input as g

        with tempfile.TemporaryDirectory() as tmp:
            source = Path(tmp) / "plain.sql"
            source.write_bytes(b"COPY public.t (a) FROM stdin;\n1\n\\.\n" * 1000)
            # The version gates are tested above; this is about the bytes.
            # Fedora's library is copied out of an image no test runs; the
            # host's libz stands in, through the same `ctypes` deflate.
            host = ctypes.util.find_library("z")
            writers = [("zlib", 6), ("zlib-ng", 6), ("zlib-ng", 1), ("gzip", 6)]
            if host is not None:
                writers.append(("zlib-ng-compat", 6))
            with (
                unittest.mock.patch.object(g, "gnu_gzip_version", lambda: g.GNU_GZIP_VERSION),
                unittest.mock.patch.object(g.zlib, "ZLIB_RUNTIME_VERSION", g.ZLIB_VERSION),
                unittest.mock.patch.object(g, "load_zlib_ng_compat", lambda: ctypes.CDLL(host)),
            ):
                for writer, level in writers:
                    with self.subTest(writer=writer, level=level):
                        out = Path(tmp) / f"{writer}-{level}.gz"
                        g.compress(source, out, writer, level)
                        data = out.read_bytes()
                        self.assertEqual(data[:10], g.header_for(level))
                        self.assertEqual(gzip.decompress(data), source.read_bytes())
                        inflater = zlib.decompressobj(g.GZIP_WINDOW_BITS)
                        inflater.decompress(data)
                        self.assertTrue(inflater.eof)
                        self.assertEqual(inflater.unused_data, b"")
            # Two writers' streams of one plaintext differ: the figure's
            # columns are different blocks, not one file four times.
            self.assertNotEqual(
                (Path(tmp) / "zlib-6.gz").read_bytes(), (Path(tmp) / "zlib-ng-6.gz").read_bytes()
            )

    @unittest.skipUnless(ctypes.util.find_library("z"), "no libz on this host")
    def test_the_ctypes_deflate_is_python_s_over_the_same_zlib(self):
        # Over a stock zlib of Python's own minor, the `ctypes` deflate writes
        # Python's bytes: it opens `deflate` at `compressobj`'s parameters and
        # feeds it the same chunks, so Fedora's library writes `pg_dump`'s.
        import zlib

        import generate_gzip_input as g

        lib = ctypes.CDLL(ctypes.util.find_library("z"))
        lib.zlibVersion.restype = ctypes.c_char_p
        host = lib.zlibVersion().decode()
        if "ng" in host or host.split(".")[:2] != zlib.ZLIB_RUNTIME_VERSION.split(".")[:2]:
            self.skipTest(f"the host's libz is {host}, Python's {zlib.ZLIB_RUNTIME_VERSION}")
        plain = b"".join(b"%d\tname%d\n" % (i, i * 7919 % 100_003) for i in range(200_000))
        ours = g.LibzDeflater(lib, 6, g.GZIP_WINDOW_BITS)
        theirs = zlib.compressobj(6, zlib.DEFLATED, g.GZIP_WINDOW_BITS)
        cut = len(plain) // 3
        got = ours.compress(plain[:cut]) + ours.compress(plain[cut:]) + ours.flush()
        want = theirs.compress(plain[:cut]) + theirs.compress(plain[cut:]) + theirs.flush()
        self.assertEqual(got, want)

    def test_bgzf_is_zlib_s_alone(self):
        import generate_gzip_input as g

        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            g.main(["--from-dump", "x.sql", "--bgzf", "--writer", "gzip", "out.gz"])


if __name__ == "__main__":
    unittest.main()
