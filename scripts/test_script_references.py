"""Unit tests for script_references.py, run with `uv run python -m unittest test_script_references`.

Each test writes a few scripts as sources, reads them, and asserts the
references a module's code makes. The tree's own graph is held one-way by
`test_module_graph`'s `ThisRepo`.
"""

from __future__ import annotations

import unittest

import script_references as sr


def read(**sources: str) -> sr.Scripts:
    """Scripts by dotted name (`pkg.mod`, `pkg` for a package's `__init__`)."""
    paths, texts = {}, {}
    for dotted, text in sources.items():
        name = "::".join(["scripts", *dotted.split(".")])
        is_package = any(other.startswith(dotted + ".") for other in sources)
        rel = "scripts/" + dotted.replace(".", "/") + ("/__init__.py" if is_package else ".py")
        paths[name], texts[name] = rel, text
    return sr.read_sources(paths, texts)


def refs(scripts: sr.Scripts) -> set[tuple]:
    return {
        (r.src_module.removeprefix("scripts::"), r.src_item, r.dst_module.removeprefix("scripts::"), r.dst_item)
        for r in scripts.refs
    }


class Items(unittest.TestCase):
    def test_a_modules_items_are_its_top_level_definitions(self):
        s = read(
            a=(
                "from typing import TYPE_CHECKING\nX = 1\nY: int = 2\n"
                "if TYPE_CHECKING:\n    Z = 3\n"
                "def f():\n    inner = 1\n"
                "class C:\n    def m(self):\n        pass\n    attr = 1\n"
            )
        )
        self.assertEqual(s.items["scripts::a"], {"X", "Y", "Z", "f", "C"})
        self.assertEqual(s.members[("scripts::a", "C")], {"m"})

    def test_a_package_is_its_init(self):
        s = read(pkg="", **{"pkg.mod": "X = 1\n"})
        self.assertEqual(s.paths["scripts::pkg"], "scripts/pkg/__init__.py")
        self.assertEqual(s.paths["scripts::pkg::mod"], "scripts/pkg/mod.py")


class Imports(unittest.TestCase):
    def test_an_import_counts_where_its_name_is_used(self):
        s = read(b="X = 1\n", a="import b\n\ndef f():\n    return b.X\n")
        self.assertEqual(refs(s), {("a", "f", "b", "X")})

    def test_a_from_import_of_an_item_and_of_a_module(self):
        s = read(
            pkg="",
            **{"pkg.low": "def g():\n    pass\n"},
            a="from pkg import low\nfrom pkg.low import g\n\ndef f():\n    low.g()\n\ndef h():\n    g()\n",
        )
        self.assertEqual(refs(s), {("a", "f", "pkg::low", "g"), ("a", "h", "pkg::low", "g")})

    def test_a_child_named_through_its_parent_is_the_child(self):
        s = read(pkg="", **{"pkg.low": "X = 1\n"}, a="import pkg.low\n\ndef f():\n    return pkg.low.X\n")
        self.assertEqual(refs(s), {("a", "f", "pkg::low", "X")})

    def test_an_unused_import_is_a_reference_from_where_it_stands(self):
        s = read(b="X = 1\n", a="import b\n\ndef f():\n    from b import X\n")
        self.assertEqual(refs(s), {("a", None, "b", None), ("a", "f", "b", "X")})

    def test_a_reexport_is_followed_to_its_definition(self):
        s = read(low="X = 1\n", mid="from low import X\n", a="from mid import X\n\nY = X\n")
        self.assertIn(("a", "Y", "low", "X"), refs(s))

    def test_a_relative_import_resolves_against_its_package(self):
        s = read(pkg="", **{"pkg.low": "X = 1\n", "pkg.a": "from .low import X\n\nY = X\n"})
        self.assertEqual(refs(s), {("pkg::a", "Y", "pkg::low", "X")})

    def test_an_import_outside_the_scripts_is_nothing(self):
        s = read(a="import os\nfrom pathlib import Path\n\nP = Path(os.sep)\n")
        self.assertEqual(refs(s), set())


class Scopes(unittest.TestCase):
    def test_a_name_a_function_binds_shadows_the_modules(self):
        s = read(
            config="REPO = 1\n",
            a="from config import REPO\n\ndef f(REPO):\n    return REPO\n\ndef g():\n    return REPO\n",
        )
        self.assertEqual(refs(s), {("a", "g", "config", "REPO")})

    def test_a_functions_own_import_binds_in_it(self):
        s = read(release="IMAGE = 1\n", a="def f():\n    import release\n    return release.IMAGE\n")
        self.assertEqual(refs(s), {("a", "f", "release", "IMAGE")})

    def test_an_annotation_and_a_type_checking_import_are_references(self):
        s = read(
            timing="class Session:\n    pass\n",
            a=(
                "from typing import TYPE_CHECKING\nif TYPE_CHECKING:\n    from timing import Session\n\n"
                "def f(session: Session) -> None:\n    pass\n"
            ),
        )
        self.assertEqual(refs(s), {("a", "f", "timing", "Session")})

    def test_a_default_is_read_in_the_defining_scope(self):
        s = read(b="X = 1\n", a="import b\n\ndef f(b=b.X):\n    return b\n")
        self.assertEqual(refs(s), {("a", "f", "b", "X")})

    def test_a_modules_own_items_reference_each_other(self):
        s = read(a="X = 1\n\ndef f():\n    return X\n\ndef g():\n    return f()\n")
        self.assertEqual(refs(s), {("a", "f", "a", "X"), ("a", "g", "a", "f")})

    def test_a_global_declared_name_is_the_modules(self):
        s = read(a="_BUILT = False\n\ndef f():\n    global _BUILT\n    _BUILT = True\n    return _BUILT\n")
        self.assertIn(("a", "f", "a", "_BUILT"), refs(s))


class Methods(unittest.TestCase):
    def test_self_names_a_method_of_its_class(self):
        s = read(
            a="class C:\n    def m(self):\n        return self.n()\n\n    def n(self):\n        return self.field\n"
        )
        found = [(r.src_item, r.src_member, r.dst_item, r.dst_member) for r in s.refs]
        self.assertEqual(found, [("C", "m", "C", "n")])

    def test_a_class_name_names_its_method_and_a_reference_inside_one_names_it(self):
        s = read(
            b="class B:\n    def go(self):\n        pass\n",
            a="import b\n\nclass C:\n    def m(self):\n        b.B.go(self)\n",
        )
        found = [(r.src_item, r.src_member, r.dst_module, r.dst_item, r.dst_member) for r in s.refs]
        self.assertEqual(found, [("C", "m", "scripts::b", "B", "go")])


if __name__ == "__main__":
    unittest.main()
