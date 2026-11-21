"""The package's own shape beyond its edges: every module says what it holds,
and nothing reads the command line.

Every edge between the package's modules running one way is `module_graph.py`'s,
which holds every script to it (`docs/process.md`, "Seams"), and
`test_module_graph` beside it.
"""

from __future__ import annotations

import ast
import unittest

import script_references
from file_limits import REPO

PACKAGE = "scripts::measurement"


def production_modules() -> dict[str, str]:
    """Every production module of the package, to its repo-relative path."""
    return {
        name: rel
        for name, rel in script_references.module_paths(REPO).items()
        if name == PACKAGE or name.startswith(PACKAGE + "::")
    }


class Layout(unittest.TestCase):
    def test_every_module_says_what_it_holds(self):
        for name, rel in production_modules().items():
            with self.subTest(module=name):
                doc = ast.get_docstring(ast.parse((REPO / rel).read_text(encoding="utf-8")))
                self.assertTrue(doc, f"{name} has no docstring")
                dotted = ".".join(name.split("::")[1:])
                self.assertNotEqual(doc.strip(), dotted, f"{name}'s docstring is its own name")

    def test_nothing_reads_the_command_line(self):
        refs = script_references.read(REPO).refs
        reading = {r.src_module for r in refs if r.dst_module == "scripts::measure"}
        self.assertEqual(reading - {"scripts::measure"}, set())


if __name__ == "__main__":
    unittest.main()
