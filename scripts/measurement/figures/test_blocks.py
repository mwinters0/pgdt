"""Tests of `measurement.figures.blocks`."""

from __future__ import annotations

import contextlib
import platform
import tempfile
import tomllib
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import host
from measurement import inputs
from measurement import parsing
from measurement import runspec
from measurement import shapes
from measurement import staging
from measurement import test_support
from measurement import timing
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import runners
from measurement.figures import blocks
from measurement.figures import declaration
from measurement.figures import register


class PeakRss(unittest.TestCase):
    """The one figure whose reading is not a time.

    Two things decide whether the number means what the table says. The
    *instrument* must report pgdt's peak and not the wrapper's or the client's
    — every wrong answer here is a plausible-looking one, which is why the
    wrapper is asserted rather than remembered. And the *rows* must vary one
    thing each: the byte pair differs in bytes alone, the block rows in blocks
    alone, and both are read against the same pivot."""

    def _rows(self) -> tuple[str, ...]:
        return register._RSS_ROWS

    def test_the_command_is_the_same_parse_the_throughput_tables_time(self):
        # A wrapped `parse`, not a different command: what this figure reports
        # has to be the resident set of the scan the rest of the doc measures.
        script = shapes._script("parse-rss")
        self.assertIn("/pgdt parse --source /dump.sql --cache /tmp/x.pgdt", script)

    def test_the_redirection_takes_pgdt_s_stdout_and_not_the_reading(self):
        # The reading goes to stderr, where bash's own `time` report goes, so
        # `>/dev/null` on the whole command cannot swallow it.
        script = shapes._script("parse-rss")
        self.assertTrue(script.rstrip().endswith(">/dev/null"))
        self.assertIn(f"time {parsing.PEAK_RSS} /pgdt parse", script)

    # What the wrapper reads, how it exits and that the reading is the child's
    # are `peak-rss`'s own tests (`crates/peak-rss/tests/peak_rss.rs`); what is held
    # here is that the harness builds that crate and mounts it.

    def test_the_wrapper_is_the_workspace_crate(self):
        cargo = tomllib.loads((config.REPO / "Cargo.toml").read_text())
        self.assertIn("crates/*", cargo["workspace"]["members"])
        self.assertTrue((config.REPO / "crates" / "peak-rss" / "Cargo.toml").is_file())
        self.assertEqual(parsing.PEAK_RSS, "/peak-rss")
        self.assertFalse(test_support.harness_has("rss_wrapper"))
        self.assertFalse(test_support.harness_has("GETRUSAGE_SYSCALL"))

    def test_the_wrapper_is_built_static_and_beside_the_shipped_binary(self):
        # musl, so it links nothing of the host's libc or the image's; under
        # its own target's directory, never `target/release/`.
        target = f"{platform.machine()}-unknown-linux-musl"
        self.assertEqual(image.peak_rss_target(), target)
        self.assertEqual(
            image.peak_rss_path(), config.REPO / "target" / target / "release" / "peak-rss"
        )
        asked = []
        with tempfile.TemporaryDirectory() as sysroot:
            (Path(sysroot) / "lib" / "rustlib" / target).mkdir(parents=True)

            def fake(argv, **_kwargs):
                asked.append(list(argv))
                return sysroot + "\n"

            with unittest.mock.patch.object(host, "run", fake), \
                    unittest.mock.patch.object(image, "_PEAK_RSS_BUILT", False):
                image.ensure_peak_rss_binary(config.Config(), lambda _m: None)
                # Once a process: a second call builds nothing.
                image.ensure_peak_rss_binary(config.Config(), lambda _m: None)
        builds = [argv for argv in asked if argv[:2] == ["cargo", "build"]]
        self.assertEqual(
            builds, [["cargo", "build", "--release", "-p", "peak-rss", "--target", target]]
        )

    def test_a_missing_target_is_refused_with_the_command_that_adds_it(self):
        with tempfile.TemporaryDirectory() as sysroot:
            with unittest.mock.patch.object(host, "run", return_value=sysroot + "\n"), \
                    unittest.mock.patch.object(image, "_PEAK_RSS_BUILT", False):
                with self.assertRaises(RuntimeError) as raised:
                    image.ensure_peak_rss_binary(config.Config(), lambda _m: None)
        self.assertIn(f"rustup target add {image.peak_rss_target()}", str(raised.exception))

    def test_every_figure_running_it_declares_it(self):
        # A change to what the instrument reads moves every reading taken
        # through it, so `--stale` must say so of exactly those figures. What a
        # figure runs is read off a dry run of it; rendering fake readings may
        # fail after its sweeps, which is past what is asked here.
        cfg = config.Config(dry_run=True)
        running = set()
        for fig in register.FIGURES:
            session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None)
            session.input_path = lambda name, regime: Path("/dev/null")
            session.figure_id = fig.id
            commands: set[str] = set()
            take = session.time_run

            def record(spec, take=take, commands=commands):
                commands.add(spec.command)
                return take(spec)

            session.time_run = record
            with contextlib.suppress(Exception):
                runners.RUNS[fig.id](session)
            if any(parsing.PEAK_RSS in shapes._script(c).split() for c in commands):
                running.add(fig.id)
        declaring = {
            fig.id for fig in register.FIGURES if set(declaration.RSS_INSTRUMENT) <= set(fig.depends)
        }
        self.assertEqual(declaring, running)
        self.assertIn("peak-rss", running)

    def test_a_resident_shape_mounts_it_and_a_timed_one_runs_without(self):
        cfg = config.Config(dry_run=True)
        logged: list[str] = []
        session = timing.Session(cfg, staging.Stager(cfg, lambda _m: None), logged.append)
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        session.figure_id = "peak-rss"
        session.time_run(runspec.RunSpec("pgdt", "control", "parse-rss", "warm", ""))
        session.time_run(runspec.RunSpec("pgdt", "control", "parse", "warm", ""))
        runs = [line for line in logged if " run --rm " in line]
        self.assertEqual(len(runs), 2)
        mount = f"{parsing.PEAK_RSS}:ro"
        self.assertIn(mount, runs[0])
        self.assertNotIn(mount, runs[1])

    def test_the_reading_is_read_back_off_the_wrapper_s_own_line(self):
        self.assertEqual(parsing.parse_maxrss_kib("real\t0m0.5s\nmaxrss_kib=6144\n"), 6144)

    def test_no_reading_is_an_error_not_a_zero(self):
        with self.assertRaises(ValueError):
            parsing.parse_maxrss_kib("real\t0m0.5s\n")

    def test_two_readings_are_an_error(self):
        with self.assertRaises(ValueError):
            parsing.parse_maxrss_kib("maxrss_kib=6144\nmaxrss_kib=6200\n")

    def test_block_counts_are_read_off_the_generator(self):
        # Written beside the row instead, a block count that moved in the
        # generator would leave a stale number in the table's own column.
        self.assertEqual(blocks.input_block_count("one_block"), 1)
        self.assertEqual(blocks.input_block_count("control"), 1)
        self.assertEqual(blocks.input_block_count("blocks4000"), 4000)

    def test_the_pivot_is_a_row_and_holds_one_block(self):
        self.assertIn(register._RSS_PIVOT, self._rows())
        self.assertEqual(blocks.input_block_count(register._RSS_PIVOT), 1)

    def test_the_byte_pair_differs_in_bytes_alone(self):
        # `one_block` and `control` are the same generator, same seed, same
        # shape; only `--size-mb` differs. So their difference is bytes.
        pivot, big = inputs.INPUTS[register._RSS_PIVOT], inputs.INPUTS["control"]
        self.assertEqual(pivot.generator, big.generator)
        self.assertEqual(blocks.input_block_count("control"), 1)
        self.assertIn("--seed", pivot.args)
        self.assertEqual(
            pivot.args[pivot.args.index("--seed") + 1],
            big.args[big.args.index("--seed") + 1],
        )

    def test_the_block_rows_multiply_blocks_against_the_pivot(self):
        # koji cannot test this half at all — 74 blocks over 784 GB — which is
        # why the claim was re-homed here.
        counts = [blocks.input_block_count(n) for n in self._rows()]
        self.assertGreater(max(counts), 1000 * blocks.input_block_count(register._RSS_PIVOT))

    def test_every_row_is_an_input_the_figure_stages(self):
        fig = register.SELECTABLE_BY_ID["peak-rss"]
        self.assertEqual(tuple(fig.warm_inputs), self._rows())

    def test_the_figure_declares_the_read_path_first(self):
        # The mechanism the claim is about, and the edge whose absence let the
        # koji row go a megabyte wrong for a whole slice.
        fig = register.SELECTABLE_BY_ID["peak-rss"]
        for path in (*declaration.READ, *declaration.MAP_BUILD, *declaration.CACHE):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)


class RssAttribution(unittest.TestCase):
    """The nine legs that say what `peak-rss`'s per-block growth is made of.

    Every failure this class covers returns a plausible-looking table of
    something else, which is the family the allocator legs already have tests
    for. Two matter most. A leg that is not
    *distinguishable* as a reading silently becomes another leg's number, since
    `RunSpec.key` carries the binary, the input, the shape and the regime and
    not the words the table prints. And a leg whose shape carries no RSS
    wrapper has no reading at all, which surfaces as a `KeyError` a sitting
    into rather than as a refusal before it.
    """

    def _fig(self) -> declaration.Figure:
        return register.EVERY_BY_ID["rss-attribution"]

    def test_every_leg_is_its_own_reading(self):
        # Two legs differing only in their label would share one key and
        # publish one measurement as two rows.
        keys = [
            spec.key("rss-attribution")
            for _, small, big in register._attribution_specs()
            for spec in (small, big)
        ]
        self.assertEqual(len(keys), len(set(keys)))
        self.assertEqual(len(keys), 2 * len(register._ATTRIBUTION_LEGS))

    def test_every_leg_reports_a_resident_set(self):
        # `Session.time_run` keys the RSS capture off `"rss" in spec.command`,
        # so a shape not spelled that way is timed and never measured.
        for _, _, command in register._ATTRIBUTION_LEGS:
            with self.subTest(command=command):
                self.assertIn("rss", command)
                self.assertIn(f" {parsing.PEAK_RSS} /pgdt ", shapes._script(command))

    def test_each_shape_wraps_and_times_exactly_one_command(self):
        # `parse_bash_time` and `parse_maxrss_kib` both refuse two reports, and
        # the `info` leg builds its cache in the same shell — untimed and
        # unwrapped, or the reading would be the builder's.
        for _, _, command in register._ATTRIBUTION_LEGS:
            with self.subTest(command=command):
                script = shapes._script(command)
                self.assertEqual(script.count("time "), 1)
                self.assertEqual(script.split().count(parsing.PEAK_RSS), 1)

    def test_the_two_block_counts_differ_in_blocks_alone(self):
        # Both are `generate_block_count_bench.py` outputs at one seed, so a
        # slope over the pair is a slope in blocks and not in anything else.
        small, big = (inputs.INPUTS[n] for n in register._ATTRIBUTION_INPUTS)
        self.assertEqual(small.generator, big.generator)
        self.assertLess(
            blocks.input_block_count(register._ATTRIBUTION_INPUTS[0]),
            blocks.input_block_count(register._ATTRIBUTION_INPUTS[1]),
        )

    def test_the_reference_leg_is_the_shape_peak_rss_times(self):
        # The first row is read against `peak-rss`'s block-count rows, which
        # only holds while the two run the same command.
        self.assertEqual(register._ATTRIBUTION_LEGS[0][1:], ("pgdt", "parse-rss"))
        self.assertEqual(
            [s.command for s in test_support._peak_rss_specs()][0], register._ATTRIBUTION_LEGS[0][2]
        )

    def test_the_allocator_legs_are_the_allocator_figure_s_own(self):
        # Borrowed by name, never built by a second recipe: `binary_path`
        # routes an `alloc:` binary through `ensure_allocator_binary`, which is
        # where the leg is built and then interrogated.
        legs = {b.removeprefix("alloc:") for _, b, _ in register._ATTRIBUTION_LEGS if ":" in b}
        self.assertEqual(legs, set(leg_binaries.ALLOCATOR_LEGS) - {leg_binaries.ALLOCATOR_LEGS[0]})

    def test_it_declares_the_two_mechanisms_only_its_own_legs_reach(self):
        # The preamble, where the per-table structure is paid, and the CLI's
        # `query` path, which the four no-match legs run. Neither is in
        # `peak-rss`'s declaration, and a figure that cannot say what
        # invalidates it is one nobody has thought about.
        fig = self._fig()
        for path in (*declaration.PREAMBLE, *declaration.QUERY_CLI):
            with self.subTest(path=path):
                self.assertIn(path, fig.depends)

    def test_the_manual_s_per_table_claim_is_this_figure_s_consumer(self):
        # `peak-rss` cannot license it: `blocks4000` gives every table exactly
        # one `COPY` block, so per-table and per-block coincide in its inputs.
        self.assertIn("docs/manual/dump-inspection.md", self._fig().also_quoted_by)

    def test_a_taken_attribution_declares_its_borrow(self):
        """A taken attribution borrows its reference row from `peak-rss`.

        Its `parse` reference row runs `peak-rss`'s `blocks500` and
        `blocks4000` shapes, so the two share a reading rather than take one
        each — two readings of one run put two numbers in the doc a section
        apart. An entry in `UNTAKEN` declares no edge, since the edge would
        entangle `peak-rss` with a figure no sweep takes; a taken one declares
        it, and that is what this asserts rather than leaves to a comment."""
        fig = self._fig()
        if fig in register.UNTAKEN:
            self.assertEqual(fig.shares, ())
        else:
            self.assertIn("peak-rss", [s.source for s in fig.shares])


if __name__ == "__main__":
    unittest.main()
