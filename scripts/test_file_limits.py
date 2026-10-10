"""`file_limits.py`'s own rules: what is masked, what counts, what is red."""

from __future__ import annotations

import io
import tempfile
import textwrap
import unittest
import unittest.mock
from pathlib import Path

import file_limits as fl


def masked_lines(src: str) -> list[str]:
    return fl.masked(textwrap.dedent(src)).split("\n")


class Masking(unittest.TestCase):
    def test_comments_are_blanked_and_lines_kept(self):
        got = fl.masked("a // one\n/* two\nthree */ b\n/// doc\n")
        self.assertEqual(got, "a       \n      \n         b\n       \n")

    def test_block_comments_nest(self):
        self.assertEqual(fl.masked("/* a /* b */ c */ x").strip(), "x")

    def test_string_contents_are_filled_and_braces_in_them_hidden(self):
        self.assertEqual(fl.masked('f("{ fn x")'), 'f("xxxxxx")')
        self.assertEqual(fl.masked('"a\\"b"'), '"xxxx"')
        self.assertEqual(fl.masked('r#"a"b"#'), 'r#"xxx"#')
        self.assertEqual(fl.masked('b"ab"'), 'b"xx"')
        self.assertEqual(fl.masked('"a\nb"'), '"x\nx"')

    def test_an_identifier_ending_in_r_or_b_is_not_a_prefix(self):
        self.assertEqual(fl.masked('bar"x"'), 'bar"x"')

    def test_char_literals_and_lifetimes(self):
        self.assertEqual(fl.masked("'{'"), "'x'")
        self.assertEqual(fl.masked("'\\n'"), "'xx'")
        self.assertEqual(fl.masked("&'a str"), "&'a str")
        self.assertEqual(fl.masked("b'{'"), "b'x'")


class TestBlocks(unittest.TestCase):
    def test_an_inline_module_is_a_line_range(self):
        src = """\
            fn a() {}
            #[cfg(test)]
            mod tests {
                use super::*;
                #[test]
                fn t() { assert!("}".len() == 1); }
            }
            fn b() {}
            """
        ranges, declared = fl.test_blocks(fl.masked(textwrap.dedent(src)))
        self.assertEqual(ranges, [(1, 6)])
        self.assertEqual(declared, [])

    def test_a_declared_module_is_a_name(self):
        ranges, declared = fl.test_blocks(fl.masked("#[cfg(test)]\nmod tests;\n#[cfg(test)]\n#[allow(x)]\nmod prior_shape;\n"))
        self.assertEqual(ranges, [])
        self.assertEqual(declared, ["tests", "prior_shape"])

    def test_cfg_test_on_an_item_is_not_a_block(self):
        ranges, declared = fl.test_blocks(fl.masked("#[cfg(test)]\nuse x::y;\nfn f() {}\n"))
        self.assertEqual((ranges, declared), ([], []))

    def test_declared_files_resolve_beside_a_root_and_under_a_module(self):
        self.assertEqual(
            fl.declared_test_files(Path("c/src/foo.rs"), ["tests"]), [Path("c/src/foo/tests.rs")]
        )
        self.assertEqual(
            fl.declared_test_files(Path("c/src/main.rs"), ["tests"]), [Path("c/src/tests.rs")]
        )


class Readings(unittest.TestCase):
    def reading(self, src: str, is_test: bool = False) -> fl.Reading:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "m.rs"
            path.write_text(textwrap.dedent(src), encoding="utf-8")
            return fl.read(path, "m.rs", is_test)

    def test_code_lines_exclude_comments_blanks_and_tests(self):
        r = self.reading(
            """\
            //! doc
            use x;

            /// doc
            pub fn a() -> fn(i32) -> i32 { |v| v }
            impl T { fn b(&self) {} }
            #[cfg(test)]
            mod tests {
                #[test]
                fn t() {}
            }
            """
        )
        self.assertEqual((r.total, r.code, r.fns, r.test_lines), (11, 3, 2, 5))

    def test_a_fn_in_a_string_is_not_counted(self):
        r = self.reading('fn a() { let s = "fn b"; }\n')
        self.assertEqual(r.fns, 1)

    def test_a_test_file_has_no_production_code(self):
        r = self.reading("fn a() {}\nfn b() {}\n", is_test=True)
        self.assertEqual((r.code, r.fns, r.test_lines), (0, 0, 2))


def reading(path: str, code: int = 0, fns: int = 0, total: int = 0, is_test: bool = False):
    return fl.Reading(path, total or code, code, fns, 0, is_test)


class Problems(unittest.TestCase):
    def test_the_limits_are_the_first_pass(self):
        self.assertEqual(
            (fl.CODE_SOFT, fl.CODE_HARD, fl.FNS_SOFT, fl.FNS_HARD, fl.TOTAL_HARD),
            (1000, 2500, 40, 100, 7500),
        )

    @unittest.mock.patch.dict(fl.EXEMPT, {}, clear=True)
    def test_soft_and_hard_lines(self):
        hard, soft, exempt, stale = fl.problems(
            [
                reading("a.rs", code=fl.CODE_SOFT + 1),
                reading("b.rs", code=fl.CODE_HARD + 1, fns=fl.FNS_SOFT + 1),
                reading("c.rs", fns=fl.FNS_HARD + 1),
                reading("d.rs", code=10),
                reading("t.rs", code=fl.CODE_HARD + 1, is_test=True),
                reading("u.rs", total=fl.TOTAL_HARD + 1, is_test=True),
            ]
        )
        self.assertEqual(
            hard,
            [
                f"!! b.rs: {fl.CODE_HARD + 1} code lines against {fl.CODE_HARD}, "
                f"{fl.FNS_SOFT + 1} functions against {fl.FNS_SOFT}",
                f"!! c.rs: {fl.FNS_HARD + 1} functions against {fl.FNS_HARD}",
                f"!! u.rs: {fl.TOTAL_HARD + 1} lines against {fl.TOTAL_HARD}",
            ],
        )
        self.assertEqual(soft, [f".. a.rs: {fl.CODE_SOFT + 1} code lines against {fl.CODE_SOFT}"])
        self.assertEqual((exempt, stale), ([], []))

    def test_an_exemption_is_reported_and_a_stale_one_fails(self):
        with unittest.mock.patch.dict(fl.EXEMPT, {"x.rs": "why", "y.rs": "why", "z.rs": "gone"}, clear=True):
            hard, soft, exempt, stale = fl.problems(
                [reading("x.rs", code=fl.CODE_HARD + 1), reading("y.rs", code=1)]
            )
            self.assertEqual(hard, [])
            self.assertEqual(
                exempt, [f"== x.rs: exempt, {fl.CODE_HARD + 1} code lines against {fl.CODE_HARD} -- why"]
            )
            self.assertEqual(
                stale,
                [
                    "!! stale exemption: y.rs is under every hard limit",
                    "!! stale exemption: z.rs is not in the workspace",
                ],
            )


class Workspace(unittest.TestCase):
    def make(self, tmp: str) -> Path:
        repo = Path(tmp)
        (repo / "Cargo.toml").write_text('[workspace]\nmembers = ["a", "crates/*"]\n')
        for member in ("a", "crates/b"):
            (repo / member / "src").mkdir(parents=True)
            (repo / member / "Cargo.toml").write_text("[package]\n")
        (repo / "a/src/lib.rs").write_text("mod m;\n")
        (repo / "a/src/m.rs").write_text("#[cfg(test)]\nmod tests;\nfn f() {}\n")
        (repo / "a/src/m").mkdir()
        (repo / "a/src/m/tests.rs").write_text("fn t() {}\n")
        (repo / "a/tests").mkdir()
        (repo / "a/tests/it.rs").write_text("fn t() {}\n")
        (repo / "crates/b/src/lib.rs").write_text("fn g() {}\n")
        (repo / "crates/b/target").mkdir()
        (repo / "crates/b/target/x.rs").write_text("fn h() {}\n")
        return repo

    def test_members_globs_and_test_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make(tmp)
            found = {r.path: r for r in fl.readings(repo)}
            self.assertEqual(
                sorted(found), ["a/src/lib.rs", "a/src/m.rs", "a/src/m/tests.rs", "a/tests/it.rs", "crates/b/src/lib.rs"]
            )
            self.assertTrue(found["a/src/m/tests.rs"].is_test)
            self.assertTrue(found["a/tests/it.rs"].is_test)
            self.assertFalse(found["a/src/m.rs"].is_test)
            self.assertEqual(found["a/src/m.rs"].fns, 1)

    def test_a_green_tree_exits_zero_and_prints_the_verdict(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make(tmp)
            out = io.StringIO()
            with unittest.mock.patch.dict(fl.EXEMPT, {}, clear=True):
                code = fl.check(repo, out)
            self.assertEqual(code, 0)
            self.assertEqual(out.getvalue(), "file-limits: 0 past a hard limit, 0 past a soft one, 0 exempt; green\n")

    def test_a_hard_limit_is_the_meter_and_a_stale_exemption_a_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = self.make(tmp)
            (repo / "a/src/big.rs").write_text("fn f() {}\n" * (fl.FNS_HARD + 1))
            out = io.StringIO()
            with unittest.mock.patch.dict(fl.EXEMPT, {}, clear=True):
                self.assertEqual(fl.check(repo, out), 1)
            self.assertTrue(out.getvalue().startswith("!! a/src/big.rs: "))
            self.assertTrue(out.getvalue().endswith("; seams\n"))
            out = io.StringIO()
            with unittest.mock.patch.dict(fl.EXEMPT, {"a/src/big.rs": "why", "nope.rs": "gone"}, clear=True):
                self.assertEqual(fl.check(repo, out), 2)
            self.assertIn("stale exemption: nope.rs", out.getvalue())


class RealTree(unittest.TestCase):
    def test_every_exemption_is_live(self):
        out = io.StringIO()
        self.assertIn(fl.check(fl.REPO, out), (0, 1), out.getvalue())


if __name__ == "__main__":
    unittest.main()
