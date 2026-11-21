"""The package's own shape: every edge between its modules runs one way, and
every module says what it holds.

The rule is `docs/process.md`, "Seams"; this is the harness's half of what
`module_graph.py` asserts of the workspace. An edge is an import of one of the
package's modules, at the top of a module or under `TYPE_CHECKING`: a type
named in an annotation is a dependency as much as a call is, so a cycle drawn
through annotations alone is still refused.
"""

from __future__ import annotations

import ast
import unittest
from pathlib import Path

import measurement

PACKAGE = Path(measurement.__file__).parent


def production_modules() -> dict[str, Path]:
    """Every production module of the package, by dotted name."""
    found = {}
    for path in sorted(PACKAGE.rglob("*.py")):
        rel = path.relative_to(PACKAGE.parent).with_suffix("")
        if rel.name.startswith("test_") or "__pycache__" in rel.parts:
            continue
        parts = rel.parts[:-1] if rel.name == "__init__" else rel.parts
        found[".".join(parts)] = path
    return found


def edges(modules: dict[str, Path]) -> dict[str, set[str]]:
    """Each module's imports of the package's other modules."""
    out: dict[str, set[str]] = {}
    for name, path in modules.items():
        tree = ast.parse(path.read_text(encoding="utf-8"))
        reached: set[str] = set()
        for node in ast.walk(tree):
            if isinstance(node, ast.ImportFrom) and node.module and node.module.startswith("measurement"):
                for alias in node.names:
                    whole = f"{node.module}.{alias.name}"
                    reached.add(whole if whole in modules else node.module)
            elif isinstance(node, ast.Import):
                reached |= {a.name for a in node.names if a.name in modules}
        out[name] = reached - {name, "measurement"}
    return out


def cycles(graph: dict[str, set[str]]) -> list[list[str]]:
    """The strongly connected components of more than one module."""
    index: dict[str, int] = {}
    low: dict[str, int] = {}
    stack: list[str] = []
    on: set[str] = set()
    found: list[list[str]] = []

    def visit(v: str) -> None:
        index[v] = low[v] = len(index)
        stack.append(v)
        on.add(v)
        for w in sorted(graph.get(v, ())):
            if w not in index:
                visit(w)
                low[v] = min(low[v], low[w])
            elif w in on:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            component = []
            while True:
                w = stack.pop()
                on.discard(w)
                component.append(w)
                if w == v:
                    break
            if len(component) > 1:
                found.append(sorted(component))

    for v in sorted(graph):
        if v not in index:
            visit(v)
    return found


class Layout(unittest.TestCase):
    def test_every_edge_runs_one_way(self):
        self.assertEqual(cycles(edges(production_modules())), [])

    def test_a_cycle_is_found(self):
        self.assertEqual(cycles({"a": {"b"}, "b": {"c"}, "c": {"a"}, "d": {"a"}}), [["a", "b", "c"]])

    def test_every_module_says_what_it_holds(self):
        for name, path in production_modules().items():
            with self.subTest(module=name):
                doc = ast.get_docstring(ast.parse(path.read_text(encoding="utf-8")))
                self.assertTrue(doc, f"{name} has no docstring")
                self.assertNotEqual(doc.strip(), name, f"{name}'s docstring is its own name")

    def test_the_command_line_imports_the_package_and_nothing_imports_it(self):
        for name, path in production_modules().items():
            with self.subTest(module=name):
                tree = ast.parse(path.read_text(encoding="utf-8"))
                imported = {a.name for n in ast.walk(tree) if isinstance(n, ast.Import) for a in n.names}
                self.assertNotIn("measure", imported)


if __name__ == "__main__":
    unittest.main()
