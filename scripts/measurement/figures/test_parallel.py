"""Tests of `measurement.figures.parallel`."""

from __future__ import annotations

import tempfile
import unittest
import unittest.mock
from pathlib import Path

from measurement import config
from measurement import inputs
from measurement import machine_state
from measurement import memory_model
from measurement import replay
from measurement import shapes
from measurement import staging
from measurement import timing
from measurement.figures import parallel
from measurement.figures import register
from measurement.figures import throughput


class ParallelFigures(unittest.TestCase):
    """The two `parallel-*` figures: the first in the register whose axis is
    `pgdt`'s own `--jobs`.

    Every assertion here is a way to get a plausible table of the wrong thing,
    which is the family this module already covers for the allocator legs.
    Three of them matter most, and each fails silently
    without a test. A **clamped row** — a stated count the library quietly
    reduces because the budget cannot hold that many partitions — is a lower
    count wearing a higher label, and the resulting table is monotone and wrong.
    A **shape drifting off `SWEEP_JOBS`** puts a second worker count in the
    register with nothing saying so, which is the defect the worker-count
    apparatus rule closed one level up. And a **leg whose rate is per compressed byte** reads five times too
    slow under a heading that looks like the plain leg's.
    """

    def test_both_figures_are_taken_and_no_longer_untaken(self):
        # Both are in `FIGURES`, where the doc-side checks apply.
        untaken = [f.id for f in register.UNTAKEN]
        self.assertNotIn("parallel-scan-throughput", untaken)
        self.assertNotIn("parallel-peak-rss", untaken)
        taken = [f.id for f in register.FIGURES]
        self.assertIn("parallel-scan-throughput", taken)
        self.assertIn("parallel-peak-rss", taken)

    def test_every_registered_job_count_has_a_shape_in_every_family(self):
        # The provider family's count is its `target_partitions`; the `--jobs`
        # it states is its untimed builder's.
        for family in shapes.JOBS_AXIS:
            for jobs in shapes.PARALLEL_JOBS:
                with self.subTest(family=family, jobs=jobs):
                    script = shapes._script(f"{family}{jobs}")
                    if family.startswith(shapes.PARALLEL_SCAN):
                        self.assertEqual(shapes._dfcli_partitions(script), {str(jobs)})
                    else:
                        self.assertIn(f"--jobs {jobs} ", script)
                    self.assertEqual(script.count("time "), 1)

    def test_a_job_count_the_figures_do_not_carry_is_an_error(self):
        # The shape is parsed rather than matched, so an unregistered count has
        # to be refused explicitly or it would run at whatever was typed and be
        # read as a row of the table.
        for command in ("parse-jobs-3", "parse-jobs-", "parse-jobs-all",
                        "dfcli-query-typed-jobs-7", "parse-rss-jobs-x"):
            with self.subTest(command=command):
                with self.assertRaises(ValueError):
                    shapes._script(command)

    def test_a_family_declared_but_not_built_is_an_error(self):
        # `JOBS_AXIS` is what `_script` dispatches on and what `command_shapes`
        # enumerates, so a prefix added to it with no branch behind it would
        # otherwise produce a shape the reconciliation counts and the builder
        # cannot make.
        with unittest.mock.patch.object(
            shapes, "JOBS_AXIS", (*shapes.JOBS_AXIS, "query-strings-jobs-")
        ):
            with self.assertRaises(ValueError):
                shapes._script("query-strings-jobs-4")

    def test_every_row_states_the_same_budget(self):
        # The axis is the worker count; a budget that moved with it would make
        # each row a different apparatus and the ratios a comparison of two
        # variables.
        # A provider leg states it as the setting `pgdt --memory` is the flag
        # for, ahead of its query in the same process.
        allowance = memory_model.stated_allowance(shapes.PARALLEL_BUDGET)
        for family in shapes.JOBS_AXIS:
            for jobs in shapes.PARALLEL_JOBS:
                with self.subTest(family=family, jobs=jobs):
                    stated = (
                        f"'SET pgdump.memory = {allowance}'"
                        if family.startswith(shapes.PARALLEL_SCAN)
                        else f"--memory {allowance}"
                    )
                    self.assertIn(stated, shapes._script(f"{family}{jobs}"))

    def test_the_budget_admits_the_widest_row_on_the_coarser_leg(self):
        # A block-decoding source charges each reader its block, the chunk
        # buffer and the decoder's own retention, and the readers together the
        # block pool's retention list — `WorkerMemory::at`, which
        # `worker_count` solves the stated bytes against. A budget below that
        # at the widest row clamps the top rows silently, which 1 GiB would.
        block = 24 * config.MIB
        # `xz_seek::Reader::decode_footprint` on this shape: an 8 MiB LZMA2
        # dictionary, the 1 MiB input chunk, and `liblzma`'s 34,592 B of state.
        decoder = 8 * config.MIB + throughput.CHUNK_DEFAULT + 34_592
        jobs = shapes.PARALLEL_JOBS[-1]
        want = jobs * (block + throughput.CHUNK_DEFAULT + decoder) + (
            max(memory_model.LIBRARY_POOL_DEPTH, jobs) - 1
        ) * block
        self.assertGreaterEqual(shapes.PARALLEL_BUDGET, want)
        self.assertLess(1 << 30, want, "1 GiB would clamp the widest row")

    def test_the_container_is_the_budget_plus_a_fixed_headroom(self):
        # `container > budget` holds against a literal `3g` whatever the budget
        # does: raising the budget 1 GiB -> 2 GiB under it halved the headroom
        # and still passed. Block-pool retention is a function of the budget,
        # so the widest `control_xz128` row went from 2110 MiB to 3067 MiB
        # against a 3072 MiB limit. Pin the rule, not the inequality.
        self.assertTrue(shapes.PARALLEL_MEMORY.endswith("g"))
        self.assertEqual(
            int(shapes.PARALLEL_MEMORY[:-1]) * config.GIB,
            shapes.PARALLEL_BUDGET + shapes.PARALLEL_HEADROOM,
        )
        self.assertNotEqual(shapes.PARALLEL_MEMORY, config.Config().memory)

    def test_the_headroom_clears_the_widest_measured_row(self):
        # The reading that forced the rule: `control_xz128` at `--jobs 24` and a
        # 2 GiB budget peaked at 3067 MiB, of which 2 GiB is the budget itself.
        # A headroom below what the unbudgeted terms actually cost buys an OOM
        # an hour into a sweep rather than a reading.
        measured_above_budget = 3067 * config.MIB - (2 << 30)
        self.assertGreater(shapes.PARALLEL_HEADROOM, measured_above_budget)

    def test_both_figures_declare_that_departure(self):
        for fid in ("parallel-scan-throughput", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                self.assertEqual(
                    register.SELECTABLE_BY_ID[fid].memory, shapes.PARALLEL_MEMORY
                )

    def test_the_baseline_row_is_one_job(self):
        # `--jobs 1` is `Parallelism::Serial` — the serial code path this
        # project ships, which is what a speedup is a speedup over.
        self.assertEqual(shapes.PARALLEL_BASELINE, 1)
        self.assertEqual(shapes.PARALLEL_JOBS[0], shapes.PARALLEL_BASELINE)

    def test_the_range_runs_past_the_plain_path_ceiling(self):
        # `POOL_DEPTH` clamps the chunk pool to four slots, so a fifth fused
        # worker on a plain source waits. A table stopping at four would leave a
        # reader to infer the scan stopped scaling.
        self.assertGreater(shapes.PARALLEL_JOBS[-1], 4)
        self.assertIn(4, shapes.PARALLEL_JOBS)

    def test_the_counts_match_the_decode_figures(self):
        # `xz-decode-scaling` is the floor these are read against: it says what
        # the decoder alone does with N workers, and a row with no counterpart
        # there is a comparison nobody can make.
        self.assertEqual(shapes.PARALLEL_JOBS, shapes.DECODE_WORKERS)

    def test_a_compressed_legs_rate_is_per_plaintext_byte(self):
        # `control_xz` is a compression of `control`, so the plaintext volume is
        # a registered input's own size. Dividing by the compressed size would
        # state a rate five times too low under a heading that reads like the
        # plain leg's.
        for inp, _, _ in parallel.PARALLEL_LEGS:
            with self.subTest(input=inp):
                plaintext = parallel.PARALLEL_PLAINTEXT[inp]
                self.assertIn(plaintext, inputs.INPUTS)
                self.assertEqual(inputs.INPUTS[plaintext].suffix, ".sql")
        self.assertEqual(parallel.PARALLEL_PLAINTEXT["control_xz"], "control")

    def test_a_compressed_leg_derives_from_the_plaintext_it_is_divided_by(self):
        # Not merely "some plain input of the same nominal size": the two must
        # be the same bytes, or the rate is per a volume the run never decoded.
        for inp, _, _ in parallel.PARALLEL_LEGS:
            spec = inputs.INPUTS[inp]
            if spec.suffix == ".xz":
                with self.subTest(input=inp):
                    self.assertEqual(spec.derives_from, parallel.PARALLEL_PLAINTEXT[inp])

    def test_both_axes_are_crossed_in_the_throughput_legs(self):
        # Plain against `.xz` is whether a decoder is in front of the scan;
        # `parse` against a typed `query` is discovery against extraction. The
        # phase's rule is one line over those two, and a table missing a column
        # would confirm whichever half it kept.
        self.assertEqual(
            sorted({(inp, cmd) for inp, cmd, _ in parallel.PARALLEL_LEGS}),
            sorted(
                (inp, cmd)
                for inp in ("control", "control_xz")
                for cmd in ("parse", shapes.PARALLEL_SCAN)
            ),
        )

    def test_the_rss_legs_differ_only_in_block_size(self):
        # The figure's whole content is that a compressed reader's per-worker
        # footprint is one decoded block, so a second variable between the legs
        # would make the two columns incomparable.
        legs = [inputs.INPUTS[leg] for leg, _ in parallel.PARALLEL_RSS_LEGS]
        self.assertEqual({s.derives_from for s in legs}, {"control"})
        self.assertEqual({s.generator for s in legs}, {"generate_xz_input.py"})
        sizes = {
            s.args[s.args.index("--block-size") + 1] if "--block-size" in s.args else "24MiB"
            for s in legs
        }
        self.assertEqual(sizes, {"24MiB", "128MiB"})

    def test_the_second_block_size_is_one_the_generator_admits(self):
        # The generator refuses a size no figure asks for, so a typo here is an
        # error rather than a valid `.xz` whose table row describes a shape
        # nobody registered.
        import generate_xz_input

        spec = inputs.INPUTS["control_xz128"]
        self.assertIn(
            spec.args[spec.args.index("--block-size") + 1], generate_xz_input.BLOCK_SIZES
        )

    def test_the_compressed_inputs_have_their_own_nominal_size(self):
        cfg = config.Config()
        for leg, _ in parallel.PARALLEL_RSS_LEGS:
            with self.subTest(leg=leg):
                self.assertLess(staging.nominal_size(cfg, leg), cfg.size_gib * config.GIB)

    def test_both_figures_read_the_parallel_gate(self):
        # `warm-parallel` gates on steal alone: a reading that occupies every
        # hardware thread is busy by construction, so `warm`'s row would discard
        # every rep of both these figures. The throughput figure's cold leg
        # reads `cold-nvme-parallel`, which gates the same way.
        stages = {
            "parallel-scan-throughput": ["cold-nvme-parallel", "warm-parallel"],
            "parallel-peak-rss": ["warm-parallel"],
        }
        for fid, want in stages.items():
            with self.subTest(figure=fid):
                self.assertEqual(sorted(register.SELECTABLE_BY_ID[fid].stage.split("+")), want)
                for regime in want:
                    self.assertEqual(machine_state.CONTENTION_LIMITS[regime], {"cpu_steal_pct": 2.0})

    def test_the_declared_paths_carry_the_leader_and_the_decoder(self):
        # These are the two mechanisms `--jobs` newly reaches; a figure about
        # them that declared neither would read green across the work that moves
        # it most.
        for fid in ("parallel-scan-throughput", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                depends = register.SELECTABLE_BY_ID[fid].depends
                self.assertIn("crates/pgdt-mapping/src/interior.rs", depends)
                self.assertIn("vendor/xz-seek/src/", depends)
                self.assertIn("crates/pgdt-io/src/source/compressed/xz.rs", depends)

    def test_a_stage_selection_does_not_reach_them(self):
        for fid in ("parallel-scan-throughput", "parallel-peak-rss"):
            with self.subTest(figure=fid):
                self.assertNotIn(
                    "warm", register.SELECTABLE_BY_ID[fid].stage.split("+")
                )


class SubstreamAnnotation(unittest.TestCase):
    """The sub-stream count belongs to the two provider legs and no other.

    It is stated per cell rather than footnoted once, so a cell that carries it
    is making a claim about *that* leg. The count was once hand-applied to a
    pasted table and landed one column left of the leg it described — on
    `.xz`, `parse`, which plans no sub-streams at all — which is why the
    mapping is asserted here rather than read off the table by eye.
    """

    def test_only_provider_legs_are_annotated(self):
        scan = shapes.PARALLEL_SCAN
        annotated = {
            (inp, family) for inp, family, _ in parallel.PARALLEL_LEGS if family == scan
        }
        self.assertEqual(annotated, {("control", scan), ("control_xz", scan)})
        for inp, family, label in parallel.PARALLEL_LEGS:
            if family != scan:
                self.assertNotIn(
                    "provider", label, f"{label} is not a provider leg but reads like one"
                )

    def test_a_cap_belongs_to_a_provider_leg(self):
        # A cap for a leg nothing annotates is dead weight that reads as a
        # claim about the table. The dict may legitimately be empty — no leg's
        # count falls inside the axis at the budget stated today — so what is
        # asserted is the membership, not that anything is in it.
        typed = {
            inp for inp, family, _ in parallel.PARALLEL_LEGS if family == shapes.PARALLEL_SCAN
        }
        self.assertLessEqual(set(parallel.QUERY_SUBSTREAM_CAP), typed)

    def test_an_empty_cap_says_so_in_the_prose(self):
        # The paragraph is emitted from the dict, so the two cannot disagree:
        # empty means the note says neither leg is clamped, and an entry means
        # it names the count. The dict is not empty at today's carving, so the
        # empty branch is reached by patching one in.
        with unittest.mock.patch.object(parallel, "QUERY_SUBSTREAM_CAP", {}):
            empty = parallel._substream_note()
        self.assertIn("neither provider leg reaches it", empty)
        self.assertNotIn("state the count they actually", empty)
        with unittest.mock.patch.object(parallel, "QUERY_SUBSTREAM_CAP", {"control": 14}):
            clamped = parallel._substream_note()
        self.assertIn("`14` on plain", clamped)
        self.assertNotIn("neither provider leg reaches it", clamped)

    def test_a_cap_is_never_above_the_largest_job_count(self):
        # A cap at or above the largest `--jobs` would annotate every row with
        # its own label and say nothing — which is why a leg the budget never
        # clamps inside the axis carries no entry at all rather than a number
        # past the top of it.
        for inp, cap in parallel.QUERY_SUBSTREAM_CAP.items():
            self.assertLess(cap, shapes.PARALLEL_JOBS[-1], inp)

    def test_only_the_plain_leg_carries_a_cap_at_this_budget(self):
        # `plan_partitions` charges the held batch's span only where the source
        # retains by the read chunk, and a block-decoding `CompressedFileSource` retains by
        # the partition — so the `.xz` leg is charged what one reader holds,
        # `34.03 MiB`, plus a 24 MiB unit of the pool's retention list for each
        # reader past four, and affords twenty-nine against the margin ceiling,
        # past the top of the axis. The plain leg recommends nothing, so it is
        # handed `DEFAULT_MEMORY_BUDGET` whatever `--memory` states (`D83`):
        # 64 MiB, spent on readers first and on the span with what is left
        # (`D84`), which seats every worker up to four and stops at seven once
        # the span is on its 1 MiB floor. The table's own paragraph is emitted
        # from this dict so it cannot disagree with it.
        self.assertEqual(parallel.QUERY_SUBSTREAM_CAP, {"control": 7})


class ParallelScanThroughputColdNvme(unittest.TestCase):
    """`parallel-scan-throughput`'s cold-NVMe leg: the plain typed provider
    scan again, every rep cold off the NVMe, over a cache built in a container
    of its own.

    What every check here is against is a reading of the wrong thing that
    still looks like a cold one: a builder in the timed container reading the
    dump back into page cache after the drop, a cache an earlier sitting's
    binary left on the device, or a leg read off tmpfs under a cold heading."""

    FIGURE = "parallel-scan-throughput"
    PREFIX = f"{shapes.PARALLEL_SCAN_PREBUILT}-jobs-"

    def _cold(self):
        return [s for s in parallel._parallel_specs() if s.regime == "cold-nvme-parallel"]

    def _session(self, tmp):
        logged = []
        cfg = config.Config(
            dry_run=True,
            cache_dir=Path(tmp) / "ssd",
            nvme_dir=Path(tmp) / "nvme",
            warm_dir=Path(tmp) / "shm",
            warm_budget=8,
        )
        session = timing.Session(cfg, staging.Stager(cfg, logged.append), logged.append)
        session.figure_id = self.FIGURE
        return session, logged

    def test_the_leg_is_the_plain_provider_scan_at_the_admitted_counts(self):
        specs = self._cold()
        self.assertEqual(
            [s.command for s in specs], [f"{self.PREFIX}{n}" for n in parallel.PARALLEL_NVME_JOBS]
        )
        self.assertEqual({(s.binary, s.input) for s in specs}, {("dfcli", "control")})
        self.assertEqual(
            parallel.PARALLEL_NVME_JOBS,
            shapes.PARALLEL_JOBS[: len(parallel.PARALLEL_NVME_JOBS)],
        )
        self.assertEqual(parallel.PARALLEL_NVME_JOBS[-1], 4)
        self.assertEqual(register.FIGURES_BY_ID[self.FIGURE].nvme_inputs, ("control",))

    def test_the_regime_drops_the_page_cache_and_reads_the_nvme(self):
        self.assertEqual(
            machine_state.REGIMES["cold-nvme-parallel"], machine_state.Regime(area="nvme", drops_caches=True)
        )
        with tempfile.TemporaryDirectory() as tmp:
            session, _ = self._session(tmp)
            self.assertEqual(
                session.input_path("control", "cold-nvme-parallel").parent,
                session.cfg.nvme_dir,
            )

    def test_the_timed_container_runs_no_builder(self):
        # A `parse` after the drop reads the dump back into page cache before
        # the timer, so the query would read it warm.
        for jobs in parallel.PARALLEL_NVME_JOBS:
            with self.subTest(jobs=jobs):
                script = shapes._script(f"{self.PREFIX}{jobs}")
                warm = shapes._script(f"{shapes.PARALLEL_SCAN}-jobs-{jobs}")
                self.assertNotIn("/pgdt parse", script)
                self.assertEqual(script.count("time "), 1)
                self.assertTrue(script.startswith("time "))
                # The same timed query as the warm leg's, word for word.
                self.assertEqual(warm.partition(" && ")[2], script)

    def test_the_builder_is_the_warm_legs_but_for_where_it_writes(self):
        warm = shapes._script(f"{shapes.PARALLEL_SCAN}-jobs-1").partition(" && ")[0]
        prebuilt = register.FIGURES_BY_ID[self.FIGURE].prebuilt
        self.assertEqual(prebuilt.reads, self.PREFIX)
        built = prebuilt.builder.format(cache=f"{timing.PREBUILT_MOUNT}/dump.sql.pgdt")
        self.assertEqual(
            built.replace(f"{timing.PREBUILT_MOUNT}/dump.sql.pgdt", "/dump.sql.pgdt"),
            warm,
        )
        self.assertNotIn("time ", built)
        self.assertIn(f"--jobs {shapes.SWEEP_JOBS} ", built)
        self.assertIn(shapes.GATHER_STATISTICS, built)

    def test_a_cache_this_sitting_did_not_build_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            session, _ = self._session(tmp)
            with self.assertRaises(RuntimeError):
                session.time_run(self._cold()[0])

    def test_the_cache_is_built_once_and_mounted_read_only_beside_the_dump(self):
        with tempfile.TemporaryDirectory() as tmp:
            session, logged = self._session(tmp)
            session.prebuild(parallel._parallel_specs())
            builds = [m for m in logged if "--cache /out/control.sql.pgdt" in m]
            self.assertEqual(len(builds), 1, logged)
            self.assertNotIn("time ", builds[0])
            dump = session.input_path("control", "cold-nvme-parallel")
            self.assertIn(f"{dump}:/dump.sql:ro", builds[0])
            self.assertIn(f"{dump.parent}:/out ", builds[0])
            logged.clear()
            session.time_run(self._cold()[0])
            cache = dump.with_name(dump.name + ".pgdt")
            self.assertTrue(any(f"{cache}:/dump.sql.pgdt:ro" in m for m in logged), logged)
            self.assertTrue(any("drop_caches" in m for m in logged), logged)

    def test_a_warm_leg_is_given_no_built_cache(self):
        with tempfile.TemporaryDirectory() as tmp:
            session, logged = self._session(tmp)
            session.prebuild(parallel._parallel_specs())
            warm = [s for s in parallel._parallel_specs() if s.binary == "dfcli"][0]
            logged.clear()
            session.time_run(warm)
            self.assertFalse(any(".pgdt:/dump.sql.pgdt" in m for m in logged), logged)

    def test_the_column_stops_where_the_regime_test_does(self):
        specs = parallel._parallel_specs()
        answer = {"result_rows": "1", "result_first": "814362", "result_digest": "ab"}
        raw = {
            "readings": {s.key(self.FIGURE): [1.0] * 5 for s in specs},
            "reported": {s.key(self.FIGURE): answer for s in specs if s.binary == "dfcli"},
            "input_sizes": {"control": 3221227790, "control_xz": 591190020},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(config.Config(), raw, Path(tmp), lambda _m: None)
            session.figure_id = self.FIGURE
            body = parallel.run_parallel_scan_throughput(session)
        rows = [r for r in body.splitlines() if r.startswith("| ")]
        self.assertTrue(rows[0].rstrip().endswith(f"| {parallel.PARALLEL_NVME_LEG[2]} |"))
        for row in rows[2:]:
            cells = [c.strip() for c in row.strip("|").split("|")]
            jobs = int(cells[0].split()[0])
            with self.subTest(jobs=jobs):
                if jobs in parallel.PARALLEL_NVME_JOBS:
                    self.assertIn("MB/s", cells[-1])
                else:
                    self.assertEqual(cells[-1], "—")


class SubstreamAnnotationLandsOnTheRightColumn(unittest.TestCase):
    """The renderer itself, over synthetic readings.

    The classes above assert the *mapping*; this one asserts the table. The
    defect this pins put the `.xz` sub-stream counts in the `.xz`, `parse`
    column — every number correct, attached to the wrong leg — which no
    assertion about `PARALLEL_LEGS` alone would have caught.

    **The cap is supplied here rather than read off the shipped constant**,
    which is empty at today's budget: a renderer asserted only against the
    arrangement that prints nothing would stop covering the placement the
    moment it stopped mattering, which is exactly when a later constant brings
    it back.
    """

    CAP = {"control": 14}

    def _render(self, cap=None):
        figure = "parallel-scan-throughput"
        specs = parallel._parallel_specs()
        answer = {"result_rows": "1", "result_first": "814362", "result_digest": "ab"}
        raw = {
            "readings": {s.key(figure): [1.0, 1.0, 1.0, 1.0, 1.0] for s in specs},
            "reported": {s.key(figure): answer for s in specs if s.binary == "dfcli"},
            "input_sizes": {"control": 3221227790, "control_xz": 591190020},
            "runs": [],
        }
        with tempfile.TemporaryDirectory() as tmp:
            session = replay.ReplaySession(
                config.Config(), raw, Path(tmp), lambda _m: None
            )
            session.figure_id = figure
            with unittest.mock.patch.object(
                parallel, "QUERY_SUBSTREAM_CAP", self.CAP if cap is None else cap
            ):
                return parallel.run_parallel_scan_throughput(session)

    def test_the_shipped_cap_annotates_the_plain_provider_leg(self):
        # The plain provider leg is clamped to seven sub-streams once the
        # span is on its floor, so every one of its annotated cells carries
        # that count and no other column does. An empty dict annotates nothing
        # at all, which is what a leg no budget clamps inside the axis gets.
        body = self._render(cap=parallel.QUERY_SUBSTREAM_CAP)
        cells = [r for r in body.splitlines() if r.startswith("| ")]
        self.assertTrue(cells)
        annotated = [r for r in cells if "sub-stream" in r]
        self.assertTrue(annotated, "the shipped cap annotates nothing")
        for row in annotated:
            self.assertEqual(row.count("sub-stream"), 1, row)
            self.assertIn("7 sub-streams", row)
        with unittest.mock.patch.object(parallel, "QUERY_SUBSTREAM_CAP", {}):
            empty = self._render(cap={})
        for row in (r for r in empty.splitlines() if r.startswith("| ")):
            self.assertNotIn("sub-stream", row)

    def test_the_annotation_is_in_the_provider_columns_only(self):
        body = self._render()
        rows = [r for r in body.splitlines() if r.startswith("| ")]
        header = [c.strip() for c in rows[0].strip("|").split("|")]
        # The cold leg's rows stop at four (`PARALLEL_NVME_JOBS`), below any
        # clamp, so it is no column an annotation may land in.
        typed = {i for i, c in enumerate(header) if "provider" in c and "cold" not in c}
        self.assertEqual(len(typed), 2, header)
        # Only the legs a budget clamp actually reaches are annotated, and
        # which those are is `QUERY_SUBSTREAM_CAP`'s to say.
        capped = {
            i
            for i in typed
            if ("control_xz" if "`.xz`" in header[i] else "control") in self.CAP
        }
        self.assertTrue(capped, "no provider leg is capped, so this asserts nothing")

        annotated_columns = set()
        for row in rows[2:]:  # skip header and the |---| separator
            cells = [c.strip() for c in row.strip("|").split("|")]
            for i, cell in enumerate(cells):
                if "sub-stream" in cell:
                    annotated_columns.add(i)
        self.assertEqual(
            annotated_columns,
            capped,
            "the sub-stream count must appear in the capped provider columns and no others",
        )

    def test_rows_at_or_below_four_carry_no_annotation(self):
        body = self._render()
        for row in body.splitlines():
            if row.startswith("| 1 ") or row.startswith("| 2 ") or row.startswith("| 4 "):
                self.assertNotIn("sub-stream", row)

    def test_an_annotated_cell_states_the_lesser_of_jobs_and_the_cap(self):
        body = self._render()
        header = None
        for row in body.splitlines():
            if not row.startswith("| "):
                continue
            cells = [c.strip() for c in row.strip("|").split("|")]
            if header is None:
                header = cells
                continue
            if cells[0].startswith("-") or not cells[0][0].isdigit():
                continue
            jobs = int(cells[0].split()[0])
            for i, cell in enumerate(cells):
                if "sub-stream" in cell:
                    inp = "control_xz" if "`.xz`" in header[i] else "control"
                    want = min(jobs, self.CAP[inp])
                    self.assertIn(f"· {want} sub-stream", cell)


if __name__ == "__main__":
    unittest.main()
