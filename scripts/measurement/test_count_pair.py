"""Tests of `measurement.count_pair`."""

from __future__ import annotations

import contextlib
import io
import re
import release
import subprocess
import tempfile
import unittest
import unittest.mock
from pathlib import Path

import measure
from measurement import config
from measurement import count_pair
from measurement import formatting
from measurement import host
from measurement import inputs
from measurement import parsing
from measurement import preflight
from measurement import shapes
from measurement import staging
from measurement.builds import image
from measurement.builds import leg_binaries
from measurement.figures import release_profile


class CountPair(unittest.TestCase):
    """`--count-pair`: one slice's change priced in `instructions:u`, the
    shipped build at the tree's parent against the tree, and refused for any
    other pair (`.claude/skills/evidence/SKILL.md`, rule 7)."""

    PARENT = "a" * 40

    def _cfg(self, tmp: str) -> "config.Config":
        return config.Config(
            out_dir=Path(tmp) / "runs",
            alloc_build_root=Path(tmp) / "builds",
            cache_dir=Path(tmp) / "cache",
        )

    def test_the_parent_is_head_under_an_uncommitted_change_and_its_parent_otherwise(self):
        for status, rev in ((" M crates/x.rs\n", "HEAD"), ("?? crates/new.rs\n", "HEAD"), ("", "HEAD^")):
            asked = []

            def fake_run(argv, cwd=None, capture=False, quiet=False, status=status):
                if argv[:2] == ["git", "status"]:
                    return status
                asked.append(argv[-1])
                return self.PARENT + "\n"

            with self.subTest(status=status), unittest.mock.patch.object(host, "run", fake_run):
                self.assertEqual(count_pair.count_pair_parent(), (self.PARENT, bool(status)))
                self.assertEqual(asked, [rev])

    def test_only_the_parent_is_accepted(self):
        def resolving(to):
            return lambda argv, cwd=None, capture=False, quiet=False: to + "\n"

        with unittest.mock.patch.object(host, "run", resolving(self.PARENT)):
            self.assertIsNone(count_pair.count_pair_refusal("HEAD~1", self.PARENT, False))
        with unittest.mock.patch.object(host, "run", resolving("b" * 40)):
            refusal = count_pair.count_pair_refusal("HEAD~2", self.PARENT, False)
        self.assertIn("parent is aaaaaaaaaa", refusal)

        def missing(argv, cwd=None, capture=False, quiet=False):
            raise subprocess.CalledProcessError(1, argv)

        with unittest.mock.patch.object(host, "run", missing):
            self.assertIn("names no commit", count_pair.count_pair_refusal("nope", self.PARENT, True))

    def test_a_moved_toolchain_or_image_is_refused(self):
        here = {"Dockerfile": b"FROM x", "rust-toolchain.toml": b"1.90", "mise.toml": b"m"}
        with unittest.mock.patch.object(release, "image_inputs", return_value=here):
            with unittest.mock.patch.object(count_pair, "image_inputs_at", return_value=dict(here)):
                self.assertIsNone(count_pair.count_pair_image_refusal(self.PARENT))
            moved = {**here, "rust-toolchain.toml": b"1.89"}
            with unittest.mock.patch.object(count_pair, "image_inputs_at", return_value=moved):
                self.assertIn("rust-toolchain.toml", count_pair.count_pair_image_refusal(self.PARENT))

    def test_the_image_inputs_at_a_commit_are_the_ones_release_reads(self):
        at = count_pair.image_inputs_at("HEAD")
        self.assertEqual(set(at), set(release.image_inputs()))

    def test_the_shapes_are_the_spec_s_five_a_nested_decode_and_a_long_field_s_parses(self):
        rows = [(command, name) for command, _, _, name in count_pair.COUNT_PAIR_SHAPES]
        control = count_pair.COUNT_PAIR_INPUT
        self.assertEqual(
            rows,
            [
                ("parse", control),
                ("parse-statistics-data", control),
                ("query-strings", control),
                ("query-typed", control),
                (shapes.PARALLEL_SCAN, control),
                ("query-typed", "arrays"),
                ("parse-statistics-data", "long_escaped_32"),
                (shapes.STRICT_PARSE, "long_escaped_32"),
                ("parse-statistics-data", "long_bytea_32"),
                (shapes.STRICT_PARSE, "long_bytea_32"),
            ],
        )
        # Each long row reads a `long-line-resident` input of one 32 MiB line
        # kind, past a chunk and under the line limit the bound replaced.
        for name, kind in (("long_escaped_32", "escaped"), ("long_bytea_32", "bytea")):
            spec = inputs.INPUTS[name]
            self.assertEqual(spec.generator, "generate_long_lines.py")
            self.assertIn(kind, spec.args)
            self.assertIn(str(32 * config.MIB), spec.args)
        self.assertLess(32 * config.MIB, 64 << 20)
        # The control holds no nested column; the one row decoding one reads
        # an input generated with them.
        self.assertIn("--arrays", inputs.INPUTS["arrays"].args)
        self.assertNotIn("--arrays", inputs.INPUTS[control].args)
        labels = [label for _, label, _, _ in count_pair.COUNT_PAIR_SHAPES]
        self.assertEqual(len(set(labels)), len(labels))
        rows_named = [count_pair.count_pair_row(c, n) for c, n in rows]
        self.assertEqual(len(set(rows_named)), len(rows_named))
        commands = [command for command, _ in rows]
        shapes_ = set(shapes.command_shapes())
        for command in commands:
            with self.subTest(command=command):
                self.assertIn(command, shapes_)
                # Raises unless exactly one timed `/pgdt` goes under the counter.
                parsing.counted_script(shapes._script(command))
        self.assertIn(shapes.NO_STATISTICS, shapes._script("parse"))
        gathering = shapes._script("parse-statistics-data")
        self.assertIn(shapes.GATHER_STATISTICS, gathering)
        self.assertNotIn(parsing.PEAK_RSS, gathering)
        # The strict row is that parse told to check every field, and nothing
        # else.
        strict = shapes._script(shapes.STRICT_PARSE)
        self.assertEqual(
            strict.replace(f" {shapes.STRICT_FLAG}", ""), gathering
        )
        self.assertIn("--postgres-invalid-values strict", strict)

    def test_the_data_level_runs_under_the_statistics_figure_s_limit(self):
        # Every row gathering statistics, the strict parse's included, under
        # that figure's limit, since its allowance is discovered from it (D85).
        for command, label, memory, _ in count_pair.COUNT_PAIR_SHAPES:
            gathers = command in ("parse-statistics-data", shapes.STRICT_PARSE)
            with self.subTest(row=label):
                self.assertEqual(memory, shapes.STATISTICS_MEMORY if gathers else None)

    def test_a_run_is_the_counted_shape_in_the_register_s_image(self):
        cfg = config.Config()
        argv = count_pair.count_pair_argv(
            cfg, Path("/b/pgdt-x"), Path("/c/counter"), Path("/d/control.sql"), "query-typed", None
        )
        joined = " ".join(argv)
        self.assertIn(f"-m {cfg.memory} --memory-swap {cfg.memory}", joined)
        self.assertIn("--cap-add PERFMON", joined)
        self.assertIn("-v /b/pgdt-x:/pgdt:ro", joined)
        self.assertIn(f"-v /c/counter:{parsing.COUNT_INSTRUCTIONS}:ro", joined)
        self.assertIn("-v /d/control.sql:/dump.sql:ro", joined)
        self.assertEqual(argv[-4:-1], [cfg.image, "bash", "-c"])
        self.assertEqual(argv[-1], parsing.counted_script(shapes._script("query-typed")))
        wide = count_pair.count_pair_argv(cfg, Path("/b"), Path("/c"), Path("/d"), "parse", "2g")
        self.assertIn("-m 2g --memory-swap 2g", " ".join(wide))

    def test_a_leg_naming_another_profile_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            cfg = self._cfg(tmp)

            def built(_cfg, build, target_dir=None, tree=config.REPO):
                self.assertEqual(build, release.Build())
                self.assertEqual(target_dir, image.image_target_dir(cfg, "count-pair-parent"))
                self.assertEqual(tree, Path(tmp) / "wt")
                out = Path(tmp) / "built"
                out.write_text("")
                return out

            with unittest.mock.patch.object(image, "build_in_image", built):
                with unittest.mock.patch.object(leg_binaries, "binary_profile", return_value="release"):
                    out = count_pair.count_pair_binary(cfg, "parent", Path(tmp) / "wt", lambda _: None)
                self.assertEqual(out, cfg.out_dir / "pgdt-count-pair-parent")
                with unittest.mock.patch.object(leg_binaries, "binary_profile", return_value="release-thin"):
                    with self.assertRaises(RuntimeError):
                        count_pair.count_pair_binary(cfg, "parent", Path(tmp) / "wt", lambda _: None)
                self.assertFalse(out.exists())

    def test_a_row_resolves_only_where_one_leg_s_fewest_clears_the_other_s_most(self):
        self.assertEqual(count_pair.count_pair_resolution([10, 12], [13, 14]), "resolved, a rise")
        self.assertEqual(count_pair.count_pair_resolution([13, 14], [10, 12]), "resolved, a fall")
        # Meeting at one value is not resolved, either way.
        self.assertEqual(count_pair.count_pair_resolution([10, 12], [12, 14]), "unresolved")
        self.assertEqual(count_pair.count_pair_resolution([10, 14], [11, 12]), "unresolved")

    def test_resolved_is_the_rule_downstream_verdict_reads(self):
        self.assertTrue(formatting.resolved_above([3, 4], [1, 2]))
        self.assertFalse(formatting.resolved_above([2, 4], [1, 2]))
        verdict = release_profile.downstream_verdict("x", ([2.0, 2.1], [1.0, 1.1]), ([30, 31], [10, 11]))
        self.assertIn("`instructions:u` +", verdict)
        self.assertIn("resolved", verdict)
        self.assertNotIn("unresolved", verdict)

    def test_the_table_carries_each_leg_s_range_the_change_and_the_resolution(self):
        counts = {
            label: {"parent": [1_000_000, 1_000_200], "tree": [1_000_100, 1_000_300]}
            for _, label, _, _ in count_pair.COUNT_PAIR_SHAPES
        }
        counts["`query --schema-mode typed`"]["tree"] = [1_010_000, 1_010_400]
        table = count_pair.count_pair_table(counts)
        self.assertIn(
            "| `query --schema-mode typed` | 1,000,000–1,000,200 | 1,010,000–1,010,400 "
            "| +10,100 | +1.010% | resolved, a rise |",
            table,
        )
        self.assertIn(
            "| `pgdt parse` at the metadata level | 1,000,000–1,000,200 | 1,000,100–1,000,300 "
            "| +100 | +0.010% | unresolved |",
            table,
        )
        # The same shape over another input is its own row.
        self.assertIn(
            "| `query --schema-mode typed` over `arrays` | 1,000,000–1,000,200 | "
            "1,000,100–1,000,300 | +100 | +0.010% | unresolved |",
            table,
        )

    def _pair(self, tmp, answer, rev="HEAD", named=None, spanned=None):
        """`cmd_count_pair` with every outside effect stood in: each run's
        count, and the answer `answer(leg)` gives on its stdout. `named` is
        what `count_pair_named` returns, and `spanned` what
        `count_span_named` does, which takes `rev` as a span."""
        cfg = self._cfg(tmp)
        ran, out = [], io.StringIO()
        self.mounted = []
        self.built = []

        def fake_subprocess(argv, **_kw):
            joined = " ".join(argv)
            leg = "parent" if "pgdt-count-pair-parent:/pgdt:ro" in joined else "tree"
            ran.append(leg)
            self.mounted.append(re.search(r"-v (\S+):/dump\.sql:ro", joined).group(1))
            count = (1000 if leg == "parent" else 1001) + ran.count(leg)
            return subprocess.CompletedProcess(argv, 0, answer(leg), f"instructions_u={count}\n")

        def fake_binary(_cfg, leg, tree, _log, target=None):
            self.built.append((leg, tree, target))
            return cfg.out_dir / f"pgdt-count-pair-{leg}"

        git = []
        stager = unittest.mock.Mock()
        stager.return_value.cold_path.side_effect = lambda name: Path(tmp) / f"{name}.sql"
        patches = (
            unittest.mock.patch.object(count_pair, "count_pair_parent", return_value=(self.PARENT, True)),
            unittest.mock.patch.object(count_pair, "count_pair_refusal", return_value=None),
            unittest.mock.patch.object(count_pair, "count_pair_image_refusal", return_value=None),
            unittest.mock.patch.object(count_pair, "count_pair_named", return_value=named),
            unittest.mock.patch.object(count_pair, "count_span_named", return_value=spanned),
            unittest.mock.patch.object(preflight, "counter_preflight", return_value=[]),
            unittest.mock.patch.object(
                image, "ensure_count_instructions_binary", return_value=Path("/c")
            ),
            unittest.mock.patch.object(staging, "Stager", stager),
            unittest.mock.patch.object(count_pair, "count_pair_binary", fake_binary),
            unittest.mock.patch.object(host, "run", lambda argv, **_kw: git.append(argv) or ""),
            unittest.mock.patch.object(subprocess, "run", fake_subprocess),
        )
        with contextlib.ExitStack() as stack, contextlib.redirect_stdout(out):
            for patch in patches:
                stack.enter_context(patch)
            code = count_pair.cmd_count_pair(rev, cfg, span=spanned is not None)
        return code, ran, git, out.getvalue()

    def test_each_shape_is_counted_release_profile_s_reps_on_each_leg_and_the_worktree_goes(self):
        reps = release_profile.RELEASE_PROFILE_REPS
        with tempfile.TemporaryDirectory() as tmp:
            code, ran, git, out = self._pair(tmp, lambda _leg: "")
        self.assertEqual(code, 0)
        # Both legs every rep, the one going first reversed for the second half.
        first = (reps + 1) // 2
        one_shape = ["parent", "tree"] * first + ["tree", "parent"] * (reps - first)
        self.assertEqual(ran, one_shape * len(count_pair.COUNT_PAIR_SHAPES))
        self.assertEqual([argv[:3] for argv in git], [["git", "worktree", "add"], ["git", "worktree", "remove"]])
        # The stand-in's counts climb by one a run, so the legs' ranges meet.
        self.assertIn("| unresolved |", out)
        self.assertIn(f"0 of {len(count_pair.COUNT_PAIR_SHAPES)} rows resolved.", out)
        self.assertIn("under the 10-minute line", out)
        # Each row reads its own input, mounted as the dump on both legs.
        expected = [
            Path(tmp) / f"{name}.sql"
            for *_, name in count_pair.COUNT_PAIR_SHAPES
            for _ in range(2 * reps)
        ]
        self.assertEqual(self.mounted, [str(p) for p in expected])
        self.assertEqual(self.built[-1], ("tree", config.REPO, None))

    def test_a_named_change_builds_both_legs_from_checkouts(self):
        change = "c" * 40
        with tempfile.TemporaryDirectory() as tmp:
            with unittest.mock.patch.object(release_profile, "RELEASE_PROFILE_REPS", 1):
                code, _, git, out = self._pair(
                    tmp, lambda _leg: "", rev="p..c", named=(self.PARENT, change, None)
                )
            runs = Path(tmp) / "runs"
        self.assertEqual(code, 0)
        self.assertEqual(
            [(argv[2], argv[-1] if argv[2] == "add" else Path(argv[-1]).name) for argv in git],
            [
                ("add", self.PARENT), ("remove", f"count-pair-worktree-{self.PARENT[:10]}"),
                ("add", change), ("remove", f"count-pair-worktree-{change[:10]}"),
            ],
        )
        self.assertEqual(
            self.built,
            [
                ("parent", runs / f"count-pair-worktree-{self.PARENT[:10]}", None),
                # Never where the checkout's tree leg builds, whose older
                # files would read a checkout's build as fresh.
                ("tree", runs / f"count-pair-worktree-{change[:10]}", count_pair.COUNT_PAIR_CHANGE_TARGET),
            ],
        )
        self.assertIn(f"count pair: {self.PARENT[:10]} against {change[:10]}", out)

    def test_a_named_pair_is_refused_unless_one_commit_apart(self):
        parent, change, other = "a" * 40, "c" * 40, "b" * 40

        def resolving(table):
            def fake(argv, cwd=None, capture=False, quiet=False):
                rev = argv[-1].removesuffix("^{commit}")
                if rev not in table:
                    raise subprocess.CalledProcessError(1, argv)
                return table[rev] + "\n"
            return fake

        names = {"p": parent, "c": change, f"{change}^": parent, "o": other}
        with unittest.mock.patch.object(host, "run", resolving(names)):
            self.assertEqual(count_pair.count_pair_named("p", "c"), (parent, change, None))
            refusal = count_pair.count_pair_named("o", "c")[2]
            self.assertIn(f"whose parent is {parent[:10]}", refusal)
            self.assertIn("prices every commit between", refusal)
            self.assertIn("names no commit", count_pair.count_pair_named("p", "nope")[2])
        root = {"p": parent, "c": change}
        with unittest.mock.patch.object(host, "run", resolving(root)):
            self.assertIn("whose parent is none", count_pair.count_pair_named("p", "c")[2])

    def test_a_named_pair_is_refused_where_either_commit_moves_the_toolchain(self):
        refusals = {"a" * 40: None, "c" * 40: "moved"}
        with unittest.mock.patch.object(
            count_pair, "count_pair_named", return_value=("a" * 40, "c" * 40, None)
        ), unittest.mock.patch.object(
            count_pair, "count_pair_image_refusal", side_effect=refusals.get
        ), unittest.mock.patch.object(count_pair, "count_pair_binary") as binary, \
             contextlib.redirect_stderr(io.StringIO()) as err:
            self.assertEqual(count_pair.cmd_count_pair("p..c", config.Config()), 2)
        binary.assert_not_called()
        self.assertIn("moved", err.getvalue())

    def test_a_span_is_refused_unless_its_base_is_on_its_tip_s_first_parent_chain(self):
        base, mid, tip, side = "a" * 40, "b" * 40, "c" * 40, "d" * 40
        names = {"base": base, "mid": mid, "tip": tip, "side": side}

        def fake(argv, cwd=None, capture=False, quiet=False):
            if argv[:2] == ["git", "rev-list"]:
                self.assertIn("--first-parent", argv)
                return {tip: f"{tip}\n{mid}\n{base}\n", mid: f"{mid}\n{base}\n"}[argv[-1]]
            rev = argv[-1].removesuffix("^{commit}")
            if rev not in names:
                raise subprocess.CalledProcessError(1, argv)
            return names[rev] + "\n"

        with unittest.mock.patch.object(host, "run", fake):
            # Two commits landed after the base: the span sums both.
            self.assertEqual(count_pair.count_span_named("base", "tip"), (base, tip, 2, None))
            self.assertEqual(count_pair.count_span_named("mid", "tip"), (mid, tip, 1, None))
            for older, newer in (("side", "tip"), ("tip", "mid"), ("tip", "tip")):
                with self.subTest(span=f"{older}..{newer}"):
                    refusal = count_pair.count_span_named(older, newer)[3]
                    self.assertIn("first-parent chain", refusal)
            self.assertIn("names no commit", count_pair.count_span_named("base", "nope")[3])

    def test_a_span_builds_both_legs_from_checkouts_and_reads_as_an_account(self):
        base, tip = "a" * 40, "c" * 40
        with tempfile.TemporaryDirectory() as tmp:
            with unittest.mock.patch.object(release_profile, "RELEASE_PROFILE_REPS", 1):
                code, _, git, out = self._pair(
                    tmp, lambda _leg: "", rev="base..tip", spanned=(base, tip, 58, None)
                )
            runs = Path(tmp) / "runs"
        self.assertEqual(code, 0)
        self.assertEqual([argv[-1] for argv in git if argv[2] == "add"], [base, tip])
        self.assertEqual(
            self.built,
            [
                ("parent", runs / f"count-pair-worktree-{base[:10]}", None),
                ("tree", runs / f"count-pair-worktree-{tip[:10]}", count_pair.COUNT_PAIR_CHANGE_TARGET),
            ],
        )
        self.assertIn(f"count pair: {base[:10]} against {tip[:10]}, an account over 58 commits", out)
        self.assertIn("An account over 58 commits, not one change's price", out)

    def test_a_refused_span_builds_nothing(self):
        for rev, spanned, said in (
            ("tip", ("", "", 0, None), "no <base>..<tip>"),
            ("side..tip", ("d" * 40, "c" * 40, 0, "not on the chain"), "not on the chain"),
        ):
            with self.subTest(rev=rev), unittest.mock.patch.object(
                count_pair, "count_span_named", return_value=spanned
            ), unittest.mock.patch.object(
                count_pair, "count_pair_image_refusal", return_value=None
            ), unittest.mock.patch.object(count_pair, "count_pair_binary") as binary, \
                 contextlib.redirect_stderr(io.StringIO()) as err:
                self.assertEqual(count_pair.cmd_count_pair(rev, config.Config(), span=True), 2)
                binary.assert_not_called()
                self.assertIn("--count-span refused", err.getvalue())
                self.assertIn(said, err.getvalue())

    def test_a_resolved_row_is_named_after_the_table(self):
        with tempfile.TemporaryDirectory() as tmp:
            with unittest.mock.patch.object(release_profile, "RELEASE_PROFILE_REPS", 1):
                code, ran, _, out = self._pair(tmp, lambda _leg: "")
        self.assertEqual(code, 0)
        self.assertEqual(ran, ["parent", "tree"] * len(count_pair.COUNT_PAIR_SHAPES))
        self.assertIn("| resolved, a rise |", out)
        self.assertIn(f"{len(count_pair.COUNT_PAIR_SHAPES)} of {len(count_pair.COUNT_PAIR_SHAPES)} rows resolved: `pgdt parse` at the metadata level,", out)

    def test_a_shape_answering_differently_on_the_two_legs_compares_nothing(self):
        with tempfile.TemporaryDirectory() as tmp:
            code, _, git, out = self._pair(tmp, lambda leg: f"result_rows={1 if leg == 'parent' else 2}\n")
        self.assertEqual(code, 1)
        self.assertIn("answered differently", out)
        self.assertNotIn("| median change |", out)
        self.assertEqual(git[-1][:3], ["git", "worktree", "remove"])

    def test_a_refused_pair_builds_nothing(self):
        with unittest.mock.patch.object(count_pair, "count_pair_parent", return_value=(self.PARENT, False)), \
             unittest.mock.patch.object(count_pair, "count_pair_refusal", return_value="not the parent"), \
             unittest.mock.patch.object(count_pair, "count_pair_image_refusal", return_value=None), \
             unittest.mock.patch.object(count_pair, "count_pair_binary") as binary, \
             contextlib.redirect_stderr(io.StringIO()) as err:
            self.assertEqual(count_pair.cmd_count_pair("HEAD~3", config.Config()), 2)
        binary.assert_not_called()
        self.assertIn("not the parent", err.getvalue())

    def test_the_flag_reaches_the_command(self):
        with unittest.mock.patch.object(count_pair, "cmd_count_pair", return_value=0) as cmd:
            self.assertEqual(measure.main(["--count-pair", "HEAD~1", "--dry-run"]), 0)
        rev, cfg = cmd.call_args.args
        self.assertEqual(rev, "HEAD~1")
        self.assertTrue(cfg.dry_run)
        self.assertEqual(cmd.call_args.kwargs, {})
        with unittest.mock.patch.object(count_pair, "cmd_count_pair", return_value=0) as cmd:
            self.assertEqual(measure.main(["--count-span", "a..b"]), 0)
        self.assertEqual(cmd.call_args.args[0], "a..b")
        self.assertEqual(cmd.call_args.kwargs, {"span": True})


if __name__ == "__main__":
    unittest.main()
