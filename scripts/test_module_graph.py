"""Unit tests for module_graph.py, run with `uv run python -m unittest test_module_graph`.

Each test builds a small workspace and the index rust-analyzer would give it,
breaks exactly one thing, and asserts the report names it. `ThisRepo` holds
the acceptances to the tree; the tree's own graph is `mise run check`'s, since
building its index is a whole-workspace analysis.
"""

from __future__ import annotations

import io
import unittest

import layering
import module_graph as mg


def sym(package: str, descriptors: str) -> str:
    return f"rust-analyzer cargo {package} 0.1.0 {descriptors}"


class Fixture:
    """Files and the occurrences an index would carry for them."""

    def __init__(self, files: dict[str, tuple[str, str]]):
        # rel -> (module, text)
        self.files = files
        self.occurrences: dict[str, list[mg.Occurrence]] = {rel: [] for rel in files}

    def _at(self, rel: str, needle: str, nth: int) -> mg.Position:
        text = self.files[rel][1]
        at = -1
        for _ in range(nth + 1):
            at = text.index(needle, at + 1)
        line = text.count("\n", 0, at)
        start = text.rfind("\n", 0, at) + 1
        return line, len(text[start:at].encode())

    def define(self, rel: str, needle: str, symbol: str, lines: tuple[int, int], nth: int = 0):
        text_lines = self.files[rel][1].split("\n")
        enclosing = ((lines[0], 0), (lines[1], len(text_lines[lines[1]].encode())))
        self.occurrences[rel].append(mg.Occurrence(self._at(rel, needle, nth), symbol, True, enclosing))
        return self

    def refer(self, rel: str, needle: str, symbol: str, nth: int = 0):
        self.occurrences[rel].append(mg.Occurrence(self._at(rel, needle, nth), symbol, False))
        return self

    def graph(self) -> mg.Graph:
        sources = {
            rel: mg.read_source(module.split("::")[0], module, rel, text)
            for rel, (module, text) in self.files.items()
        }
        docs = [mg.Document(rel, occ) for rel, occ in self.occurrences.items()]
        return mg.read_graph(sources, docs)


def run(graph: mg.Graph, **kw) -> tuple[int, str]:
    out = io.StringIO()
    kw.setdefault("accepted", {})
    kw.setdefault("owed", {})
    code = mg.report(graph, out, **kw)
    return code, out.getvalue()


A = "c/src/a.rs"
B = "c/src/b.rs"
BEE = sym("c", "b/Bee#")
AY = sym("c", "a/Ay#")


def two_modules(a_text: str, b_text: str = "pub struct Bee;\n") -> Fixture:
    return Fixture({A: ("c::a", a_text), B: ("c::b", b_text)}).define(B, "Bee", BEE, (0, 0))


class Edges(unittest.TestCase):
    def test_a_reference_in_code_is_an_edge_and_one_in_a_use_is_not(self):
        f = two_modules("use crate::b::Bee;\n\npub fn ay() -> Bee {\n    Bee\n}\n")
        f.define(A, "ay", sym("c", "a/ay()."), (2, 4)).refer(A, "Bee", BEE, 0)
        graph = f.graph()
        self.assertEqual(graph.refs, [])
        f.refer(A, "Bee", BEE, 1)
        graph = f.graph()
        self.assertEqual(
            [(r.src_module, r.src_item, r.dst_module, r.dst_item, r.line) for r in graph.refs],
            [("c::a", "ay", "c::b", "Bee", 3)],
        )

    def test_a_reference_in_test_code_is_no_edge(self):
        text = (
            "pub fn ay() {}\n"
            "#[cfg(test)]\nfn helper() -> crate::b::Bee { todo!() }\n"
            "#[cfg(test)]\nmod tests {\n    fn t() { let _ = crate::b::Bee; }\n}\n"
        )
        f = two_modules(text).define(A, "ay", sym("c", "a/ay()."), (0, 0))
        f.refer(A, "Bee", BEE, 0).refer(A, "Bee", BEE, 1)
        self.assertEqual(f.graph().refs, [])

    def test_a_module_named_as_a_path_segment_is_no_edge(self):
        f = two_modules("pub fn ay() {\n    crate::b::Bee;\n}\n").define(A, "ay", sym("c", "a/ay()."), (0, 2))
        f.refer(A, "b::", sym("c", "b/"))
        self.assertEqual(f.graph().refs, [])
        f.refer(A, "Bee", BEE)
        self.assertEqual([r.dst_item for r in f.graph().refs], ["Bee"])

    def test_an_impl_belongs_to_its_self_type(self):
        text = "pub struct Ay;\n\nimpl From<Bee> for Ay {\n    fn from(_: Bee) -> Self {\n        Ay\n    }\n}\n"
        f = two_modules(text).define(A, "Ay", AY, (0, 0))
        f.define(A, "from", sym("c", "a/impl#[Ay][From]from()."), (3, 5))
        f.refer(A, "Bee", BEE, 0).refer(A, "Bee", BEE, 1)
        self.assertEqual({(r.src_item, r.line) for r in f.graph().refs}, {("Ay", 3), ("Ay", 4)})

    def test_a_reference_names_the_item_a_move_would(self):
        text = (
            "pub struct Ay<'a>(&'a str);\n\n"
            "impl<'a> Ay<'a> {\n    pub fn get(&self) {}\n}\n\n"
            "pub fn ay() {\n    const KW: u8 = 0;\n}\n"
        )
        f = Fixture({A: ("c::a", text), B: ("c::b", "pub fn bee() {\n    get(); KW;\n}\n")})
        f.define(A, "Ay", AY, (0, 0)).define(A, "get", sym("c", "a/impl#[`Ay<'a>`]get()."), (3, 3))
        f.define(A, "ay", sym("c", "a/ay()."), (6, 8)).define(A, "KW", sym("c", "a/KW."), (7, 7))
        f.define(B, "bee", sym("c", "b/bee()."), (0, 2))
        f.refer(B, "get", sym("c", "a/impl#[`Ay<'a>`]get().")).refer(B, "KW", sym("c", "a/KW."))
        f.define(A, "self", "local 1", (3, 3)).refer(B, "bee", "local 1")
        graph = f.graph()
        self.assertEqual({r.dst_item for r in graph.refs}, {"Ay", "ay"})
        self.assertLessEqual({r.dst_item for r in graph.refs}, graph.items["c::a"])

    def test_columns_are_utf8_bytes(self):
        f = two_modules("pub fn ay() {\n    let _é = \"—\"; crate::b::Bee;\n}\n")
        f.define(A, "ay", sym("c", "a/ay()."), (0, 2)).refer(A, "Bee", BEE)
        self.assertEqual([r.dst_item for r in f.graph().refs], ["Bee"])

    def test_a_comment_is_no_edge(self):
        f = two_modules("/// Builds a [`Bee`].\npub fn ay() {}\n").define(A, "ay", sym("c", "a/ay()."), (0, 1))
        f.refer(A, "Bee", BEE)
        self.assertEqual(f.graph().refs, [])


class Cycles(unittest.TestCase):
    def mutual(self) -> mg.Graph:
        f = two_modules("pub struct Ay;\npub fn ay(_: crate::b::Bee) {}\n", "pub struct Bee(crate::a::Ay);\n")
        f.define(A, "Ay", AY, (0, 0)).define(A, "ay", sym("c", "a/ay()."), (1, 1))
        f.refer(A, "Bee", BEE).refer(B, "Ay", AY)
        return f.graph()

    def test_two_modules_reading_each_other_are_a_cycle(self):
        code, out = run(self.mutual())
        self.assertEqual(code, 1)
        self.assertIn("!! cycle: c::a, c::b", out)
        self.assertIn("c/src/a.rs:2 ay -> Bee", out)
        self.assertIn("c/src/b.rs:1 Bee -> Ay", out)

    def test_an_accepted_cycle_passes_and_a_stale_acceptance_fails(self):
        reason = {frozenset({"c::a", "c::b"}): "a dispatcher and its arm"}
        code, out = run(self.mutual(), accepted=reason)
        self.assertEqual(code, 0, out)
        self.assertIn("== cycle accepted: c::a, c::b: a dispatcher and its arm", out)
        code, out = run(two_modules("").graph(), accepted=reason)
        self.assertEqual(code, 1)
        self.assertIn("!! stale acceptance: c::a, c::b is no cycle", out)

    def test_an_owed_cycle_passes_until_a_cut_changes_it(self):
        debt = {frozenset({"c::a", "c::b"}): "the next cut"}
        code, out = run(self.mutual(), owed=debt)
        self.assertEqual(code, 0, out)
        self.assertIn("1 cycles, 0 accepted, 1 owed", out)
        code, out = run(two_modules("").graph(), owed=debt)
        self.assertIn("!! stale debt: c::a, c::b is no cycle", out)

    def test_a_parent_reexporting_its_child_is_no_edge_but_calling_it_is(self):
        parent, child = "c/src/p.rs", "c/src/p/k.rs"
        f = Fixture(
            {
                parent: ("c::p", "mod k;\npub use k::Kay;\npub trait Contract {}\npub fn go() { k::Kay; }\n"),
                child: ("c::p::k", "pub struct Kay;\nimpl super::Contract for Kay {}\n"),
            }
        )
        kay, contract = sym("c", "p/k/Kay#"), sym("c", "p/Contract#")
        f.define(child, "Kay", kay, (0, 0)).define(parent, "Contract", contract, (2, 2))
        f.define(parent, "go", sym("c", "p/go()."), (3, 3))
        f.refer(parent, "Kay", kay, 0).refer(child, "Contract", contract)
        code, out = run(f.graph())
        self.assertEqual(code, 0, out)
        f.refer(parent, "Kay", kay, 1)
        code, out = run(f.graph())
        self.assertIn("!! cycle: c::p, c::p::k", out)


class Design(unittest.TestCase):
    def graph(self) -> mg.Graph:
        # `a` reads `Bee`; `b`'s `helper` reads `Ay`, so they cycle until
        # `helper` moves.
        f = two_modules(
            "pub struct Ay(crate::b::Bee);\n",
            "pub struct Bee;\npub fn helper(_: crate::a::Ay) {}\n",
        )
        f.define(A, "Ay", AY, (0, 0)).define(B, "helper", sym("c", "b/helper()."), (1, 1))
        f.refer(A, "Bee", BEE).refer(B, "Ay", AY)
        return f.graph()

    def test_moving_an_item_removes_the_cycle_it_made(self):
        moves, problems = mg.read_moves("```moves\nc::b::helper -> c::a  # beside what it reads\n```\n", self.graph())
        self.assertEqual(problems, [])
        code, out = run(self.graph(), moves=moves)
        self.assertEqual(code, 0, out)
        self.assertIn("module-graph: the moved tree: 2 modules, 1 edges, no cycle", out)

    def test_an_items_line_wins_over_its_modules(self):
        text = "```moves\nc::b -> c::bee\nc::b::helper -> c::b\n```\n"
        moves, _ = mg.read_moves(text, self.graph())
        self.assertEqual(moves.target("c::b", "helper"), "c::b")
        self.assertEqual(moves.target("c::b", "Bee"), "c::bee")
        # `Bee` leaving `b` for `bee` parts the cycle: `a` reads `bee`, `b` reads `a`.
        code, out = run(self.graph(), moves=moves)
        self.assertEqual(code, 0, out)
        self.assertIn("3 modules, 2 edges, no cycle", out)

    def test_a_move_naming_nothing_is_refused(self):
        _, problems = mg.read_moves("```moves\nc::b::nothing -> c::a\nc::b c::a\n```\n", self.graph())
        self.assertEqual(
            problems,
            [
                "move `c::b::nothing` names neither a module, an item one defines, nor a method of one",
                "move `c::b c::a` has no `->`",
            ],
        )

    def test_a_method_moves_away_from_its_type_by_its_own_line(self):
        text = (
            "pub struct Ay;\n"
            "impl Ay {\n    pub fn eval(&self) {}\n}\n"
            "impl Ay {\n    pub fn truths(&self) {\n        self.eval();\n    }\n}\n"
        )
        f = Fixture({A: ("c::a", text)}).define(A, "Ay", AY, (0, 0))
        f.define(A, "eval", sym("c", "a/impl#[Ay]eval()."), (2, 2))
        f.define(A, "truths", sym("c", "a/impl#[Ay]truths()."), (5, 7))
        f.refer(A, "eval", sym("c", "a/impl#[Ay]eval()."), 1)
        graph = f.graph()
        self.assertEqual(graph.members[("c::a", "Ay")], {"eval", "truths"})
        self.assertEqual(
            [(r.src_item, r.src_member, r.dst_item, r.dst_member) for r in graph.refs],
            [("Ay", "truths", "Ay", "eval")],
        )
        moves, problems = mg.read_moves("```moves\nc::a::Ay::truths -> c::a::truths\n```\n", graph)
        self.assertEqual(problems, [])
        self.assertEqual(moves.target("c::a", "Ay", "truths"), "c::a::truths")
        self.assertEqual(moves.target("c::a", "Ay", "eval"), "c::a")
        self.assertEqual(set(mg.edges(graph.refs, moves)), {("c::a::truths", "c::a")})

    def test_a_cycle_a_move_makes_across_crates_is_a_crate_cycle(self):
        moves, _ = mg.read_moves("```moves\nc::b::helper -> d::low\nc::b::Bee -> d::low\n```\n", self.graph())
        code, out = run(self.graph(), moves=moves)
        self.assertEqual(code, 1)
        self.assertIn("!! cycle: c::a, d::low", out)
        self.assertIn("!! crate cycle: c, d", out)


class ListedEdges(unittest.TestCase):
    def listed(self, graph: mg.Graph, group: str, moves: mg.Moves | None = None) -> tuple[bool, str]:
        out = io.StringIO()
        return mg.list_edges(graph, group, out, moves), out.getvalue()

    def test_a_modules_edges_are_listed_both_ways_each_pair_once(self):
        f = two_modules("pub fn ay(_: crate::b::Bee, _: crate::b::Bee) {}\n")
        f.define(A, "ay", sym("c", "a/ay()."), (0, 0))
        f.refer(A, "Bee", BEE, 0).refer(A, "Bee", BEE, 1)
        named, out = self.listed(f.graph(), "c::b")
        self.assertTrue(named)
        self.assertIn("-- edges of c::b: 0 references out, 2 in", out)
        self.assertIn("   in c::a -> c::b: 2\n       c/src/a.rs:1 ay -> Bee (+1 more)\n", out)

    def test_a_crate_takes_every_module_under_it_and_drops_what_stays_inside(self):
        named, out = self.listed(Design().graph(), "c")
        self.assertTrue(named)
        self.assertIn("-- edges of c: 0 references out, 0 in", out)

    def test_a_designs_new_module_lists_what_its_cluster_reads_and_is_read_by(self):
        graph = Design().graph()
        moves, _ = mg.read_moves("```moves\nc::b::helper -> c::h\n```\n", graph)
        named, out = self.listed(graph, "c::h", moves)
        self.assertTrue(named)
        self.assertIn("   out c::h -> c::a: 1\n       c/src/b.rs:2 helper -> Ay\n", out)

    def test_a_group_naming_no_module_is_refused(self):
        named, out = self.listed(Design().graph(), "c::nothing")
        self.assertFalse(named)
        self.assertIn("!! --edges c::nothing names no module", out)


class DocLinks(unittest.TestCase):
    def test_a_doc_link_against_the_edges_is_listed_and_never_fails(self):
        f = two_modules("pub struct Ay(crate::b::Bee);\n", "/// Held by [`crate::a::Ay`].\npub struct Bee;\n")
        f.define(A, "Ay", AY, (0, 0)).refer(A, "Bee", BEE)
        code, out = run(f.graph(), show_docs=True)
        self.assertEqual(code, 0, out)
        self.assertIn("-- doc link against the edges: c/src/b.rs:1 `crate::a::Ay`", out)
        self.assertIn("1 doc links against the edges", out)

    def test_a_doc_link_with_the_edges_is_not_listed(self):
        f = two_modules("/// Holds a [`crate::b::Bee`].\npub struct Ay(crate::b::Bee);\n")
        f.define(A, "Ay", AY, (0, 1)).refer(A, "Bee", BEE, 1)
        self.assertEqual(len(f.graph().docs), 1)
        self.assertIn("0 doc links against the edges", run(f.graph())[1])

    def test_a_path_in_test_code_or_a_string_is_no_doc_link(self):
        f = two_modules('pub fn ay() { let _ = "crate::b::Bee"; }\n#[cfg(test)]\nmod t {\n    // crate::b::Bee\n}\n')
        self.assertEqual(f.graph().docs, [])


class Index(unittest.TestCase):
    @staticmethod
    def varint(n: int) -> bytes:
        out = b""
        while True:
            b, n = n & 0x7F, n >> 7
            out += bytes([b | (0x80 if n else 0)])
            if not n:
                return out

    def field(self, number: int, value: bytes | int) -> bytes:
        if isinstance(value, int):
            return self.varint(number << 3) + self.varint(value)
        return self.varint(number << 3 | 2) + self.varint(len(value)) + value

    def packed(self, *ns: int) -> bytes:
        return b"".join(self.varint(n) for n in ns)

    def test_the_index_is_read(self):
        definition = (
            self.field(1, self.packed(3, 4, 7))
            + self.field(2, b"rust-analyzer cargo c 0.1.0 a/Ay#")
            + self.field(3, 1)
            + self.field(7, self.packed(2, 0, 5, 1))
        )
        reference = self.field(1, self.packed(9, 8, 10, 2)) + self.field(2, b"local 3")
        document = self.field(1, b"c/src/a.rs") + self.field(2, definition) + self.field(2, reference)
        data = self.field(1, b"metadata") + self.field(2, document)
        (doc,) = mg.read_index(data)
        self.assertEqual(doc.path, "c/src/a.rs")
        self.assertEqual(
            doc.occurrences,
            [
                mg.Occurrence((3, 4), "rust-analyzer cargo c 0.1.0 a/Ay#", True, ((2, 0), (5, 1))),
                mg.Occurrence((9, 8), "local 3", False, None),
            ],
        )

    def test_a_symbols_item_is_its_first_descriptor_after_the_module_path(self):
        cases = {
            "a/b/f().": "f",
            "a/Ay#Bee#field.": "Ay",
            "a/impl#[Ay][Clone]clone().": "Ay",
            "a/impl#[Ay]": "Ay",
            "timed!": "timed",
            "a/CONST.": "CONST",
        }
        for d, want in cases.items():
            self.assertEqual(mg.item_name(sym("c", d)), want, d)
        self.assertIsNone(mg.item_name("local 4"))
        self.assertTrue(mg.is_module(sym("c", "a/b/")))


class ThisRepo(unittest.TestCase):
    def test_every_kept_cycle_names_modules_of_the_tree_and_a_reason(self):
        sources = mg.workspace_sources(mg.REPO, layering.metadata(mg.REPO))
        modules = {s.module for s in sources.values()}
        for key, reason in {**mg.ACCEPTED, **mg.OWED}.items():
            self.assertLessEqual(set(key), modules, sorted(key))
            self.assertTrue(reason.strip(), sorted(key))
        self.assertFalse(set(mg.ACCEPTED) & set(mg.OWED))


if __name__ == "__main__":
    unittest.main()
