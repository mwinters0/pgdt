"""Tests of `measurement.figures.reserve`."""

from __future__ import annotations

import dataclasses
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import emit
from measurement import formatting
from measurement import host
from measurement import inputs
from measurement import memory_model
from measurement import parsing
from measurement import replay
from measurement import shapes
from measurement import staging
from measurement import test_support
from measurement import timing
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import register
from measurement.figures import reserve


class Reserve(unittest.TestCase):
    """The instrument behind the budget rule's one number.

    Every assertion here is a way to get a plausible table of the wrong thing,
    which is the family this module already covers for the `--jobs` axis. Four
    matter most and each fails silently. An **arena setting that never
    reached the process** — written where the wrapper takes it as an argument
    rather than as its environment — gives two legs that agree, which reads as
    "the cap buys nothing" rather than as an instrument that set nothing. A
    **budget the shape does not carry** would run at whatever was typed and be
    read as the row it is labelled with. A **worker count drifting onto
    `SWEEP_JOBS`** would read the stated axis at a count the flagless `.xz`
    default never recommends on this machine. And a **`Shared` edge missing
    from the taken figure** leaves two readings of `peak-rss`'s `control` run in
    the doc a section apart.
    """

    def _fig(self):
        return register.SELECTABLE_BY_ID["reserve"]

    def _shape(self, token="unset", budget=None):
        return f"{shapes.RESERVE_FAMILY}{token}-{budget or shapes.RESERVE_BUDGETS[0]}"

    def test_every_registered_leg_has_a_shape(self):
        shapes_ = set(shapes.command_shapes())
        for token, _, _ in shapes.RESERVE_ARENAS:
            for budget in shapes.RESERVE_BUDGETS:
                with self.subTest(arena=token, budget=budget):
                    self.assertIn(self._shape(token, budget), shapes_)

    def test_the_arena_setting_is_the_processs_environment(self):
        # `peak-rss` starts pgdt in its own environment, so an assignment in
        # front of it is inherited by pgdt. In front of `/pgdt` it would be a
        # further argument to `peak-rss` and would set nothing at all.
        for token, value, _ in shapes.RESERVE_ARENAS:
            with self.subTest(arena=token):
                script = shapes._script(self._shape(token))
                if not value:
                    self.assertNotIn("MALLOC_ARENA_MAX", script)
                    continue
                self.assertIn(f"time MALLOC_ARENA_MAX={value} {parsing.PEAK_RSS} /pgdt", script)

    def test_one_leg_sets_nothing(self):
        # The shipped default has to survive the operator who followed no
        # recommendation, so an uncapped leg is not optional decoration.
        unset = [token for token, value, _ in shapes.RESERVE_ARENAS if not value]
        self.assertEqual(unset, ["unset"])

    def test_no_leg_caps_at_or_above_the_worker_count(self):
        # A cap at or above the arena count cannot bind, and a leg pricing one
        # reads as "the cap buys nothing". The arena count is `readers + 2`
        # from two readers up, and 2 at one, rather than the worker count, so this bound is looser than inertness
        # — a `readers + 1` cap is below it and unpriced — but a cap at the
        # worker count or above is a leg no sitting needs.
        for token, value, _ in shapes.RESERVE_ARENAS:
            if value:
                with self.subTest(arena=token):
                    self.assertLess(int(value), shapes.RESERVE_JOBS)

    def test_the_capped_leg_is_the_tightest_an_operator_would_set(self):
        # The pair is uncapped against the tightest plausible cap, and both are
        # load-bearing: a one-leg figure cannot report a null.
        capped = [value for _, value, _ in shapes.RESERVE_ARENAS if value]
        self.assertEqual(capped, ["2"])
        self.assertEqual(len(shapes.RESERVE_ARENAS), 2)

    def test_a_budget_or_arena_the_figure_does_not_carry_is_an_error(self):
        # Parsed rather than matched, so an unregistered value has to be
        # refused explicitly or it runs and is read as a row of the table.
        with self.assertRaises(ValueError):
            shapes._script(self._shape(budget=99))
        with self.assertRaises(ValueError):
            shapes._script(self._shape(token="sixteen"))

    def test_the_axis_starts_at_the_librarys_own_default(self):
        # The smallest budget is the library's own default, which is also the
        # number a run that states nothing gets.
        self.assertEqual(shapes.RESERVE_BUDGETS[0], memory_model.LIBRARY_DEFAULT_BUDGET)
        self.assertEqual(sorted(shapes.RESERVE_BUDGETS), list(shapes.RESERVE_BUDGETS))

    def test_every_budget_is_a_whole_mebibyte(self):
        for budget in shapes.RESERVE_BUDGETS:
            with self.subTest(budget=budget):
                self.assertEqual(reserve._fmt_budget(budget), f"{budget >> 20} MiB")

    def test_every_shape_states_the_figures_own_worker_count(self):
        for token, _, _ in shapes.RESERVE_ARENAS:
            with self.subTest(arena=token):
                script = shapes._script(self._shape(token))
                self.assertIn(f"--jobs {shapes.RESERVE_JOBS} ", script)
                self.assertEqual(script.count("time "), 1)
        self.assertNotEqual(shapes.RESERVE_JOBS, shapes.SWEEP_JOBS)
        self.assertEqual(shapes.RESERVE_JOBS, shapes.PARALLEL_JOBS[-1])

    def test_the_family_is_a_declared_axis_rather_than_an_unpinned_shape(self):
        # Both halves of the worker-count reconciliation: the family states a
        # count, and it is exempt from `SWEEP_JOBS` because it declares itself.
        self.assertEqual(shapes.worker_count_problems(), [])
        self.assertEqual(shapes.pinned_count_problems(), [])

    def test_the_shape_name_says_it_carries_a_resident_reading(self):
        # `time_run` reads the wrapper's report only for a shape whose name
        # contains `rss`; without it every reading here would come back `None`
        # and the table would fail on a missing key rather than on a wrong one.
        self.assertIn("rss", shapes.RESERVE_FAMILY)

    def test_both_sources_are_read(self):
        # A reserve that is a property of the source is not a constant, so the
        # loosest shape is read beside the tightest.
        self.assertEqual([n for n, _ in shapes.RESERVE_INPUTS], ["control_xz", "control"])

    def test_the_serial_baseline_is_peak_rsss_own_row(self):
        # Spec for spec, which is what makes it the borrow this figure declares
        # when it is published rather than a second reading of one measurement.
        base = register._RESERVE_BASELINE
        # By key, which is what identifies a reading: the label is what the
        # table prints and is free to differ between two figures naming one run.
        keys = [s.key("peak-rss") for s in test_support._peak_rss_specs()]
        self.assertIn(base.key("peak-rss"), keys)
        self.assertEqual(base.input, "control")
        self.assertIn("control", register._RSS_ROWS)

    def test_it_declares_no_share_while_it_is_untaken(self):
        """The same obligation `rss-attribution` carries, and for the same reason.

        An entry in `UNTAKEN` declares no edge, since the edge would entangle
        `peak-rss` with a figure no sweep takes; a taken one borrows the
        `control` run, which `Session.borrow` carries with its resident set."""
        fig = self._fig()
        if fig in register.UNTAKEN:
            self.assertEqual(fig.shares, ())
        else:
            self.assertIn("peak-rss", [s.source for s in fig.shares])

    def test_the_baselines_input_is_staged(self):
        # It is one of the two already; naming it anyway is what survives the
        # day it is not.
        self.assertIn(register._RESERVE_BASELINE.input, self._fig().warm_inputs)
        self.assertEqual(
            len(self._fig().warm_inputs), len(set(self._fig().warm_inputs))
        )


class InstrumentReport(unittest.TestCase):
    """The introspection build's own report, out of the file it writes.

    The transport is a file rather than a block on a shared stream, so the
    parse is `parse_reported`'s and the questions here are about the channel:
    that the variable naming it matches the binary's, that a leg declaring the
    instrument fails loudly when no report arrives, and that the readings are
    filed per rep rather than collapsed to the last one.
    """

    #: A report shaped as the instrument writes one: the three scope keys, the
    #: prose note before them, each allocator's totals, and mimalloc's JSON and
    #: glibc's XML verbatim beneath them.
    REPORT = (
        "instrument=counting-allocator\n"
        "live_scope=rust-global-alloc\n"
        "live_bytes=76876\n"
        "live_peak_bytes=209822121\n"
        "# `live_*` counts only what passed through Rust's `GlobalAlloc`, and\n"
        "# cannot see. The process has two heaps; neither family is all of it.\n"
        "mimalloc_scope=rust-heap\n"
        "mimalloc_committed_peak_bytes=268435456\n"
        "# mi_stats_json\n"
        '  "committed": { "total": 9, "peak": 268435456, "current": 1 },\n'
        "# end mi_stats_json\n"
        "glibc_scope=c-malloc\n"
        "mallinfo_arena=131768320\n"
        "malloc_system_max=404201472\n"
        "# malloc_info\n"
        '<system type="max" size="135168"/>\n'
        "# end malloc_info\n"
    )

    def test_it_reads_the_keys_and_ignores_the_prose(self):
        got = parsing.parse_reported(self.REPORT)
        self.assertEqual(got["live_peak_bytes"], "209822121")
        self.assertEqual(got["malloc_system_max"], "404201472")
        # The scope labels are part of the report, not commentary on it: a
        # consumer differencing one heap's count against another's is what
        # they exist to make visible.
        self.assertEqual(got["live_scope"], "rust-global-alloc")
        self.assertEqual(got["mimalloc_scope"], "rust-heap")
        self.assertEqual(got["glibc_scope"], "c-malloc")
        self.assertEqual(got["mimalloc_committed_peak_bytes"], "268435456")
        # The verbatim documents are detail, not readings.
        self.assertNotIn("committed", got)

    def test_the_harnesss_own_reading_cannot_reach_it(self):
        # `maxrss_kib` is `peak-rss`'s, and it is a `key=value` line by this
        # same grammar. The file is why it cannot land here: one writer by
        # construction, where the bracketed stderr block it replaced was a
        # framing protocol over a stream two processes wrote to.
        self.assertNotIn("maxrss_kib", parsing.parse_reported(self.REPORT))

    def test_the_variable_matches_the_binarys(self):
        # Two constants in two languages. A rename on one side that missed the
        # other would leave the instrument writing nowhere the harness looks —
        # which is now an error rather than an empty dict, but only because the
        # name is right.
        source = (config.REPO / "crates/pgdt/src/introspect.rs").read_text()
        self.assertIn(f'pub const OUT_VAR: &str = "{parsing.INSTRUMENT_OUT_VAR}";', source)
        # `datafusion-cli-pgdump`'s section, which `pgdt sql` writes, names it too.
        source = (config.REPO / "crates/datafusion-cli-pgdump/src/pgdump.rs").read_text()
        self.assertIn(
            f'const INTROSPECT_OUT_VAR: &str = "{parsing.INSTRUMENT_OUT_VAR}";', source
        )

    def test_no_leg_that_does_not_declare_it_is_given_the_variable(self):
        # An env var leaves the command shape identical, which is the whole
        # reason it is not a flag — but it must still reach only the legs that
        # asked for it, or a default build's runs acquire a mount for nothing.
        self.assertFalse(any(s.instrument for s in reserve._reserve_flagless_specs()))

    def test_a_declaring_leg_is_pointed_at_the_instrument_build_and_nothing_else_is(self):
        # A leg that declares the instrument and runs the default binary
        # produces no report, which `_read_instrument` can only report as "one
        # of three apparatus faults" — and the pairing is the one of the three
        # that can be checked before the sitting starts.
        for spec in [
            *reserve._reserve_instrument_specs(),
            *reserve._reserve_diagnostic_specs(),
        ]:
            with self.subTest(leg=spec.label):
                self.assertEqual(spec.instrument, spec.binary == "introspect")
        for spec in [
            *reserve._reserve_flagless_specs(),
            *(s for _, s in reserve._reserve_mechanism_specs()),
            *reserve._reserve_step_specs(),
            *reserve._reserve_specs(),
        ]:
            with self.subTest(leg=spec.label):
                self.assertNotEqual(spec.binary, "introspect")
                self.assertFalse(spec.instrument)

    def test_a_build_that_names_no_instrument_is_refused(self):
        # The mirror of `binary_allocator`'s refusal, pointed the other way:
        # there an instrumented build must not be timed, here a leg that
        # declares the instrument must carry one.
        with unittest.mock.patch.object(
            host, "run", return_value="pgdt 0.1.0 (allocator: system)\n"
        ):
            with self.assertRaises(RuntimeError) as caught:
                leg_binaries.binary_instrument(Path("/pgdt"))
        self.assertIn("--features introspect", str(caught.exception))

    def test_the_instrument_build_is_read_back_and_names_itself(self):
        with unittest.mock.patch.object(
            host,
            "run",
            return_value="pgdt 0.1.0 (allocator: system) (instrument: counting-allocator)\n",
        ):
            self.assertEqual(
                leg_binaries.binary_instrument(Path("/pgdt")), "counting-allocator"
            )

    def test_the_dictionary_term_is_inside_the_reader_charge(self):
        # The one term an attribution adds back by hand: `liblzma` allocates it
        # through C `malloc`, so the counting allocator cannot see it and glibc
        # cannot separate it. It is part of what `reader_bytes` already charges,
        # never a term beside it — a decomposition that added it twice would
        # under-report retention by 8 MiB a reader.
        self.assertLess(memory_model.XZ_DICT_BYTES, memory_model.XZ_DECODE_FOOTPRINT)
        self.assertEqual(memory_model.XZ_DICT_BYTES, 8 << 20)
        # What is left of the footprint once the dictionary comes out is
        # `xz_seek`'s 1 MiB input chunk plus `liblzma`'s own 34,592 B of state.
        self.assertEqual(
            memory_model.XZ_DECODE_FOOTPRINT - memory_model.XZ_DICT_BYTES,
            memory_model.LIBRARY_CHUNK_BYTES + 34_592,
        )

    def _session(self, out_root):
        cfg = config.Config()
        session = timing.Session(
            cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None, out_root
        )
        # A unit test builds no `peak-rss`: a fresh checkout has none to stage.
        self.enterContext(
            unittest.mock.patch.object(image, "ensure_peak_rss_binary", return_value=Path("/dev/null"))
        )
        session.figure_id = "reserve"
        session.input_path = lambda name, regime: Path("/dev/null")
        session.binary_path = lambda which: Path("/dev/null")
        return session

    TIMED = "maxrss_kib=68228\n\nreal\t0m0.012s\nuser\t0m0.008s\nsys\t0m0.004s\n"
    NO_KILL = "low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\noom_group_kill 0\n"

    def _instrumented(self):
        spec = dataclasses.replace(reserve._reserve_flagless_specs()[0], instrument=True)
        return spec

    def _run(self, session, spec, write_report: bool):
        """One fake run, optionally writing the report the container would."""

        def fake_run(argv, **_kwargs):
            if write_report:
                out = [a for a in argv if a.startswith(f"{parsing.INSTRUMENT_OUT_VAR}=")]
                self.assertEqual(len(out), 1, argv)
                name = out[0].split("=", 1)[1].rpartition("/")[2]
                (session.out_root / parsing.INSTRUMENT_DIR / name).write_text(self.REPORT)
            return subprocess.CompletedProcess(
                argv, 0, stdout="", stderr=self.TIMED + self.NO_KILL
            )

        with unittest.mock.patch.object(subprocess, "run", fake_run):
            return session.time_run(spec)

    def test_a_declared_leg_that_reports_nothing_is_an_error(self):
        # The three apparatus faults this catches — built without the feature,
        # pointed at the default binary, a write that failed — all used to read
        # as an empty dict, and an empty column an hour later is what that
        # looks like.
        with tempfile.TemporaryDirectory() as tmp:
            session = self._session(Path(tmp))
            with self.assertRaises(RuntimeError) as caught:
                self._run(session, self._instrumented(), write_report=False)
        self.assertIn("declares the instrument", str(caught.exception))
        self.assertIn("--features introspect", str(caught.exception))

    def test_the_report_is_read_back_and_kept_beside_the_readings(self):
        with tempfile.TemporaryDirectory() as tmp:
            session = self._session(Path(tmp))
            spec = self._instrumented()
            self._run(session, spec, write_report=True)
            self.assertEqual(session._last_instrument["live_peak_bytes"], "209822121")
            record = session.records[0]
            self.assertEqual(record["instrument"]["glibc_scope"], "c-malloc")
            # The file stays, under the sitting's own directory, because the
            # `malloc_info` XML in it is per-arena detail no summary carries.
            kept = Path(tmp) / record["instrument_report"]
            self.assertTrue(kept.exists())
            self.assertIn("malloc_info", kept.read_text())

    def test_the_command_shape_is_untouched_by_the_variable(self):
        # The argv a figure records must be the argv it would run without the
        # instrument; the variable and the mount are `nerdctl` arguments.
        plain = shapes._script(reserve._reserve_flagless_specs()[0].command)
        self.assertEqual(plain, shapes._script(self._instrumented().command))
        self.assertNotIn(parsing.INSTRUMENT_OUT_VAR, plain)

    def test_the_readings_are_filed_per_rep_and_not_collapsed(self):
        # `reported` keeps one dict per spec, the last rep's, because what it
        # holds is identical across reps. `live_peak_bytes` is not: it is a
        # reading, and a spread of them is what an attribution reads.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(dry_run=True)
            session = timing.Session(
                cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None, Path(tmp)
            )
            session.figure_id = "reserve"
            session.input_path = lambda name, regime: Path("/dev/null")
            spec = self._instrumented()
            session.sweep("reserve", [spec], reps=3)
            peaks = [r["live_peak_bytes"] for r in session.instrument_reports("reserve", spec)]
        self.assertEqual(len(peaks), 3)
        self.assertGreater(len(set(peaks)), 1, peaks)

    def test_a_leg_with_no_instrument_is_absent_rather_than_empty(self):
        # The same split `get_rss` makes: an absent key means this leg carries
        # no instrument, an empty list means every reading it took was
        # censored.
        with tempfile.TemporaryDirectory() as tmp:
            cfg = config.Config(dry_run=True)
            session = timing.Session(
                cfg, staging.Stager(cfg, lambda _m: None), lambda _m: None, Path(tmp)
            )
            session.figure_id = "reserve"
            session.input_path = lambda name, regime: Path("/dev/null")
            spec = reserve._reserve_flagless_specs()[0]
            session.sweep("reserve", [spec], reps=1)
            with self.assertRaises(KeyError):
                session.instrument_reports("reserve", spec)


class CensoredCells(unittest.TestCase):
    """The reserve table with a leg the kernel killed.

    A censored cell is a **bound on a peak the process never reached**, so what
    this holds is that no number is printed for it, that it leaves the fit, and
    that the figure says it must not be published. All three fail silently
    otherwise: a killed leg whose surviving reps were averaged publishes a
    median of the reps that stayed *under* the ceiling, which is exactly the
    number that makes a too-small reserve look adequate.
    """

    FIGURE = "reserve"
    #: The leg killed in these renders: the 128 MiB file in a 1 GiB allocation,
    #: which is the one an actual sitting lost.
    KILLED = ("control_xz128", "1g")
    #: The worst killed rep's `maxrss_bound_kib` is this plus `100 * rep`, so a
    #: renderer that took the first rep rather than the worst prints a number
    #: this test can tell apart.
    BOUND_KIB = 900_000.0

    def _specs(self):
        return {
            "flagless": reserve._reserve_flagless_specs(),
            "instrument": reserve._reserve_instrument_specs(),
            "diagnostic": reserve._reserve_diagnostic_specs(),
            "mechanism": [s for _, s in reserve._reserve_mechanism_specs()],
            "steps": reserve._reserve_step_specs(),
            "stated": reserve._reserve_specs(),
        }

    #: One instrument report, shaped as `introspect.rs` writes one. The
    #: quantities are monotone in the leg for the same reason the readings are,
    #: so a renderer that crossed two legs' reports prints a number these tests
    #: can tell apart.
    def _report(self, i):
        return {
            "instrument": "counting-allocator",
            "live_scope": "rust-global-alloc",
            "live_bytes": str(64 << 10),
            "live_peak_bytes": str((100 + 10 * i) * config.MIB),
            "glibc_scope": "whole-process",
            "mallinfo_arena": str((150 + 10 * i) * config.MIB),
            "mallinfo_hblkhd": "0",
            "mallinfo_uordblks": str((60 + 10 * i) * config.MIB),
            "mallinfo_fordblks": str(90 * config.MIB),
            "malloc_heaps": "6",
            "malloc_system_current": str((150 + 10 * i) * config.MIB),
            "malloc_system_max": str((260 + 10 * i) * config.MIB),
        }

    def _raw(self, killed_reps=3, total_reps=3):
        specs = self._specs()
        every = [
            *specs["flagless"],
            *specs["instrument"],
            *specs["diagnostic"],
            *specs["mechanism"],
            *specs["steps"],
            *specs["stated"],
            register._RESERVE_BASELINE,
        ]
        rss, reported, killed, instrument = {}, {}, {}, {}
        for i, spec in enumerate(every):
            key = spec.key(self.FIGURE)
            # Monotone in the leg, so the flagless fit is well conditioned and
            # the numbers below are never two legs' readings by accident.
            rss[key] = [100000.0 + 1000 * i + 10 * r for r in range(total_reps)]
            reported[key] = {
                "resolved_jobs": str(2 + i % 5),
                # Above `charge_bytes(128 MiB, 1)` — 522.0 MiB — so every
                # flagless leg of this fixture takes the block path and the fit
                # below it has legs to cover. That line carries one reader's
                # share of the pool's retention list (`block_path_afforded`).
                "resolved_budget": str(memory_model.charge_bytes(128 << 20, 1) + (1 << 20)),
            }
            if spec.instrument:
                instrument[key] = [self._report(i)] * total_reps
        # The killed reps' own records, exactly as `time_run`'s kill branch
        # writes them: the constraint line is rendered off `runs`, so a fixture
        # with an empty one would exercise only the nothing-was-reported branch.
        runs = []
        for spec in specs["flagless"]:
            if (spec.input, spec.memory) == self.KILLED:
                key = spec.key(self.FIGURE)
                killed[key] = killed_reps
                rss[key] = rss[key][: total_reps - killed_reps]
                for rep in range(killed_reps):
                    runs.append(
                        {
                            "figure": self.FIGURE,
                            "spec": dataclasses.asdict(spec),
                            "seconds": None,
                            "maxrss_kib": None,
                            "killed": True,
                            "maxrss_bound_kib": self.BOUND_KIB + 100 * rep,
                            "seconds_to_kill": 4.0 + rep,
                            "oom_kill": 1,
                            "exit": 1,
                        }
                    )
        return {
            "readings": {k: [1.0] * total_reps for k in rss},
            "rss": rss,
            "killed": killed,
            "reported": reported,
            "instrument": instrument,
            "input_sizes": {name: 1 << 30 for name in inputs.INPUTS},
            "runs": runs,
        }

    def _render(self, **kwargs):
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(), self._raw(**kwargs), Path(tmp), lambda _m: None
            )
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                return reserve.run_reserve(session), session

    def test_a_wholly_killed_leg_prints_no_number(self):
        body, _ = self._render()
        cells = [c.strip() for r in body.splitlines() if r.startswith("| ") for c in r.split("|")]
        killed = [c for c in cells if "OOM-killed" in c]
        self.assertTrue(killed, "the killed leg's cell says nothing about the kill")
        for cell in killed:
            self.assertNotIn("MiB", cell.split("·")[0])

    def test_a_killed_leg_is_out_of_the_fit_and_the_fit_says_so(self):
        body, _ = self._render()
        self.assertIn("out of the fit", body)
        self.assertIn("a bound on a peak the process never reached", body)

    def test_a_partly_killed_leg_keeps_its_reps_and_declares_the_kill(self):
        # A partly-censored leg's median understates, so the cell may show its
        # surviving reps only with the kill beside it — and it still leaves the
        # fit.
        body, _ = self._render(killed_reps=1)
        self.assertIn("1 rep(s) OOM-killed", body)
        self.assertIn("out of the fit", body)

    def test_a_killed_leg_is_printed_as_a_constraint_and_names_its_allocation(self):
        # The reading the fit cannot use and the figure must not lose: what the
        # kill still proves. Discarded, it left `KILL_TOLERANT` recording a
        # reading and the renderer throwing it away.
        body, _ = self._render()
        self.assertIn("What the killed legs still prove", body)
        self.assertIn("**not** fitted", body)
        self.assertIn(f"`-m {self.KILLED[1]}`", body)
        self.assertIn("did not fit 1,024 MiB", body)

    def test_the_constraint_carries_the_worst_reps_floor_and_calls_it_one(self):
        # `maxrss_bound_kib` and `seconds_to_kill` reached `raw.json` with
        # nothing reading them. The worst rep's pair, printed as one rep's, and
        # labelled a floor rather than a peak.
        body, _ = self._render()
        worst = self.BOUND_KIB + 100 * 2
        self.assertIn(formatting.fmt_mib(worst), body)
        self.assertIn("6.0 s in", body)
        self.assertIn("a floor on the peak, not the peak", body)

    def test_a_censored_leg_with_no_recorded_bound_says_so_rather_than_guessing(self):
        # A sitting taken before the kill branch existed carries `killed` and no
        # `runs`. An empty bound list means *nothing is known*, which is not the
        # same as zero and must not print as a number.
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            raw["runs"] = []
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        self.assertIn("reported no bound before it died", body)
        self.assertIn("did not fit 1,024 MiB", body)

    def test_every_fit_states_the_window_it_covers(self):
        # `evidence` rule 2 with the sign flipped: a leg is censored exactly
        # when its resident ran closest to its ceiling, so a fit over what
        # survives is a fit over the legs that had room. Naming only what left
        # asks the reader to subtract from a tuple the table never prints.
        body, _ = self._render()
        section = body.split("Resident against the reader count")[1].split(
            "What the killed legs still prove"
        )[0]
        fits = [ln for ln in section.splitlines() if ln.startswith("- **")]
        self.assertEqual(len(fits), len(shapes.RESERVE_FLAGLESS_INPUTS))
        for line in fits:
            self.assertTrue(
                any(w in line for w in ("it covers", "what is left is", "left cover")),
                f"the line states no window: {line}",
            )
            # Every window names allocations, not just reader counts.
            self.assertTrue(
                any(f"`{token}` at" in line for token, _ in shapes.RESERVE_LIMITS),
                f"the window names no allocation: {line}",
            )

    def test_every_published_term_names_the_pool_it_excludes(self):
        # Both terms of every line here are the **remainder** — what a
        # leg held outside the block pool's retention list, which is subtracted
        # before the fit because it is known in advance and bends the line at
        # `POOL_DEPTH`. A reader who takes them for the whole of what a leg held
        # is out by that term: 72 MiB at one reader of 24 MiB blocks, 384 at
        # 128, and a unit a reader more above four.
        body, _ = self._render()
        section = body.split("Resident against the reader count")[1].split(
            "What the killed legs still prove"
        )[0]
        self.assertIn("(POOL_DEPTH.max(jobs) − 1) × unit", section)
        fits = [ln for ln in section.splitlines() if ln.startswith("- **")]
        self.assertEqual(len(fits), len(shapes.RESERVE_FLAGLESS_INPUTS))
        for line in fits:
            self.assertIn("outside the pool", line)
        live = next(
            ln for ln in body.splitlines() if ln.startswith("**What the program itself held")
        )
        self.assertIn("outside the block pool", live)
        # And the comparison that reads the slope: both sides exclude the list,
        # `reader_bytes` being the per-worker term without it.
        self.assertIn("outside the pool plus", live)

    def test_the_no_line_branch_states_its_window_too(self):
        # "No line" is a claim about a window as much as a fit is, and it was the
        # branch that named only the declined legs. Reached by resolving one
        # reader count everywhere, which is what leaves a single point — below
        # even a secant, since a secant needs two abscissae as much as a fit
        # does.
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            for report in raw["reported"].values():
                report["resolved_jobs"] = "4"
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        no_line = [ln for ln in body.splitlines() if ln.startswith("- **") and "no line" in ln]
        self.assertEqual(len(no_line), len(shapes.RESERVE_FLAGLESS_INPUTS))
        for line in no_line:
            self.assertIn("what is left is", line)
            self.assertTrue(
                any(f"`{token}` at" in line for token, _ in shapes.RESERVE_LIMITS),
                f"the window names no allocation: {line}",
            )

    def _two_count_body(self):
        counts = [4, 4, 4, 4, 5, 5]
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            for key, report in raw["reported"].items():
                report["resolved_jobs"] = str(counts[sum(map(ord, key)) % len(counts)])
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                return reserve.run_reserve(session)

    def test_a_two_point_family_publishes_a_secant_and_no_intercept(self):
        # The guard is three distinct reader counts, because a two-term model
        # passes exactly through two points and prints `±0 MiB` as though it
        # were a residual. What it withholds is the intercept: the slope
        # between two counts is a difference the axis measured and carries no
        # model claim, so it is published and the intercept is not.
        self.assertEqual(shapes.RESERVE_FIT_MIN_COUNTS, 3)
        body = self._two_count_body()
        section = body.split("Resident against the reader count")[1].split(
            "What the killed legs still prove"
        )[0]
        lines = [ln for ln in section.splitlines() if ln.startswith("- **")]
        self.assertEqual(len(lines), len(shapes.RESERVE_FLAGLESS_INPUTS))
        for line in lines:
            with self.subTest(line=line):
                self.assertIn("secant", line)
                self.assertIn("a reader **", line)
                # The slope is the remainder's too (`_depooled`).
                self.assertIn("outside the pool", line)
                self.assertIn("distinct reader count(s), under the 3", line)
                # The two things the guard withholds, and nothing else: no
                # intercept, and no residual value printed beside it.
                self.assertNotIn("fixed **", line)
                self.assertNotIn("residuals reach", line)
                # Named endpoints, so a reader knows which difference it is.
                self.assertIn(" → ", line)
                self.assertTrue(
                    any(f"`{token}`" in line for token, _ in shapes.RESERVE_LIMITS),
                    f"the secant names no allocation: {line}",
                )

    def test_the_instrument_family_crosses_the_same_guard(self):
        # The guard is a property of the model, so it cannot live in one
        # renderer, or the instrument account admits an intercept at two
        # distinct counts while the flagless axis refuses one in the same
        # sitting.
        body = self._two_count_body()
        live = [ln for ln in body.splitlines() if "a reader" in ln and "secant" in ln]
        self.assertTrue(
            any("What a reader costs the program" in ln for ln in live),
            "the instrument account published something other than a secant",
        )
        line = next(ln for ln in live if "What a reader costs the program" in ln)
        self.assertNotIn("fixed **", line)
        self.assertNotIn("a residual of", line)
        # Named legs, not bare counts: a difference nobody can locate is a
        # difference nobody can re-take.
        self.assertIn(" → ", line)
        self.assertTrue(
            any(f"in {token}" in line for token in reserve.RESERVE_INSTRUMENT_LIMITS),
            f"the secant names no leg: {line}",
        )
        # The comparison a censored sitting would otherwise lose silently: it
        # reads the slope alone, so a secant keeps it. Both sides exclude the
        # pool's retention list, which is what makes them commensurable
        # (`_depooled`).
        self.assertIn("`BlockCache::reader_bytes` bills", line)
        self.assertIn("outside the pool", line)

    def test_the_guard_is_above_the_arithmetic_floor_it_sits_on(self):
        # Two floors, deliberately: `_least_squares` refuses one point because
        # the line is undefined there, and the publication guard refuses two
        # because the residual is. Collapsing them would make the arithmetic
        # answer the publication question.
        self.assertGreater(shapes.RESERVE_FIT_MIN_COUNTS, 2)
        fixed, slope = reserve._least_squares([(1, 100.0), (2, 200.0)])
        self.assertAlmostEqual(fixed, 0.0)
        self.assertAlmostEqual(slope, 100.0)

    def test_the_figure_reports_its_kills_for_the_publication_bar(self):
        _, session = self._render()
        kills = session.figure_kills(self.FIGURE)
        self.assertEqual(len(kills), 1)
        self.assertEqual(sum(kills.values()), 3)

    def test_the_note_pasted_above_the_table_refuses_publication(self):
        _, session = self._render()
        note = emit.censored_note(session.figure_kills(self.FIGURE))
        self.assertIn("must not be published", note)
        self.assertIn("OOM-killed", note)

    # -- the attribution -------------------------------------------------

    def test_the_account_adds_the_decoder_dictionaries_back_by_hand(self):
        # `liblzma` allocates through C `malloc`, so the counting allocator
        # cannot see it and glibc cannot separate it. A decomposition that
        # subtracted the two families without this term charges 8 MiB a reader
        # to retention — which is the term the whole sitting is about.
        body, session = self._render()
        section = body.split("The account, term by term")[1].split("What the program itself")[0]
        self.assertIn("Decoder dictionaries", section)
        self.assertIn(f"{memory_model.XZ_DICT_BYTES:,} bytes", body)
        rows = [ln for ln in section.splitlines() if ln.startswith("| instrument")]
        self.assertEqual(len(rows), len(reserve._reserve_instrument_specs()))
        # The column is `readers x dictionary` exactly, read off the
        # arrangement the run itself reported rather than off the limit — so
        # the fixture's own resolved counts are what it has to reproduce.
        readers = [
            int(session.reported[spec.key(self.FIGURE)]["resolved_jobs"])
            for spec in reserve._reserve_instrument_specs()
        ]
        for row, count in zip(rows, readers):
            self.assertIn(
                formatting._fmt_budget_bytes(count * memory_model.XZ_DICT_BYTES), row
            )

    def test_the_fit_is_stated_against_the_charge_the_source_bills(self):
        # The point of taking the account: what a reader costs the program plus
        # the C dictionary the counter is blind to, against what
        # what the compressed source bills a sub-stream. It is the one
        # comparison that says whether the number the budget rule divides by is
        # the number a reader actually is.
        body, _ = self._render()
        line = next(ln for ln in body.splitlines() if ln.startswith("**What the program itself"))
        self.assertIn("Against the charge", line)
        self.assertIn(
            formatting._fmt_budget_bytes(memory_model.reader_bytes(shapes.RESERVE_MECHANISM_UNIT)),
            line,
        )

    def test_the_two_columns_that_can_leave_range_are_explained(self):
        # `RSS − heap high-water` goes negative and the `fordblks` share passes
        # 100% whenever two high-waters are taken at different instants. Both
        # read as apparatus faults unless the table says otherwise.
        body, _ = self._render()
        section = body.split("The account, term by term")[1].split("What the program itself")[0]
        self.assertIn("RSS − heap high-water", section)
        self.assertIn("non-simultaneity", section)

    def test_the_program_fit_is_evaluated_inside_its_own_window(self):
        # An intercept is a physical quantity only where the fit still holds
        # where the mechanism is simplest: a `403 MiB` intercept fitted over
        # 3–24 readers predicts 436 MiB at one reader, where the process holds
        # 62.9 (`.claude/skills/evidence/SKILL.md`, rule 2).
        body, _ = self._render()
        line = next(ln for ln in body.splitlines() if ln.startswith("**What the program itself"))
        self.assertIn("smallest arrangement in its own window", line)
        self.assertIn("residual", line)

    def test_the_remainder_carries_a_name_or_says_it_has_none(self):
        # Never a bare number carried across sessions: a residual with no owner
        # acquires a false one.
        body, _ = self._render()
        self.assertTrue(
            "**The remainder has a name**" in body or "**The remainder has no name here**" in body,
            "the account ends on neither a name nor an explicit no-name",
        )

    def test_a_two_heap_report_withholds_the_one_heap_account(self):
        # An instrument counting in front of mimalloc leaves glibc holding only
        # what reached C `malloc`, so subtracting the counter's high-water from
        # glibc's takes the Rust heap out of a heap that never held it. The
        # account is withheld and says so; the counter's own line, which no
        # allocator moves, still prints.
        one_heap = self._report

        def two_heaps(i):
            return {
                **one_heap(i),
                "mimalloc_scope": "rust-heap",
                "mimalloc_committed_peak_bytes": str((400 + 10 * i) * config.MIB),
                "glibc_scope": "c-malloc",
            }

        with unittest.mock.patch.object(self, "_report", two_heaps):
            body, _ = self._render()
        self.assertIn("**withheld**", body)
        self.assertIn("`KD34`", body)
        self.assertNotIn("The account, term by term", body)
        self.assertNotIn("The remainder has", body)
        self.assertTrue(
            any(ln.startswith("**What the program itself") for ln in body.splitlines()),
            "the counter's own line went with the account",
        )

    def test_a_two_heap_report_prints_each_heap_s_readings_and_subtracts_none(self):
        # Readings are not a model, so withholding the account does not reach
        # them: each leg's row carries every reading the report gives, each
        # column headed with the memory it covers, and no column is a
        # difference of two others.
        one_heap = self._report

        def two_heaps(i):
            return {
                **one_heap(i),
                "mimalloc_scope": "rust-heap",
                "mimalloc_committed_peak_bytes": str((400 + 10 * i) * config.MIB),
                "mimalloc_reserved_peak_bytes": str(1024 * config.MIB),
                "glibc_scope": "c-malloc",
            }

        with unittest.mock.patch.object(self, "_report", two_heaps):
            body, _ = self._render()
        header = next(ln for ln in body.splitlines() if "mimalloc committed high-water" in ln)
        cells = [c.strip() for c in header.strip("|").split("|")]
        for heading, covers in (
            ("Live high-water", "Rust"),
            ("mimalloc committed high-water", "the Rust heap"),
            ("mimalloc reserved high-water", "the Rust heap's address space"),
            ("glibc heap high-water", "C `malloc` alone"),
            ("glibc arenas", "C"),
            ("Freed and held at exit", "glibc, C"),
            ("mmap-backed at exit", "glibc, C"),
        ):
            with self.subTest(heading=heading):
                cell = next(c for c in cells if c.startswith(heading))
                self.assertIn(f" — {covers}", cell)
        self.assertNotIn("−", header)
        rows = [
            ln
            for ln in body.split(header)[1].split("\n\n")[0].splitlines()
            if ln.startswith("| instrument")
        ]
        self.assertEqual(len(rows), len(reserve._reserve_instrument_specs()))
        self.assertTrue(
            any(formatting._fmt_budget_bytes(400 * config.MIB) in r for r in rows)
            or any(formatting._fmt_budget_bytes(410 * config.MIB) in r for r in rows)
            or any(formatting._fmt_budget_bytes(420 * config.MIB) in r for r in rows),
            "no row carries mimalloc's committed high-water",
        )
        self.assertTrue(all(formatting._fmt_budget_bytes(1024 * config.MIB) in r for r in rows))
        # The mechanism legs' cap reaches C alone on such a build, and the
        # reference no longer claims glibc holds the Rust heap.
        self.assertIn("On this build the cap reaches C alone", body)
        self.assertNotIn("the reference — glibc", body)

    def test_the_check_states_the_arrangement_and_the_tolerance(self):
        # The exact half is whether the instrument build resolved the shipped
        # build's arrangement; the resident half is a stated tolerance, because
        # a different binary cannot be held to the shipped leg's own spread.
        body, _ = self._render()
        checks = [ln for ln in body.splitlines() if ln.startswith("- `-m ")]
        self.assertEqual(len(checks), len(reserve.RESERVE_INSTRUMENT_LIMITS))
        for line in checks:
            self.assertIn("resolved", line)
            self.assertIn(f"{reserve.INSTRUMENT_TOLERANCE_PCT}%", line)

    def test_the_diagnostic_legs_print_the_bill_beside_each_heap_and_stay_out_of_the_fit(self):
        # The attribution of what the shipped build holds above its charge at
        # the failing block size: one row a diagnostic leg, its own run's bill
        # beside every heap's reading, checked against its shipped leg — and
        # none of it in the counter's line, which is a fit at another unit.
        body, session = self._render()
        section = body.split("What the shipped build holds above its charge at")[1]
        rows = [ln for ln in section.splitlines() if ln.startswith("| instrument, 128 MiB")]
        legs = reserve._reserve_diagnostic_specs()
        self.assertEqual(len(rows), len(legs))
        unit = dict((name, unit) for name, _, unit in shapes.RESERVE_FLAGLESS_INPUTS)[
            reserve.RESERVE_DIAGNOSTIC_INPUT
        ]
        for row, spec in zip(rows, legs):
            with self.subTest(leg=spec.label):
                jobs = int(session.reported[spec.key(self.FIGURE)]["resolved_jobs"])
                billed = formatting._fmt_budget_bytes(memory_model.charge_bytes(unit, jobs))
                self.assertEqual([c.strip() for c in row.strip("|").split("|")][2], billed)
        checks = [ln for ln in section.splitlines() if ln.startswith("- 128 MiB blocks, `-m ")]
        self.assertEqual(len(checks), len(legs))
        for line in checks:
            self.assertIn(f"{reserve.INSTRUMENT_TOLERANCE_PCT}%", line)
        fit = next(ln for ln in body.splitlines() if ln.startswith("**What the program itself"))
        self.assertIn(
            f"over the {len(reserve._reserve_instrument_specs())} leg(s) that survived", fit
        )
        self.assertNotIn("128 MiB", fit)

    def test_a_sitting_without_the_diagnostic_legs_renders_without_their_table(self):
        # A sitting taken before the legs existed is re-rendered, not refused.
        raw = self._raw()
        for spec in reserve._reserve_diagnostic_specs():
            key = spec.key(self.FIGURE)
            for field in ("readings", "rss", "instrument", "reported"):
                raw[field].pop(key, None)
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        self.assertNotIn("What the shipped build holds above its charge at", body)
        self.assertIn("**What a flagless scan resolves", body)

    def test_a_killed_diagnostic_leg_prints_no_reading(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            spec = reserve._reserve_diagnostic_specs()[0]
            key = spec.key(self.FIGURE)
            raw["instrument"][key] = []
            raw["rss"][key] = []
            raw["killed"][key] = 3
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        row = next(ln for ln in body.splitlines() if ln.startswith(f"| {spec.label} |"))
        cells = row.split(f"| {spec.label} |")[1]
        self.assertIn("**OOM-killed**, 3 rep(s)", cells)
        self.assertNotIn("MiB", cells)

    def test_a_censored_instrument_leg_prints_no_terms(self):
        # The report is written at exit, so a killed leg has none — and a
        # renderer that read a missing field as zero would print an account
        # summing to a resident set nobody measured.
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            spec = reserve._reserve_instrument_specs()[0]
            key = spec.key(self.FIGURE)
            raw["instrument"][key] = []
            raw["rss"][key] = []
            raw["killed"][key] = 3
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        row = next(
            ln for ln in body.splitlines() if ln.startswith(f"| {spec.label} |")
        )
        self.assertIn("OOM-killed", row)
        self.assertNotIn("MiB", row)

    def test_an_untouched_sitting_carries_no_kill_and_no_note(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = self._raw()
            raw["killed"] = {}
            for spec in reserve._reserve_flagless_specs():
                key = spec.key(self.FIGURE)
                if len(raw["rss"][key]) < 3:
                    raw["rss"][key] = [100000.0, 100010.0, 100020.0]
            session = replay.ReplaySession(
                config.Config(), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        self.assertNotIn("OOM-killed", body)
        self.assertEqual(session.figure_kills(self.FIGURE), {})


class ChargeModelSection(unittest.TestCase):
    """The model check the reserve renderer prints, cell by cell.

    What it is written against is the account done by hand: a sitting that
    reports forty headroom percentages and leaves the account to whoever reads
    the log afterwards. Three things fail silently here. A **criterion stated
    after the answer** reads as a description of whatever came back. A **pool
    term folded into the remainder** reads as a term nothing accounts for and
    as a breach on a larger block size, which is why the charge bills it. And a
    **declined or censored leg
    left in the table** evaluates the model at a leg that ran none of it.
    """

    FIGURE = CensoredCells.FIGURE
    KILLED = CensoredCells.KILLED
    BOUND_KIB = CensoredCells.BOUND_KIB

    # The reserve renderer's one fixture, borrowed rather than rebuilt: a
    # second raw dict is a second account of what a sitting records, and the
    # two drift.
    _specs = CensoredCells._specs
    _report = CensoredCells._report
    _raw = CensoredCells._raw
    _render = CensoredCells._render

    #: One flagless arrangement per input and limit: reader count and worst rep
    #: in MiB.
    #:
    #: The four original limits are the reserve constant's five-build grid at
    #: its r384 build — the 24 MiB rows are that build's headroom column
    #: inverted against its container limit, the 128 MiB rows the same readings
    #: `SEED_CELLS` carries. **`544m` and `1088m` are constructed**, no sitting
    #: having measured them: each is a one- or two-reader arrangement whose worst
    #: rep satisfies the criterion, chosen so the model table carries a
    #: one-reader cell at each block size, which no cell of the four-limit grid
    #: reached.
    #:
    #: **The budget a cell renders under is its own arrangement's charge, not
    #: its container's allowance** (`_seeded_body`), so a cell here takes the
    #: block path whatever `-m` it carries — which is why `control_xz128` at
    #: `512m` bills 660 MiB inside 512. The fixture is exercising the renderer's
    #: arithmetic over a reader count; which limits afford which count is
    #: `reserve_axis_problems`' question and is checked there.
    SEEDED = {
        ("control_xz", "512m"): (2, 251.9),
        ("control_xz", "544m"): (1, 240.0),
        ("control_xz", "1g"): (11, 817.2),
        ("control_xz", "1088m"): (12, 860.0),
        ("control_xz", "1536m"): (19, 1224.2),
        ("control_xz", "2g"): (24, 1515.5),
        ("control_xz128", "512m"): (2, 802.1),
        ("control_xz128", "544m"): (1, 760.0),
        ("control_xz128", "1g"): (3, 939.9),
        ("control_xz128", "1088m"): (2, 900.0),
        ("control_xz128", "1536m"): (4, 1077.7),
        ("control_xz128", "2g"): (5, 1342.6),
    }

    def _seeded_body(self, bumps=None):
        """The renderer over a fixture whose flagless legs are the five-build
        grid's own readings, nothing killed.

        `bumps` adds MiB to one leg's worst rep, which is how a cell is put in a
        chosen band of the criterion without inventing a second fixture."""
        raw = self._raw()
        raw["killed"] = {}
        units = {name: unit for name, _, unit in shapes.RESERVE_FLAGLESS_INPUTS}
        for spec in reserve._reserve_flagless_specs():
            jobs, worst = self.SEEDED[(spec.input, spec.memory)]
            worst += (bumps or {}).get((spec.input, spec.memory), 0.0)
            key = spec.key(self.FIGURE)
            raw["rss"][key] = [worst * 1024 - 20, worst * 1024 - 10, worst * 1024]
            raw["reported"][key] = {
                "resolved_jobs": str(jobs),
                "resolved_budget": str(memory_model.charge_bytes(units[spec.input], jobs)),
            }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                return reserve.run_reserve(session)

    def _unnamed_at(self, cell):
        """What the model leaves unnamed at one seeded cell, in bytes."""
        units = {name: unit for name, _, unit in shapes.RESERVE_FLAGLESS_INPUTS}
        jobs, worst = self.SEEDED[cell]
        return worst * config.MIB - memory_model.charge_bytes(units[cell[0]], jobs)

    def _bump_to(self, cell, unnamed):
        """MiB to add to one seeded cell's worst rep so that its unnamed
        remainder lands at `unnamed` bytes.

        Computed rather than written, because the bands are bounded on both
        sides: a bump stated as "the bound plus a megabyte" lands in whichever
        band the cell's own remainder plus that number falls in, which is the
        `rule` band as often as not."""
        return (unnamed - self._unnamed_at(cell)) / config.MIB

    def _model_rows(self, body):
        section = body.split("The charge against what was held")[1].split(
            "What the process says it held"
        )[0]
        return [ln for ln in section.splitlines() if ln.startswith("| ")]

    def test_the_criterion_is_stated_before_the_answer(self):
        body, _ = self._render()
        section = body.split("The charge against what was held")[1].split(
            "What the process says it held"
        )[0]
        # The prose above the table is the criterion; whatever follows the last
        # row of it is the answer. Read that way rather than off the verdict's
        # opening words, which name the band and so are not one sentence.
        lines = section.splitlines()
        last_row = max(i for i, ln in enumerate(lines) if ln.startswith("| "))
        criterion = "\n".join(lines[:last_row])
        verdict = "\n".join(lines[last_row + 1 :]).strip()
        self.assertIn("non-negative", criterion)
        self.assertIn("MEMORY_RESERVE", criterion)
        # Both ceilings, and which finding each is: a section stating one of
        # them would read as a criterion while answering the other question.
        self.assertIn("MEMORY_UNPOOLED_BOUND", criterion)
        self.assertIn("the **bound** is wrong", criterion)
        self.assertIn("over-bill", criterion)
        # And that the column below says which of the three a cell crossed,
        # since that is what decides how the cell is read.
        self.assertIn("names the band", criterion)
        self.assertTrue(verdict, "the section states a criterion and never answers it")

    def test_the_seeded_readings_pass_and_the_floor_is_its_own_column(self):
        body = self._seeded_body()
        self.assertIn("The model holds at every cell above", body)
        rows = self._model_rows(body)
        # Eight flagless legs, and every one of them took the block path at the
        # budget its own reported arrangement carries.
        self.assertEqual(len(rows), len(self.SEEDED) + 1)
        pooled = next(r for r in rows if "128 MiB blocks, `-m 512m`" in r)
        # `(POOL_DEPTH - 1) x 128 MiB`, named rather than left in the remainder.
        self.assertIn(formatting._fmt_budget_bytes(3 * (128 << 20)), pooled)
        self.assertIn(formatting._fmt_budget_bytes(142.0 * config.MIB)[:5], pooled)

    def test_the_pool_term_is_billed_at_every_cell_of_each_block_size(self):
        # The column exists to keep the pool out of the remainder, and a table
        # in which any cell reads `—` there is a cell whose bill has lost its
        # larger half. The term never clamps off, so every row of every block
        # size carries it.
        rows = self._model_rows(self._seeded_body())
        for _name, label, unit in shapes.RESERVE_FLAGLESS_INPUTS:
            with self.subTest(block_size=label):
                mine = [r for r in rows if r.startswith(f"| {label}, ")]
                self.assertTrue(mine, f"{label} has no cell at all")
                for row in mine:
                    readers = int(row.split("|")[2].strip().rstrip("r"))
                    self.assertIn(
                        formatting._fmt_budget_bytes(
                            (max(memory_model.LIBRARY_POOL_DEPTH, readers) - 1) * unit
                        ),
                        row,
                    )

    def test_an_over_bill_is_named_in_those_words_and_refutes(self):
        # The default fixture holds ~100 MiB against a charge of several
        # hundred, which is the over-bill side of the criterion. It refutes —
        # it is never apparatus scatter, having to exceed the whole of the rest
        # of the process's footprint before the arithmetic reports it at all,
        # and the sitting discharges nothing by printing it.
        body, _ = self._render()
        self.assertIn("over-billed", body)
        self.assertIn("The model is refuted, and by these cells:", body)
        # And the refutation does not bar publication.
        self.assertIn("The table publishes all the same", body)
        # And the verdict says why *this* band refutes rather than asserting the
        # rule band's reason over every cell in the stanza.
        self.assertIn(memory_model.band_refutes(memory_model.BAND_OVER_BILL), body)
        self.assertNotIn("a finding rather than a refutation", body)

    def test_a_cell_above_the_bound_alone_publishes_with_its_finding(self):
        # Between the two lines the allocation holds and the number the
        # count is predicted against is wrong. The verdict says which, and
        # re-derives the bound from the sitting's own remainders rather than
        # owing a re-take.
        over = memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND + config.MIB
        bump = self._bump_to(("control_xz128", "1g"), over)
        body = self._seeded_body(bumps={("control_xz128", "1g"): bump})
        self.assertNotIn("The model is refuted", body)
        self.assertIn("a finding rather than a refutation", body)
        self.assertIn("MEMORY_UNPOOLED_BOUND", body)
        # The bumped cell is the sitting's worst remainder, rounded up on the
        # 64 MiB grid the constant was read off.
        rederived = memory_model.rederived_unpooled_bound(over)
        self.assertIn(
            f"re-derive `MEMORY_UNPOOLED_BOUND` at **{formatting._fmt_budget_bytes(rederived)}**",
            body,
        )
        self.assertIn("| **bound** |", body)

    def test_a_cell_above_the_reserve_refutes_and_says_so(self):
        # The outer line: a remainder the reserve cannot cover is an
        # arrangement the discovery cannot keep inside its allocation.
        bump = self._bump_to(
            ("control_xz128", "1g"), memory_model.LIBRARY_MEMORY_RESERVE + config.MIB
        )
        body = self._seeded_body(bumps={("control_xz128", "1g"): bump})
        self.assertIn("The model is refuted, and by these cells:", body)
        self.assertIn("The table publishes all the same", body)
        self.assertIn(memory_model.band_refutes(memory_model.BAND_RULE), body)
        self.assertIn("| **rule** |", body)

    def test_the_two_bands_are_reported_apart_in_one_sitting(self):
        # The case one sentence over both cannot state: a cell of each band. The
        # refutation is named on the rule cells alone, and the bound cell is
        # published beside it with its finding.
        body = self._seeded_body(
            bumps={
                ("control_xz128", "1g"): self._bump_to(
                    ("control_xz128", "1g"),
                    memory_model.LIBRARY_MEMORY_RESERVE + config.MIB,
                ),
                ("control_xz", "2g"): self._bump_to(
                    ("control_xz", "2g"),
                    memory_model.LIBRARY_MEMORY_UNPOOLED_BOUND + config.MIB,
                ),
            }
        )
        self.assertIn("The model is refuted, and by these cells:", body)
        self.assertIn("a finding rather than a refutation", body)
        refuting = body.split("The model is refuted")[1].split("Inside the rule")[0]
        self.assertIn("128 MiB blocks** at `-m 1g`", refuting)
        self.assertNotIn("24 MiB blocks** at `-m 2g`", refuting)
        # And no bound is re-derived here: a sitting in which the model is
        # refuted has a worst remainder that is not a reading a term of that
        # model may be sized to.
        self.assertNotIn("re-derive `MEMORY_UNPOOLED_BOUND`", body)
        self.assertIn("No bound is re-derived from this sitting", body)

    def test_a_declined_leg_is_absent_rather_than_evaluated(self):
        # The streaming fallback holds none of the model's terms, so a leg whose
        # reported budget cannot afford one reader is not a cell of it.
        raw = self._raw()
        raw["killed"] = {}
        spec = next(
            s
            for s in reserve._reserve_flagless_specs()
            if (s.input, s.memory) == ("control_xz128", "512m")
        )
        for other in reserve._reserve_flagless_specs():
            jobs, worst = self.SEEDED[(other.input, other.memory)]
            key = other.key(self.FIGURE)
            raw["rss"][key] = [worst * 1024]
            units = {n: u for n, _, u in shapes.RESERVE_FLAGLESS_INPUTS}
            raw["reported"][key] = {
                "resolved_jobs": str(jobs),
                "resolved_budget": str(memory_model.charge_bytes(units[other.input], jobs)),
            }
        raw["reported"][spec.key(self.FIGURE)] = {
            "resolved_jobs": "1",
            "resolved_budget": str(memory_model.charge_bytes(128 << 20, 1) - 1),
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        rows = self._model_rows(body)
        self.assertEqual(len(rows), len(self.SEEDED))
        self.assertFalse([r for r in rows if "128 MiB blocks, `-m 512m`" in r])

    def test_no_cell_at_all_is_said_rather_than_read_as_a_pass(self):
        # "Every cell holds" and "there were no cells" are not the same claim,
        # and the second is what a sitting where everything declined produces.
        raw = self._raw()
        raw["killed"] = {}
        for spec in reserve._reserve_flagless_specs():
            raw["reported"][spec.key(self.FIGURE)] = {
                "resolved_jobs": "1",
                "resolved_budget": "1",
            }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        self.assertIn("The model was evaluated at no cell", body)
        self.assertNotIn("The model holds at every cell", body)
        self.assertEqual(len(self._model_rows(body)), 1)

    def test_a_censored_leg_is_absent_rather_than_evaluated(self):
        # A kill leaves a bound on a peak never reached, which is not a `held`
        # the model may be evaluated at — the exclusion the fit already makes.
        body, _ = self._render()
        rows = self._model_rows(body)
        self.assertFalse([r for r in rows if f"`-m {self.KILLED[1]}`" in r and "128 MiB" in r])

    # -- which path each cell ran ------------------------------------------

    def _flagless_rows(self, body):
        section = body.split("What a flagless scan resolves")[1].split(
            "Resident against the reader count"
        )[0]
        return [ln for ln in section.splitlines() if ln.startswith("| `-m ")]

    def test_every_flagless_cell_names_the_path_it_ran(self):
        # The block path and the streaming fallback hold different
        # things, so a column mixing them is two series printed as one — and an
        # unmarked cell cannot be told from a cell nobody checked. Every cell
        # says which, including the ones that took the block path.
        body = self._seeded_body()
        rows = self._flagless_rows(body)
        self.assertEqual(len(rows), len(shapes.RESERVE_LIMITS))
        for row in rows:
            with self.subTest(row=row):
                for cell in row.split("|")[2:-1]:
                    self.assertTrue(
                        "*block path*" in cell or "*streaming*" in cell,
                        f"cell names no path: {cell}",
                    )

    def test_a_declined_cell_says_streaming_where_a_block_cell_does_not(self):
        # The seeded fixture's budgets are what each leg's own reported
        # arrangement carries, so every cell takes the block path; dropping one
        # leg's budget a byte below the line is the only difference.
        raw = self._raw()
        raw["killed"] = {}
        units = {n: u for n, _, u in shapes.RESERVE_FLAGLESS_INPUTS}
        target = ("control_xz128", "512m")
        for spec in reserve._reserve_flagless_specs():
            jobs, worst = self.SEEDED[(spec.input, spec.memory)]
            key = spec.key(self.FIGURE)
            raw["rss"][key] = [worst * 1024]
            budget = (
                memory_model.charge_bytes(units[spec.input], 1) - 1
                if (spec.input, spec.memory) == target
                else memory_model.charge_bytes(units[spec.input], jobs)
            )
            raw["reported"][key] = {"resolved_jobs": str(jobs), "resolved_budget": str(budget)}
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            with unittest.mock.patch.object(
                leg_binaries, "ensure_instrument_binary", lambda *_a, **_k: Path("/pgdt")
            ):
                body = reserve.run_reserve(session)
        row = next(r for r in self._flagless_rows(body) if r.startswith(f"| `-m {target[1]}`"))
        cells = row.split("|")[2:-1]
        # `RESERVE_FLAGLESS_INPUTS` order is the column order, and the declined
        # leg is the second of them.
        self.assertIn("*block path*", cells[0])
        self.assertIn("*streaming*", cells[1])
        self.assertNotIn("*block path*", cells[1])


if __name__ == "__main__":
    unittest.main()
