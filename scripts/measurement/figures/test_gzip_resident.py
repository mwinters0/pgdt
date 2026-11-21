"""Tests of `measurement.figures.gzip_resident`."""

from __future__ import annotations

import generate_gzip_input
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import emit
from measurement import formatting
from measurement import gzip_charge
from measurement import inputs
from measurement import memory_model
from measurement import parsing
from measurement import replay
from measurement import shapes
from measurement import staging
from measurement.figures import declaration
from measurement.figures import gzip_resident
from measurement.figures import register


class GzipResident(unittest.TestCase):
    """gzip's resident against its charge: two single members' first passes,
    one whose chunks reach the speculation limit, and a BGZF file's block
    path, each at three worker counts."""

    FIGURE = "gzip-resident"

    #: What each leg's run said it billed, read off `scan started` by hand from
    #: trial runs of `pgdt parse` under `GZPASS_BUDGET`'s allowance: the
    #: first-pass legs on 2026-10-29, at the 4 MiB chunk
    #: (`runs/p15.20/trial-bills.sh`), the BGZF leg's block path on
    #: 2026-10-27 (`runs/p15.18/trial-bills.sh`), which that charge does not
    #: move. The mirror is held to these, the bills a run states, as well as
    #: to the source. Every run resolved its row's count, and each first pass
    #: ran that many workers.
    STATED_BILLS = {
        ("control_gz", 1): 114_115_162,
        ("control_gz", 4): 456_460_648,
        ("control_gz", 24): 2_738_763_888,
        ("repeated_gz", 1): 114_115_162,
        ("repeated_gz", 4): 456_460_648,
        ("repeated_gz", 24): 2_738_763_888,
        ("control_bgzf", 1): 102_987_776,
        ("control_bgzf", 4): 185_168_384,
        ("control_bgzf", 24): 1_237_000_704,
    }
    #: `control_gz`'s stored size on those runs, which decides it has chunks
    #: past its first.
    GZ_SIZE = 791_308_108
    #: Each leg's stored size on those runs, and the plaintext the limit leg
    #: was generated from, `control`'s.
    SIZES = {"control_gz": GZ_SIZE, "repeated_gz": 134_043_544, "control_bgzf": 842_864_994}
    CONTROL_SIZE = 3_221_227_790

    def test_it_is_taken_and_borrows_nothing(self):
        # Standing in no sharing edge is what lets it publish from its own
        # commit, outside a sweep; taken, it is in `FIGURES`, where the
        # doc-side checks apply.
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        self.assertIn(fig, register.FIGURES)
        self.assertNotIn(fig, register.UNTAKEN)
        self.assertEqual(fig.shares, ())
        self.assertEqual(fig.subtracts, ())
        self.assertEqual(fig.stage, "warm-parallel")
        self.assertEqual(fig.warm_inputs, ("control_gz", "repeated_gz", "control_bgzf"))
        self.assertEqual(fig.memory, shapes.GZPASS_MEMORY)

    def test_what_can_move_it_is_declared_and_exists(self):
        fig = register.SELECTABLE_BY_ID[self.FIGURE]
        for path in (
            *register.GZIP_READ,
            *declaration.READ,
            *declaration.RSS_INSTRUMENT,
            "scripts/generate_gzip_input.py",
            "scripts/generate_repeated_rows.py",
            *declaration.GEN_PERF,
            "crates/pgdt/Cargo.toml",
        ):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)
        for path in register.GZIP_READ:
            with self.subTest(path=path):
                self.assertTrue((config.REPO / path).exists(), path)

    def test_the_bgzf_leg_is_a_compression_of_the_control(self):
        spec = inputs.INPUTS["control_bgzf"]
        self.assertEqual(spec.derives_from, "control")
        self.assertEqual(spec.generator, "generate_gzip_input.py")
        self.assertIn("--bgzf", spec.args)
        self.assertEqual(inputs.input_file("control_bgzf"), "control_bgzf.gz")

    def test_the_limit_leg_is_controls_rows_in_one_member(self):
        spec = inputs.INPUTS["repeated_gz"]
        self.assertEqual(spec.derives_from, "control")
        self.assertEqual(spec.generator, "generate_repeated_rows.py")
        self.assertEqual(spec.args, ("--from-dump", "@SOURCE@"))
        self.assertEqual(inputs.input_file("repeated_gz"), "repeated_gz.gz")
        kinds = {leg: kind for leg, _, kind in register.GZIP_RSS_LEGS}
        self.assertEqual(kinds["repeated_gz"], "pass")

    def test_the_limit_leg_must_pack_past_the_limits_ratio_with_room(self):
        # The ratio a chunk inflates to the limit at, and the room past it.
        at = gzip_charge.GZIP_SPECULATION_LIMIT / gzip_charge.GZIP_CHUNK
        need = at * gzip_resident.GZIP_LIMIT_ROOM
        plaintext = 3 << 30
        self.assertIsNone(gzip_resident.gzip_limit_ratio_problem(plaintext, int(plaintext / need) - 1))
        self.assertIn(
            "fills no task's charge",
            gzip_resident.gzip_limit_ratio_problem(plaintext, int(plaintext / need) + 1),
        )
        # A member packing at the limit's own ratio is no falsifier: a chunk
        # looser than the mean would fall short of the limit.
        self.assertIsNotNone(gzip_resident.gzip_limit_ratio_problem(plaintext, int(plaintext / at)))
        self.assertIsNotNone(gzip_resident.gzip_limit_ratio_problem(plaintext, 0))
        self.assertGreater(gzip_resident.GZIP_LIMIT_ROOM, 1)

    def test_a_dry_run_reads_the_limit_leg_as_a_falsifier(self):
        # A dry run sizes an ungenerated input by its nominal, so the limit
        # leg's must clear the ratio check over `control`'s, or every dry run
        # refuses the figure before it names a command.
        cfg = config.Config(dry_run=True)
        plaintext = staging.nominal_size(cfg, inputs.INPUTS["repeated_gz"].derives_from)
        packed = staging.nominal_size(cfg, "repeated_gz")
        self.assertIsNone(gzip_resident.gzip_limit_ratio_problem(plaintext, packed))
        self.assertGreater(packed, self.SIZES["repeated_gz"])

    def test_a_derived_inputs_source_is_sized_for_a_render(self):
        # The limit leg's ratio reads `control`'s length, which no figure
        # stages, so a re-render must find it among the recorded sizes.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(cache_dir=Path(tmp))
            for name, size in (("control", 300), ("repeated_gz", 10)):
                with (Path(tmp) / inputs.input_file(name)).open("wb") as fh:
                    fh.truncate(size)
            sizes = emit.recorded_input_sizes(
                staging.Stager(cfg, lambda _m: None), [register.SELECTABLE_BY_ID[self.FIGURE]]
            )
        self.assertEqual(sizes, {"control": 300, "repeated_gz": 10})

    def test_the_counts_are_parallel_rows_either_side_of_the_depth(self):
        jobs = shapes.GZIP_RSS_JOBS
        self.assertEqual(list(jobs), sorted(set(jobs)))
        self.assertTrue(set(jobs) <= set(shapes.PARALLEL_JOBS))
        # The serial path, a count below the pool's depth and one above it:
        # the block leg's pool is constant on one side and grows on the other.
        self.assertEqual(jobs[0], shapes.PARALLEL_BASELINE)
        self.assertLessEqual(jobs[1], memory_model.LIBRARY_POOL_DEPTH)
        self.assertGreater(jobs[-1], memory_model.LIBRARY_POOL_DEPTH)

    def test_every_leg_is_parallel_peak_rss_s_shape_over_gzip(self):
        specs = gzip_resident._gzip_rss_specs()
        self.assertEqual(len(specs), len(register.GZIP_RSS_LEGS) * len(shapes.GZIP_RSS_JOBS))
        self.assertEqual({s.input for s in specs}, {"control_gz", "repeated_gz", "control_bgzf"})
        self.assertEqual({s.regime for s in specs}, {"warm-parallel"})
        self.assertEqual({s.binary for s in specs}, {"pgdt"})
        for spec in specs:
            with self.subTest(spec=spec.label):
                script = shapes._script(spec.command)
                self.assertIn("/peak-rss /pgdt parse", script)
                self.assertIn(f"--jobs {spec.command.rpartition('-jobs-')[2]} ", script)
                self.assertIn(
                    f"--memory {memory_model.stated_allowance(shapes.GZPASS_BUDGET)}", script
                )
                # `parallel-peak-rss`'s row at that count, but for the budget.
                twin = shapes._script(spec.command.replace(shapes.GZRSS_FAMILY, "parse-rss-jobs-"))
                self.assertEqual(
                    script,
                    twin.replace(
                        str(memory_model.stated_allowance(shapes.PARALLEL_BUDGET)),
                        str(memory_model.stated_allowance(shapes.GZPASS_BUDGET)),
                    ),
                )
        self.assertEqual(len({s.label for s in specs}), len(specs))

    def test_each_shape_is_a_command_shape_and_another_count_is_an_error(self):
        for jobs in shapes.GZIP_RSS_JOBS:
            with self.subTest(jobs=jobs):
                self.assertIn(f"{shapes.GZRSS_FAMILY}{jobs}", shapes.command_shapes())
        for command in ("parse-gzrss-jobs-2", "parse-gzrss-jobs-", "parse-gzrss-jobs-all"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_the_mirrored_constants_are_the_librarys_own(self):
        # Hand-copied on `QUERY_SUBSTREAM_CAP`'s argument, so checked here:
        # a term that moved in the library and not here bills a cell wrongly,
        # which the run's own stated bill then refuses at the sitting.
        repo = config.REPO
        deflate = (repo / "crates/pgdt-io/src/source/compressed/deflate.rs").read_text()
        self.assertIn(
            "const CHECKPOINT_SPACING: NonZeroU64 = NonZeroU64::new("
            f"{gzip_charge.LIBRARY_GZIP_SPACING >> 20} << 20).unwrap();",
            deflate,
        )
        self.assertIn(f"const INPUT_CHUNK: u64 = 1 << {gzip_charge.GZIP_INPUT_CHUNK.bit_length() - 1};",
                      deflate)
        self.assertIn("self.lending_footprint() + self.input_chunk()", deflate)
        self.assertIn("pgdt_gzip_seek::DECODER_BYTES + pgdt_gzip_seek::WINDOW_SIZE as u64", deflate)
        zlib = (repo / "crates/pgdt-gzip-seek/src/zlib.rs").read_text()
        self.assertIn(f"pub const DECODER_BYTES: u64 = {gzip_charge.GZIP_DECODER_BYTES >> 10} * 1024;",
                      zlib)
        self.assertIn(f"pub const WINDOW_SIZE: usize = {gzip_charge.GZIP_WINDOW_BYTES >> 10} * 1024;",
                      zlib)
        speculate = (repo / "crates/pgdt-gzip-seek/src/speculate.rs").read_text()
        self.assertIn("pub const SPECULATION_LIMIT: usize = 2 * ((1 << (9 + 6)) - 1) * 258;",
                      speculate)
        self.assertEqual(gzip_charge.GZIP_SPECULATION_LIMIT, 2 * ((1 << (9 + 6)) - 1) * 258)
        first_pass = repo / "crates/pgdt-io/src/source/compressed/deflate/first_pass"
        pass_rs = (first_pass / "pass.rs").read_text()
        self.assertIn(
            f"const SKIP_SCRATCH: usize = {gzip_charge.GZIP_SKIP_SCRATCH >> 10} * 1024;", pass_rs
        )
        first = (first_pass.parent / "first_pass.rs").read_text()
        self.assertIn(
            "const DECODE_BYTES: u64 =\n"
            "    pgdt_gzip_seek::DECODER_BYTES + pgdt_gzip_seek::WINDOW_SIZE as u64 + INPUT_CHUNK;",
            first,
        )
        self.assertIn("const PASS_BYTES: u64 = 2 * DECODE_BYTES + SKIP_SCRATCH as u64;", first)
        self.assertIn("let per_worker = if ahead { pass.max(task) } else { pass };", first)
        ahead = (first_pass / "ahead.rs").read_text()
        self.assertIn(
            "const CHUNK_BYTES: NonZeroU64 = NonZeroU64::new("
            f"{gzip_charge.GZIP_CHUNK >> 20} << 20).unwrap();",
            ahead,
        )
        self.assertIn(
            "Chunking { bytes: CHUNK_BYTES, limit: pgdt_gzip_seek::SPECULATION_LIMIT, lead: 4 };", ahead
        )
        self.assertIn(f"const DECODES_A_TASK: usize = {gzip_charge.GZIP_DECODES_A_TASK};", ahead)
        self.assertIn(f"const GUESSES_A_TASK: usize = {gzip_charge.GZIP_GUESSES_A_TASK};", ahead)
        for line in (
            "let feed = if chunk > INPUT_CHUNK { chunk } else { INPUT_CHUNK };",
            "let running = if speculation > decode { speculation } else { decode };",
            "let held = decodes * decode + guesses * pgdt_gzip_seek::ChunkTask::GUESS_BYTES;",
            "running + held + INPUT_CHUNK",
        ):
            self.assertIn(line, ahead)
        gz = repo / "crates/pgdt-gzip-seek/src"
        task = (gz / "parallel/task.rs").read_text()
        self.assertIn("pub const GUESS_BYTES: u64 = 3 * WINDOW_SIZE as u64;", task)
        self.assertIn(
            "const EVENTS_A_GUESS: usize = 16 * (SPECULATION_LIMIT / ((1 << (8 + 6)) - 1));",
            speculate,
        )
        self.assertIn(
            f"const STATE_BYTES: u64 = {gzip_charge.GZIP_TASK_STATE_BYTES >> 10} * 1024;", speculate
        )
        self.assertIn(
            "const CHAIN_PAST: u64 = u16::MAX as u64 + 1 + crate::bgzf::BGZF_HEADER_LEN as u64;",
            speculate,
        )
        for line in (
            "let past = if limit > CHAIN_PAST { limit } else { CHAIN_PAST };",
            "let input = chunk_bytes + past + LONGEST_HEADER as u64 + feed + PAD as u64;",
            "let sinks = 3 * (limit + ROOM_PAST_LIMIT as u64);",
            "let guess = 2 * WINDOW_SIZE as u64;",
            "input + sinks + guess + STATE_BYTES",
        ):
            self.assertIn(line, speculate)
        chunk = (gz / "chunk.rs").read_text()
        for line in (
            "const EVENTS_A_GUESSED: usize = EVENTS_A_GUESS + 3;",
            "pub(crate) const PIECES_A_DECODE: usize = EVENTS_A_GUESSED;",
            "pub(crate) const EVENTS_A_DECODE: usize = 2 * EVENTS_A_GUESSED + 1;",
            "let buffers = WINDOW_SIZE + limit + 1 + events + pieces;",
            "buffers as u64 + DECODER_BYTES + STATE_BYTES",
        ):
            self.assertIn(line, chunk)
        native = gz / "native"
        inflate = (native / "inflate.rs").read_text()
        self.assertIn("pub(crate) const LONGEST_HEADER: usize =", inflate)
        self.assertIn("(3 + 5 + 5 + 4 + 19 * 3 + (288 + 32) * 7_usize).div_ceil(8);", inflate)
        self.assertEqual(gzip_charge.GZIP_LONGEST_HEADER, 290)
        self.assertIn(
            f"pub(crate) const PAD: usize = {gzip_charge.GZIP_INPUT_PAD};",
            (native / "input.rs").read_text(),
        )
        sink = (native / "sink.rs").read_text()
        self.assertIn("const LONGEST_COPY: usize = 258;", sink)
        self.assertIn("const SLACK: usize = 32;", sink)
        self.assertEqual(gzip_charge.GZIP_ROOM_PAST_LIMIT, 258 + 32)

    def test_the_charges_are_the_bills_a_run_stated(self):
        sizes = self.SIZES
        kinds = {leg: kind for leg, _, kind in register.GZIP_RSS_LEGS}
        for (leg, jobs), stated in self.STATED_BILLS.items():
            with self.subTest(leg=leg, jobs=jobs):
                billed, pool = gzip_resident.gzip_leg_charge(kinds[leg], jobs, sizes[leg])
                self.assertEqual(billed, stated)
                # A first pass retains no block, so it bills no pool; the BGZF
                # leg's pool is `pool_bytes` over its unit, at every count.
                if kinds[leg] == "pass":
                    self.assertEqual(pool, 0)
                else:
                    self.assertEqual(pool, memory_model.pool_bytes(gzip_charge.bgzf_unit(), jobs))

    def test_a_first_pass_of_one_chunk_bills_the_pass_alone(self):
        # No task can run on a file of one chunk, so none is charged.
        self.assertEqual(
            gzip_charge.gzip_first_pass_charge(4, gzip_charge.GZIP_CHUNK),
            4 * gzip_charge.gzip_pass_bytes(),
        )
        self.assertLess(gzip_charge.gzip_pass_bytes(), gzip_charge.gzip_task_bytes())

    def test_a_bgzf_unit_is_the_fewest_members_reaching_the_spacing(self):
        unit = gzip_charge.bgzf_unit()
        self.assertEqual(unit % generate_gzip_input.BGZF_BLOCK, 0)
        self.assertGreaterEqual(unit, gzip_charge.LIBRARY_GZIP_SPACING)
        self.assertLess(unit - generate_gzip_input.BGZF_BLOCK, gzip_charge.LIBRARY_GZIP_SPACING)

    def test_an_unknown_charge_is_an_error(self):
        with self.assertRaises(ValueError):
            gzip_resident.gzip_leg_charge("walk", 1, 1)

    def test_the_bands_are_the_xz_checks_own(self):
        # One set of lines for every source: `charge_model_problem` is
        # `charge_fault` over `.xz`'s bill.
        unit = 24 << 20
        for held in (1.0, 400 * config.MIB, 900 * config.MIB, 2000 * config.MIB):
            with self.subTest(held=held):
                billed, pool, _ = memory_model.charge_model(unit, 4, held)
                self.assertEqual(
                    memory_model.charge_model_problem(unit, 4, held),
                    memory_model.charge_fault(billed, pool, 4, held),
                )

    # -- the renderer ------------------------------------------------------

    def _raw(self, extra=None, reported=None, input_sizes=None):
        """A sitting whose every cell holds its bill plus `extra` MiB (100 by
        default), each run reporting the bill and count it ran under."""
        sizes = self.SIZES
        kinds = {leg: kind for leg, _, kind in register.GZIP_RSS_LEGS}
        rss, reports = {}, {}
        for spec in gzip_resident._gzip_rss_specs():
            jobs = int(spec.command.rpartition("-jobs-")[2])
            key = spec.key(self.FIGURE)
            # The mirror's bill, which `test_the_charges_are_the_bills_a_run_stated`
            # holds to what runs stated.
            bill, _ = gzip_resident.gzip_leg_charge(kinds[spec.input], jobs, sizes[spec.input])
            held = bill + (extra or {}).get((spec.input, jobs), 100 * config.MIB)
            rss[key] = [held / 1024 - 20, held / 1024 - 10, held / 1024]
            report = {
                "resolved_jobs": str(jobs),
                "resolved_budget": str(bill),
            }
            if kinds[spec.input] == "pass":
                report["first_pass_workers"] = str(jobs)
            report.update((reported or {}).get((spec.input, jobs), {}))
            reports[key] = {k: v for k, v in report.items() if v is not None}
        return {
            "readings": {k: [1.0] * 3 for k in rss},
            "rss": rss,
            "killed": {},
            "reported": reports,
            "input_sizes": {
                **{name: 1 << 30 for name in inputs.INPUTS},
                "control": self.CONTROL_SIZE,
                **sizes,
                **(input_sizes or {}),
            },
            "runs": [],
        }

    def _render(self, raw):
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            return gzip_resident.run_gzip_resident(session)

    def test_cells_inside_the_bound_meet_the_model(self):
        body = self._render(self._raw())
        self.assertIn("The model holds at every cell above", body)
        labels = tuple(f"| {label}, `--jobs " for _, label, _ in register.GZIP_RSS_LEGS)
        rows = [ln for ln in body.splitlines() if ln.startswith(labels)]
        self.assertEqual(len(rows), 9)
        for row in rows:
            self.assertIn("| met |", row)
        # The first pass says how many workers it ran; the BGZF leg pools.
        self.assertIn("| 24 · 24 ran |", body)
        self.assertIn(
            formatting._fmt_budget_bytes(memory_model.pool_bytes(gzip_charge.bgzf_unit(), 24)), body
        )

    def test_a_cell_past_the_bound_is_a_finding_and_re_derives_it(self):
        over = memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND + config.MIB
        body = self._render(self._raw(extra={("control_gz", 24): over}))
        self.assertNotIn("The model is refuted", body)
        self.assertIn("a finding rather than a refutation", body)
        self.assertIn(
            "re-derive `MEMORY_UNPOOLED_BOUND` at "
            f"**{formatting._fmt_budget_bytes(memory_model.rederived_unpooled_bound(over))}**",
            body,
        )

    def test_an_over_bill_refutes_and_names_the_leg(self):
        body = self._render(self._raw(extra={("control_gz", 24): -config.MIB}))
        self.assertIn("The model is refuted, and by these cells:", body)
        self.assertIn("**single member, first pass** at `--jobs 24`", body)
        self.assertIn("over-billed", body)

    def test_the_secant_is_what_each_added_worker_held_and_was_charged(self):
        added = 10 * config.MIB
        extra = {("control_gz", 24): 100 * config.MIB + 20 * added}
        body = self._render(self._raw(extra=extra))
        task = gzip_charge.gzip_task_bytes()
        held_each = task + added
        self.assertIn(
            f"| single member, first pass | {formatting._fmt_budget_bytes(held_each)} | "
            f"{formatting._fmt_budget_bytes(task)} | "
            f"**past it by {formatting._fmt_budget_bytes(added)}** |",
            body,
        )
        # A leg whose workers each held their charge or less is within it.
        self.assertIn(
            f"| single member at the limit, first pass | {formatting._fmt_budget_bytes(task)} | "
            f"{formatting._fmt_budget_bytes(task)} | within it |",
            body,
        )

    def test_the_limit_legs_ratio_is_stated(self):
        body = self._render(self._raw())
        ratio = self.CONTROL_SIZE / self.SIZES["repeated_gz"]
        self.assertIn(f"`repeated_gz`, {formatting._fmt_bytes(self.SIZES['repeated_gz'])} "
                      f"compressed, {ratio:.2f} : 1", body)

    def test_a_limit_leg_packing_short_of_the_limit_refuses_the_figure(self):
        # Packed only as tightly as `control_gz`, its chunks fill no term.
        raw = self._raw(input_sizes={"repeated_gz": self.GZ_SIZE})
        with self.assertRaisesRegex(RuntimeError, "cannot read its limit leg"):
            self._render(raw)

    def test_a_bill_the_mirror_does_not_charge_refuses_the_figure(self):
        raw = self._raw(reported={("control_bgzf", 4): {"resolved_budget": "1"}})
        with self.assertRaisesRegex(RuntimeError, "the mirror is not the library's charge"):
            self._render(raw)

    def test_a_count_other_than_the_row_s_refuses_the_figure(self):
        raw = self._raw(reported={("control_gz", 24): {"resolved_jobs": "3"}})
        with self.assertRaisesRegex(RuntimeError, "resolved 3 worker"):
            self._render(raw)

    def test_a_single_member_that_opened_no_first_pass_refuses_the_figure(self):
        raw = self._raw(reported={("control_gz", 4): {"first_pass_workers": None}})
        with self.assertRaisesRegex(RuntimeError, "opened no first pass"):
            self._render(raw)

    def test_a_bgzf_file_read_by_a_first_pass_refuses_the_figure(self):
        raw = self._raw(reported={("control_bgzf", 1): {"first_pass_workers": "1"}})
        with self.assertRaisesRegex(RuntimeError, "not walked"):
            self._render(raw)

    # -- the started line ---------------------------------------------------

    def test_the_first_pass_line_gives_its_workers(self):
        serial = (
            "INFO first pass started, reading serially while it builds the index "
            "source=/dump.sql codec=gzip"
        )
        parallel = (
            "INFO first pass started, inflating in parallel while it builds the index, its "
            "scan one reader source=/dump.sql codec=gzip workers=24"
        )
        resumed = (
            "INFO first pass started, resuming from a saved checkpoint and inflating in "
            "parallel while it builds the index, its scan one reader source=/dump.sql "
            "codec=gzip checkpoint=3 plaintext=75497472 workers=4"
        )
        self.assertEqual(parsing.parse_first_pass(serial), {"first_pass_workers": "1"})
        self.assertEqual(parsing.parse_first_pass(parallel), {"first_pass_workers": "24"})
        self.assertEqual(parsing.parse_first_pass(resumed), {"first_pass_workers": "4"})
        # A `workers=` elsewhere in the log is not the pass's.
        self.assertEqual(
            parsing.parse_first_pass("INFO scan arrangement workers=7\n" + serial),
            {"first_pass_workers": "1"},
        )
        self.assertEqual(parsing.parse_first_pass("INFO scan started jobs=4"), {})

    def test_the_library_spells_the_line_the_harness_reads(self):
        source = (
            config.REPO / "crates/pgdt-io/src/source/compressed/deflate/first_pass.rs"
        ).read_text()
        self.assertEqual(source.count('"first pass started, '), 4)
        self.assertIn("reading serially", source)
        self.assertIn("workers,\n", source)

    # -- the generator ------------------------------------------------------

    def _bgzf(self, plaintext: bytes) -> bytes:
        import generate_gzip_input

        with tempfile.TemporaryDirectory() as tmp:
            source, out = Path(tmp) / "plain.sql", Path(tmp) / "out.gz"
            source.write_bytes(plaintext)
            with unittest.mock.patch.object(
                generate_gzip_input.zlib, "ZLIB_RUNTIME_VERSION", generate_gzip_input.ZLIB_VERSION
            ):
                generate_gzip_input.compress_bgzf(source, out)
            self.assertFalse((Path(tmp) / "out.gz.partial").exists())
            return out.read_bytes()

    def test_it_writes_bgzf_members_holding_the_source(self):
        import gzip
        import struct

        import generate_gzip_input

        block = generate_gzip_input.BGZF_BLOCK
        plaintext = b"COPY public.t (a) FROM stdin;\n1\n\\.\n" * 5000
        data = self._bgzf(plaintext)
        self.assertEqual(gzip.decompress(data), plaintext)
        self.assertTrue(data.endswith(generate_gzip_input.BGZF_EOF))
        # Walk the members by their own `BSIZE`, as `pgdt`'s walk does.
        at, sizes = 0, []
        while at < len(data):
            header = data[at : at + len(generate_gzip_input.BGZF_HEADER)]
            self.assertEqual(header, generate_gzip_input.BGZF_HEADER)
            (bsize,) = struct.unpack_from("<H", data, at + len(header))
            (isize,) = struct.unpack_from("<I", data, at + bsize + 1 - 4)
            sizes.append(isize)
            at += bsize + 1
        self.assertEqual(at, len(data))
        # Every member but the last two full; then the remainder, then EOF.
        self.assertEqual(sizes[-1], 0)
        self.assertEqual(sum(sizes), len(plaintext))
        self.assertTrue(all(size == block for size in sizes[:-2]))
        self.assertEqual(sizes[-2], len(plaintext) - block * (len(sizes) - 2))

    def test_a_member_past_bsize_is_refused(self):
        import random

        import generate_gzip_input

        noise = random.Random(7).randbytes(70_000)
        with self.assertRaises(SystemExit):
            generate_gzip_input.bgzf_member(noise)

    def test_the_eof_member_is_bgzf_s_own(self):
        import gzip

        import generate_gzip_input

        eof = generate_gzip_input.BGZF_EOF
        self.assertEqual(len(eof), 28)
        self.assertTrue(eof.startswith(generate_gzip_input.BGZF_HEADER))
        self.assertEqual(gzip.decompress(eof), b"")


if __name__ == "__main__":
    unittest.main()
