"""Tests of `measurement.staleness.comments`."""

from __future__ import annotations

import unittest
import unittest.mock

from measurement.staleness import comments


class CommentOnlyCommits(unittest.TestCase):
    """The one oracle the harness decides for itself.

    It replaced three consecutive commits on `main` whose whole content was
    telling `--stale` that the commit before them had moved nothing, so what
    these tests hold is the direction it fails in: anything the scanner cannot
    place is *not* comment-only, because an oracle that has to be trusted is
    worth less than the red it clears.
    """

    def test_a_rust_doc_comment_is_a_comment(self):
        got = comments._rust_comment_lines("/// doc\n//! inner\n// plain\nfn f() {}\n")
        self.assertEqual(got, {1, 2, 3})

    def test_a_blank_line_carries_nothing(self):
        self.assertEqual(comments._rust_comment_lines("fn f() {}\n\n"), {2})

    def test_a_whole_line_block_comment_is_read(self):
        got = comments._rust_comment_lines("/*\n * prose\n */\nfn f() {}\n")
        self.assertEqual(got, {1, 2, 3})

    def test_a_block_delimiter_beside_code_defeats_the_scanner(self):
        # Where the comment starts is a lexing question -- a `/*` inside a
        # string literal reads the same -- so the file is not read at all.
        self.assertIsNone(comments._rust_comment_lines('let s = "/*";\n'))

    def test_an_unclosed_block_defeats_the_scanner(self):
        self.assertIsNone(comments._rust_comment_lines("/*\n prose\n"))

    def test_a_python_hash_and_docstring_are_comments(self):
        src = '# lead\ndef f():\n    """Doc.\n\n    More.\n    """\n    return 1\n'
        self.assertEqual(comments._python_comment_lines(src), {1, 3, 4, 5, 6})

    def test_an_assigned_triple_quote_is_a_value(self):
        # `x = """..."""` is data the program reads, not prose, so the scanner
        # refuses the file rather than excusing an edit to it.
        self.assertIsNone(comments._python_comment_lines('x = """body"""\n'))

    def test_markdown_is_comment_all_the_way_down(self):
        self.assertEqual(comments.comment_lines("docs/x.md", "a\nb\n"), {1, 2})

    def test_a_suffix_the_oracle_does_not_read_is_unanalyzable(self):
        self.assertIsNone(comments.comment_lines("Cargo.toml", "[package]\n"))

    def test_hunk_headers_give_both_sides_ranges(self):
        diff = "@@ -3,2 +3,0 @@\n@@ -10 +8,3 @@\n"
        self.assertEqual(comments.hunk_ranges(diff), ([3, 4, 10], [8, 9, 10]))

    def test_an_unresolvable_commit_is_not_comment_only(self):
        self.assertFalse(comments.comment_only_commit("no-such-rev", "crates/pgdt-io/src/lib.rs"))

    def test_a_path_the_commit_did_not_touch_is_not_comment_only(self):
        # Silence is not an excuse: saying "comment-only" about a path with no
        # hunks in it would be answering a question nobody asked.
        self.assertFalse(comments.comment_only_commit("HEAD", "docs/design/measurements.md.missing"))


if __name__ == "__main__":
    unittest.main()
