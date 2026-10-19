"""`layering.py`'s own rules, each held on a workspace built to break it."""

from __future__ import annotations

import io
import tempfile
import textwrap
import unittest
from pathlib import Path

import layering as la

#: A workspace every rule holds of: each member a crate with a `lib.rs`, the
#: namespaces `NAMESPACES` lists declared and present.
BASE: dict[str, str] = {
    "pgdt-core/src/lib.rs": "pub mod instrument;\npub mod internal {}\n",
    "pgdt-core/src/instrument.rs": "pub fn allocated() {}\n",
    "pgdt-dump/src/lib.rs": "pub mod cache;\nmod scan;\n",
    "pgdt-dump/src/cache.rs": "pub struct CacheMode;\n",
    "pgdt-dump/src/scan.rs": "pub fn scan() {}\n",
    "pgdt-codec/src/lib.rs": "pub mod compare;\npub mod decode;\npub mod grammar;\npub mod key;\npub mod nested;\npub mod pgtype;\n",
    "pgdt-codec/src/compare.rs": "pub enum CompareKind {}\n",
    "pgdt-codec/src/decode.rs": "pub enum Read {}\npub(crate) mod float;\n",
    "pgdt-codec/src/decode/float.rs": "pub fn float_field() {}\n",
    "pgdt-codec/src/grammar.rs": "pub enum TextGrammar {}\n",
    "pgdt-codec/src/key.rs": "pub struct ValueKey;\n",
    "pgdt-codec/src/nested.rs": "pub struct ArrayLiteral;\n",
    "pgdt-codec/src/pgtype.rs": "pub enum TypeOutcome {}\n",
    "pgdt-mapping/src/lib.rs": "pub mod statistics;\n",
    "pgdt-mapping/src/statistics.rs": "pub struct StatisticsRequest;\n",
    "pgdt-query/tests/embedder.rs": "use pgdt_query::Dump;\n",
}


class Workspace:
    """A temporary tree of members and the metadata `cargo` would give it."""

    def __init__(self, files: dict[str, str], deps: dict[str, list[tuple[str, str | None]]]):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        for name in la.GRAPH:
            path = self.root / "crates" / name / "src" / "lib.rs"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("")
        for rel, text in {**BASE, **files}.items():
            path = self.root / "crates" / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(text))
        names = sorted({rel.split("/")[0] for rel in files} | set(la.GRAPH))
        self.meta = {
            "workspace_members": names,
            "packages": [
                {
                    "id": name,
                    "name": name,
                    "manifest_path": str(self.root / "crates" / name / "Cargo.toml"),
                    "dependencies": [
                        {"name": dep, "kind": kind} for dep, kind in deps.get(name, [])
                    ],
                }
                for name in names
            ],
        }

    def problems(self) -> list[str]:
        try:
            return la.problems(self.meta, self.root)
        finally:
            self.tmp.cleanup()


def problems(files: dict[str, str] | None = None, deps=None) -> list[str]:
    return Workspace(files or {}, deps or {}).problems()


class Graph(unittest.TestCase):
    def test_the_base_workspace_holds(self):
        self.assertEqual(problems(), [])

    def test_every_drawn_edge_holds(self):
        deps = {name: [(dep, None) for dep in allowed] for name, allowed in la.GRAPH.items()}
        self.assertEqual(problems(deps=deps), [])

    def test_an_unlisted_member_is_refused(self):
        found = problems({"pgdt-extra/src/lib.rs": ""})
        self.assertEqual(found, ["pgdt-extra is a member the graph does not list (D68)"])

    def test_an_edge_the_graph_does_not_draw_is_refused(self):
        found = problems(deps={"pgdt-codec": [("pgdt-dump", None)]})
        self.assertEqual(
            found, ["pgdt-codec depends on pgdt-dump, an edge the graph does not draw (D68)"]
        )

    def test_a_build_dependency_is_an_edge(self):
        self.assertTrue(problems(deps={"pgdt-core": [("pgdt-model", "build")]}))

    def test_a_dev_dependency_is_not_read(self):
        self.assertEqual(problems(deps={"pgdt-codec": [("pgdt-dump", "dev")]}), [])

    def test_a_library_never_depends_on_an_embedder_or_the_apparatus(self):
        self.assertTrue(problems(deps={"pgdt-query": [("namespace-init", None)]}))
        self.assertTrue(problems(deps={"pgdt-query": [("datafusion-pgdump", None)]}))

    def test_only_a_dev_dependency_reaches_the_fixtures(self):
        for name in la.GRAPH:
            if name != "pgdt-fixtures":
                self.assertTrue(problems(deps={name: [("pgdt-fixtures", None)]}), name)
                self.assertEqual(problems(deps={name: [("pgdt-fixtures", "dev")]}), [], name)


class Arrow(unittest.TestCase):
    def test_below_the_codec_no_arrow_crate(self):
        found = problems(deps={"pgdt-dump": [("arrow-schema", None)]})
        self.assertEqual(found, ["pgdt-dump depends on arrow-schema (D74)"])

    def test_the_codec_takes_the_schema_and_buffer_crates_but_not_arrow(self):
        ok = {"pgdt-codec": [("arrow-schema", None), ("arrow-buffer", None)]}
        self.assertEqual(problems(deps=ok), [])
        found = problems(deps={"pgdt-codec": [("arrow", None)]})
        self.assertEqual(found, ["pgdt-codec depends on arrow (D74)"])

    def test_the_mapping_pass_takes_the_schema_crate_but_not_arrow(self):
        self.assertEqual(problems(deps={"pgdt-mapping": [("arrow-schema", None)]}), [])
        found = problems(deps={"pgdt-mapping": [("arrow", None)]})
        self.assertEqual(found, ["pgdt-mapping depends on arrow (D74)"])

    def test_the_query_layer_takes_arrow(self):
        self.assertEqual(problems(deps={"pgdt-query": [("arrow", None)]}), [])

    def test_the_codec_names_no_arrow_buffer_item_but_i256(self):
        ok = {"pgdt-codec/src/key.rs": "use arrow_buffer::i256;\npub struct ValueKey;\n"}
        self.assertEqual(problems(ok), [])
        found = problems(
            {"pgdt-codec/src/key.rs": "use arrow_buffer::{i256, Buffer};\npub struct ValueKey;\n"}
        )
        self.assertEqual(
            found, ["crates/pgdt-codec/src/key.rs names `arrow_buffer::Buffer`, not `i256` (D74)"]
        )

    def test_a_comment_naming_an_arrow_buffer_item_is_not_a_use(self):
        ok = {"pgdt-codec/src/key.rs": "// arrow_buffer::Buffer\npub struct ValueKey;\n"}
        self.assertEqual(problems(ok), [])


class Purity(unittest.TestCase):
    def test_the_core_names_no_dependency_its_error_could_carry(self):
        found = problems({"pgdt-core/src/instrument.rs": "fn f(_: tokio::task::JoinError) {}\n"})
        self.assertEqual(found, ["crates/pgdt-core/src/instrument.rs (pgdt-core) names `tokio` (D74)"])

    def test_the_model_does_no_io(self):
        found = problems({"pgdt-model/src/lib.rs": "use std::fs;\n"})
        self.assertEqual(found, ["crates/pgdt-model/src/lib.rs (pgdt-model) names `std::fs` (D74)"])

    def test_the_codec_is_synchronous(self):
        found = problems({"pgdt-codec/src/key.rs": "pub struct ValueKey;\nasync fn f() {}\n"})
        self.assertEqual(found, ["crates/pgdt-codec/src/key.rs (pgdt-codec) names `async fn` (D74)"])

    def test_test_code_and_comments_are_not_production(self):
        files = {
            "pgdt-model/src/lib.rs": """\
                // std::fs, in a comment
                const NOTE: &str = "std::fs";
                #[cfg(test)]
                mod tests;
                #[cfg(test)]
                mod inline {
                    use std::fs;
                }
                """,
            "pgdt-model/src/tests.rs": "use std::fs;\n",
        }
        self.assertEqual(problems(files), [])

    def test_a_module_the_tree_declares_is_read(self):
        files = {"pgdt-model/src/lib.rs": "mod block;\n", "pgdt-model/src/block.rs": "use tokio;\n"}
        self.assertEqual(
            problems(files), ["crates/pgdt-model/src/block.rs (pgdt-model) names `tokio` (D74)"]
        )


class Embedders(unittest.TestCase):
    def test_an_internal_path_is_refused(self):
        for text in (
            "use pgdt_codec::internal::float_field;\n",
            "use pgdt_core::{Error, internal::Cursor};\n",
            "fn f() { pgdt_dump::internal::scan_region(); }\n",
        ):
            with self.subTest(text=text):
                found = problems({"pgdt/src/main.rs": text})
                self.assertEqual(found, ["crates/pgdt/src/main.rs (pgdt) names a crate's `internal` module"])

    def test_the_internal_macros_are_refused(self):
        for text in ("fn f() { timed!(x, {}); }\n", "use pgdt_core::row_evaluated;\n"):
            with self.subTest(text=text):
                self.assertEqual(len(problems({"datafusion-pgdump/tests/t.rs": text})), 1)

    def test_a_name_that_is_no_macro_is_not_refused(self):
        files = {
            "pgdt/src/main.rs": """\
                // timed!(x) and pgdt_core::internal, in a comment
                fn f(timed: u8) -> bool { timed != 3 }
                fn g() { let internal = 1; }
                """
        }
        self.assertEqual(problems(files), [])

    def test_the_apparatus_is_not_an_embedder(self):
        self.assertEqual(problems({"peak-rss/src/main.rs": "use pgdt_core::internal::X;\n"}), [])


class EmbedderTest(unittest.TestCase):
    WHO = "crates/pgdt-query/tests/embedder.rs (the embedder test)"

    def test_it_must_exist(self):
        ws = Workspace({}, {})
        (ws.root / "crates/pgdt-query/tests/embedder.rs").unlink()
        self.assertEqual(
            ws.problems(), ["crates/pgdt-query/tests/embedder.rs, the embedder test, is missing"]
        )

    def test_an_internal_path_or_macro_is_refused(self):
        for text, refusal in (
            ("use pgdt_codec::internal::column_of;\n", "names a crate's `internal` module"),
            ("fn f() { timed!(x, {}); }\n", "names `timed!`"),
        ):
            with self.subTest(text=text):
                found = problems({"pgdt-query/tests/embedder.rs": text})
                self.assertEqual(found, [f"{self.WHO} {refusal}"])

    def test_the_fixture_vocabulary_is_refused(self):
        found = problems({"pgdt-query/tests/embedder.rs": "use pgdt_fixtures::edge_cases;\n"})
        self.assertEqual(
            found, [f"{self.WHO} names `pgdt_fixtures`, which no embedder depends on"]
        )

    def test_a_module_it_declares_is_read(self):
        for module in ("pgdt-query/tests/helpers.rs", "pgdt-query/tests/helpers/mod.rs"):
            with self.subTest(module=module):
                files = {
                    "pgdt-query/tests/embedder.rs": "#[cfg(test)]\nmod helpers;\n",
                    module: "use pgdt_fixtures::edge_cases;\n",
                }
                self.assertEqual(
                    problems(files),
                    [f"crates/{module} (the embedder test) names `pgdt_fixtures`, which no embedder depends on"],
                )

    def test_a_comment_naming_them_is_not_a_use(self):
        text = "//! No `pgdt_fixtures`, no `pgdt_core::internal`, no `timed!`.\nuse pgdt_query::Dump;\n"
        self.assertEqual(problems({"pgdt-query/tests/embedder.rs": text}), [])

    def test_another_test_of_its_crate_is_not_held(self):
        files = {"pgdt-query/tests/cache.rs": "use pgdt_codec::internal::column_of;\nuse pgdt_fixtures::edge_cases;\n"}
        self.assertEqual(problems(files), [])


class Modules(unittest.TestCase):
    def test_a_path_attribute_on_a_library_module_is_refused(self):
        found = problems(
            {
                "pgdt-dump/src/lib.rs": 'pub mod cache;\n#[path = "scan_impl.rs"]\nmod scan;\n',
                "pgdt-dump/src/scan_impl.rs": "",
            }
        )
        self.assertEqual(found, ["crates/pgdt-dump/src/lib.rs loads a module by `#[path]`"])

    def test_a_path_attribute_in_a_library_test_module_is_refused_too(self):
        found = problems({"pgdt-io/src/tests.rs": '#[path = "x.rs"]\nmod x;\n'})
        self.assertEqual(found, ["crates/pgdt-io/src/tests.rs loads a module by `#[path]`"])

    def test_a_path_attribute_in_an_integration_test_is_refused_too(self):
        found = problems({"pgdt-query/tests/a.rs": '#[path = "x.rs"]\nmod x;\n'})
        self.assertEqual(found, ["crates/pgdt-query/tests/a.rs loads a module by `#[path]`"])

    def test_a_path_attribute_in_a_member_no_rule_reads_is_refused(self):
        found = problems({"pgdt-fixtures/src/lib.rs": '#[path = "x.rs"]\nmod x;\n'})
        self.assertEqual(found, ["crates/pgdt-fixtures/src/lib.rs loads a module by `#[path]`"])

    def test_a_public_module_that_is_no_namespace_is_refused(self):
        found = problems({"pgdt-dump/src/lib.rs": "pub mod cache;\npub mod scan;\n"})
        self.assertEqual(found, ["pgdt-dump: `pub mod scan` is not a namespace `NAMESPACES` lists"])

    def test_a_public_child_of_a_namespace_is_refused(self):
        found = problems({"pgdt-codec/src/decode.rs": "pub enum Read {}\npub mod float;\n"})
        self.assertEqual(
            found, ["crates/pgdt-codec/src/decode.rs: `pub mod float` inside namespace `decode`"]
        )

    def test_a_listed_namespace_must_exist(self):
        found = problems({"pgdt-codec/src/lib.rs": "pub mod compare;\npub mod decode;\npub mod grammar;\npub mod nested;\npub mod pgtype;\n"})
        self.assertEqual(found, ["pgdt-codec: namespace `key` has no module file"])


class OnePath(unittest.TestCase):
    def test_a_namespaced_item_at_the_root_too_is_refused(self):
        for root in (
            "pub use decode::Read;\n",
            "pub use crate::decode::{self, Read as Read};\n",
            "pub use decode::{Read};\n",
        ):
            with self.subTest(root=root):
                found = problems(
                    {"pgdt-codec/src/lib.rs": "pub mod compare;\npub mod decode;\npub mod grammar;\npub mod key;\npub mod nested;\npub mod pgtype;\n" + root}
                )
                self.assertIn("pgdt-codec: `Read` is at its root and in `decode` (one path)", found)

    def test_internal_is_not_the_root(self):
        files = {
            "pgdt-codec/src/lib.rs": """\
                pub mod compare;
                pub mod decode;
                pub mod grammar;
                pub mod key;
                pub mod nested;
                pub mod pgtype;
                pub mod internal {
                    pub use crate::decode::float::float_field;
                    pub use crate::decode::Read;
                }
                """
        }
        self.assertEqual(problems(files), [])

    def test_use_trees_name_their_leaves(self):
        self.assertEqual(la._use_names("a::{b, c::{d, e as f}, self}"), ["b", "d", "f", "a"])
        self.assertEqual(la._use_names("x::Y"), ["Y"])


class ThisRepo(unittest.TestCase):
    """The workspace in the tree, held to every rule."""

    def test_the_tree_holds(self):
        out = io.StringIO()
        self.assertEqual(la.check(la.REPO, out), 0, out.getvalue())

    def test_the_rules_have_something_to_read(self):
        meta = la.metadata(la.REPO)
        self.assertEqual(set(la.members(meta)), set(la.GRAPH))
        tree = la.module_tree(la.members(meta)["pgdt-codec"] / "src")
        self.assertIn("key.rs", {p.name for p in tree})


if __name__ == "__main__":
    unittest.main()
