#!/usr/bin/env python3
"""The scripts' references, read off Python's own `ast`: what
`module_graph.py` holds one-way beside the workspace's modules.

The rule is `docs/process.md`, "Seams". rust-analyzer reads no Python, so
the scripts' edges are read here, in the shape `module_graph.py` reads a
crate's:

* a **module** is a production `.py` under `scripts/`, as `file_limits.py`
  reads one, named `scripts::<path>` -- `scripts::measure`,
  `scripts::measurement::timing`, a package's `__init__.py` the package -- so
  a design's moves name a script's items as they name a crate's, and every
  script is a module of one crate, `scripts`;
* an **item** is what a module's top level defines -- a `def`, a `class`, a
  name assigned -- a top-level `if`, `try` or `with` included (`if
  TYPE_CHECKING:`), and a **member** a method a top-level class defines;
* a **reference** is a name an item's code loads that resolves to another
  module's item, or to another item of its own module: through an import
  binding it (`from measurement import config` then `config.REPO`, `from
  measurement.timing import Session`, `import release` then
  `release.IMAGE`), followed through a re-export, or by the module's own
  top-level name. A name a function binds -- a parameter, an assignment, its
  own import -- shadows the module's, as Python scopes it. A name in an
  annotation is a reference, a type naming a dependency as much as a call
  does, and so is one under `TYPE_CHECKING`. A child module named through
  its parent (`measurement.timing`) is the child; the package is no
  reference;
* an **import is no reference where its name is used**, as a Rust `use` is
  none: the use is the reference, so a design moving the use moves the edge.
  An import whose name nothing uses is one, from where it stands, since
  Python executes it whatever reads it;
* `self.<method>` and `cls.<method>` inside a top-level class's method, and
  `<Class>.<method>` anywhere, name that method, so a design can move one
  method away from its class. A call through any other instance names no
  item: Python carries no type to resolve it by, and the class named where
  the instance is declared is the reference standing for it. So a design
  moving a method alone is checked on what the method reads and on the
  calls its class and its class's name make.

Nothing outside `scripts/` -- the standard library, a dependency -- is a
module here. A reference from or to a module's top level carries no item
(`None`), which `module_graph.py` names `(top level)`.

A library: `module_graph.py` is its command line, and
`uv run python -m unittest test_script_references` its tests.
"""

from __future__ import annotations

import ast
from dataclasses import dataclass, field
from pathlib import Path

import file_limits

#: The crate every script module is named under, `module_graph.py`'s word
#: for one.
ROOT = "scripts"

Target = tuple[str, "str | None"]  # (dotted module, item or None)
Binding = tuple["ast.Import | ast.ImportFrom", ast.alias]


@dataclass(frozen=True)
class Reference:
    """One reference, its fields `module_graph.Ref`'s, in its order; an item
    of `None` is the module's top level."""

    src_module: str
    src_item: str | None
    dst_module: str
    dst_item: str | None
    rel: str
    line: int  # 1-based
    src_member: str | None = None
    dst_member: str | None = None


@dataclass
class Scripts:
    """The scripts' modules, their items and methods, and their references."""

    paths: dict[str, str]  # module -> repo-relative path
    items: dict[str, set[str]]
    members: dict[tuple[str, str], set[str]]
    refs: list[Reference]


@dataclass
class _Module:
    name: str  # dotted, as Python imports it
    graph_name: str  # `scripts::…`
    rel: str
    tree: ast.Module
    items: set[str] = field(default_factory=set)
    members: dict[str, set[str]] = field(default_factory=dict)
    imports: dict[str, Binding] = field(default_factory=dict)


def module_paths(repo: Path) -> dict[str, str]:
    """Every production script's module name, to its repo-relative path."""
    out = {}
    for r in file_limits.script_readings(repo):
        if r.is_test:
            continue
        parts = list(Path(r.path).with_suffix("").parts)
        if parts[-1] == "__init__":
            parts = parts[:-1]
        out["::".join(parts)] = r.path
    return out


def _flattened(stmt: ast.stmt) -> list[ast.stmt]:
    """The statements a top-level `if`, `try` or `with` holds, at any depth,
    as the module's own; any other statement alone."""
    if isinstance(stmt, ast.If):
        inner = stmt.body + stmt.orelse
    elif isinstance(stmt, ast.Try):
        inner = stmt.body + [s for h in stmt.handlers for s in h.body] + stmt.orelse + stmt.finalbody
    elif isinstance(stmt, ast.With):
        inner = stmt.body
    else:
        return [stmt]
    return [s for i in inner for s in _flattened(i)]


def _assigned(stmt: ast.stmt) -> list[str]:
    """The names an assignment binds, in order."""
    if isinstance(stmt, ast.Assign):
        targets = stmt.targets
    elif isinstance(stmt, (ast.AnnAssign, ast.AugAssign)):
        targets = [stmt.target]
    else:
        return []
    return [n.id for t in targets for n in ast.walk(t) if isinstance(n, ast.Name)]


def _bound(alias: ast.alias) -> str:
    return alias.asname or alias.name.split(".")[0]


def _read_module(name: str, graph_name: str, rel: str, text: str) -> _Module:
    module = _Module(name, graph_name, rel, ast.parse(text, filename=rel))
    for top in module.tree.body:
        for stmt in _flattened(top):
            if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                module.items.add(stmt.name)
                if isinstance(stmt, ast.ClassDef):
                    module.members[stmt.name] = {
                        s.name for s in stmt.body if isinstance(s, (ast.FunctionDef, ast.AsyncFunctionDef))
                    }
            elif isinstance(stmt, (ast.Import, ast.ImportFrom)):
                for alias in stmt.names:
                    module.imports[_bound(alias)] = (stmt, alias)
            else:
                module.items.update(_assigned(stmt))
    return module


class _Resolver:
    """What a name bound by an import, or reached through a module, is."""

    def __init__(self, modules: dict[str, _Module]):
        self.modules = modules

    def imported(self, here: _Module, binding: Binding, seen: frozenset[str] = frozenset()) -> Target | None:
        """`(module, None)` for a module, `(module, item)` for an item, None
        outside the scripts."""
        stmt, alias = binding
        if isinstance(stmt, ast.Import):
            whole = alias.name if alias.asname else alias.name.split(".")[0]
            return (whole, None) if whole in self.modules else None
        base = stmt.module or ""
        if stmt.level:
            package = here.name.split(".")
            if not here.rel.endswith("__init__.py"):
                package = package[:-1]
            package = package[: len(package) - (stmt.level - 1)]
            base = ".".join([*package, *([base] if base else [])])
        if base in self.modules or f"{base}.{alias.name}" in self.modules:
            return self.attribute(base, alias.name, seen)
        return None

    def attribute(self, module: str, name: str, seen: frozenset[str] = frozenset()) -> Target:
        """`module.name`: a child module, an item, or what `module` imported
        under that name; else the module's top level."""
        if f"{module}.{name}" in self.modules:
            return (f"{module}.{name}", None)
        held = self.modules[module]
        if name in held.items:
            return (module, name)
        if name in held.imports and module not in seen:
            found = self.imported(held, held.imports[name], seen | {module})
            if found is not None:
                return found
        return (module, None)


@dataclass
class _Scope:
    locals: set[str]
    imports: dict[str, Binding]
    owner: str | None  # the top-level class a method's scope belongs to
    is_class: bool = False


class _Visitor(ast.NodeVisitor):
    """One module's references, each from the item and method it sits in."""

    def __init__(self, resolver: _Resolver, module: _Module, out: list[Reference]):
        self.resolver, self.module, self.out = resolver, module, out
        self.scopes: list[_Scope] = []
        self.item: str | None = None
        self.member: str | None = None
        self.used: set[tuple[int, str]] = set()  # (id of an import table, name)
        self.tables: list[tuple[dict[str, Binding], str | None, str | None]] = [
            (module.imports, None, None)
        ]

    def top(self, stmt: ast.stmt) -> None:
        """One top-level statement, read as the item it defines; a top-level
        `if`'s, `try`'s or `with`'s statements each as its own."""
        self.item = self.member = None
        if isinstance(stmt, ast.If):
            self.visit(stmt.test)
        elif isinstance(stmt, ast.With):
            for w in stmt.items:
                self.visit(w.context_expr)
        elif isinstance(stmt, ast.Try):
            for h in stmt.handlers:
                if h.type is not None:
                    self.visit(h.type)
        if isinstance(stmt, (ast.If, ast.With, ast.Try)):
            inner = {ast.If: lambda s: s.body + s.orelse, ast.With: lambda s: s.body}.get(
                type(stmt),
                lambda s: s.body + [x for h in s.handlers for x in h.body] + s.orelse + s.finalbody,
            )(stmt)
            for s in inner:
                self.top(s)
            return
        if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            self.item = stmt.name
        else:
            named = _assigned(stmt)
            self.item = named[0] if named else None
        self.visit(stmt)
        self.item = self.member = None

    def emit(self, node: ast.AST, target: Target, dst_member: str | None = None) -> None:
        dst, dst_item = target
        if (dst, dst_item, dst_member) == (self.module.name, self.item, self.member):
            return
        self.out.append(
            Reference(
                self.module.graph_name,
                self.item,
                self.resolver.modules[dst].graph_name,
                dst_item,
                self.module.rel,
                getattr(node, "lineno", 1),
                self.member,
                dst_member,
            )
        )

    def lookup(self, name: str) -> Target | None:
        """A loaded name's target, or None for a local, a builtin or anything
        outside the scripts. A class body's names are not its methods'."""
        for scope in reversed(self.scopes):
            if scope.is_class:
                continue
            if name in scope.imports:
                self.used.add((id(scope.imports), name))
                return self.resolver.imported(self.module, scope.imports[name])
            if name in scope.locals:
                return None
        if name in self.module.imports:
            self.used.add((id(self.module.imports), name))
            return self.resolver.imported(self.module, self.module.imports[name])
        if name in self.module.items:
            return (self.module.name, name)
        return None

    def owner(self) -> str | None:
        return next((s.owner for s in reversed(self.scopes) if s.owner is not None), None)

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, ast.Load):
            found = self.lookup(node.id)
            if found is not None:
                self.emit(node, found)

    def visit_Attribute(self, node: ast.Attribute) -> None:
        chain: list[str] = []
        base: ast.expr = node
        while isinstance(base, ast.Attribute):
            chain.append(base.attr)
            base = base.value
        if not isinstance(base, ast.Name):
            self.generic_visit(node)
            return
        chain.reverse()
        owner = self.owner()
        if base.id in ("self", "cls") and owner is not None:
            if chain[0] in self.module.members.get(owner, set()):
                self.emit(node, (self.module.name, owner), chain[0])
            return
        found = self.lookup(base.id)
        if found is None:
            return
        rest = list(chain)
        while rest and found[1] is None:
            found = self.resolver.attribute(found[0], rest.pop(0))
        methods = self.resolver.modules[found[0]].members.get(found[1] or "", set())
        self.emit(node, found, rest[0] if rest and rest[0] in methods else None)

    def visit_FunctionDef(self, node: ast.FunctionDef) -> None:
        self.function(node)

    def visit_AsyncFunctionDef(self, node: ast.AsyncFunctionDef) -> None:
        self.function(node)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        self.function(node)

    def function(self, node: ast.FunctionDef | ast.AsyncFunctionDef | ast.Lambda) -> None:
        """A function's defaults, decorators and annotations in the scope it
        is defined in, its body in its own."""
        args = node.args
        for default in [*args.defaults, *(d for d in args.kw_defaults if d is not None)]:
            self.visit(default)
        if not isinstance(node, ast.Lambda):
            for deco in node.decorator_list:
                self.visit(deco)
            for arg in _parameters(args):
                if arg.annotation is not None:
                    self.visit(arg.annotation)
            if node.returns is not None:
                self.visit(node.returns)
        body = node.body if isinstance(node.body, list) else [node.body]
        method = (
            not isinstance(node, ast.Lambda)
            and len(self.scopes) == 1
            and self.scopes[0].is_class
            and self.scopes[0].owner is not None
        )
        outer = self.member
        if method:
            self.member = node.name  # type: ignore[union-attr]
        scope = _Scope(_locals(args, body), _imports(body), self.scopes[0].owner if method else None)
        self.tables.append((scope.imports, self.item, self.member))
        self.scopes.append(scope)
        for stmt in body:
            self.visit(stmt)
        self.scopes.pop()
        self.member = outer

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for expr in [*node.bases, *(k.value for k in node.keywords), *node.decorator_list]:
            self.visit(expr)
        self.scopes.append(_Scope(set(), {}, None if self.scopes else node.name, is_class=True))
        for stmt in node.body:
            self.visit(stmt)
        self.scopes.pop()

    def visit_Import(self, node: ast.Import) -> None:
        pass  # a binding, referenced where it is used (`unused`)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        pass

    def unused(self) -> None:
        """Each import whose name nothing loads, from where it stands."""
        for table, item, member in self.tables:
            for name, binding in table.items():
                if (id(table), name) in self.used:
                    continue
                target = self.resolver.imported(self.module, binding)
                if target is not None:
                    self.item, self.member = item, member
                    self.emit(binding[0], target)
        self.item = self.member = None


def _parameters(args: ast.arguments) -> list[ast.arg]:
    return [a for a in [*args.posonlyargs, *args.args, *args.kwonlyargs, args.vararg, args.kwarg] if a]


def _own_nodes(body: list[ast.stmt]):
    """Every node of a function's body but what a nested function, class or
    lambda holds, the nested definition's own node included."""
    todo: list[ast.AST] = list(body)
    while todo:
        node = todo.pop()
        yield node
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef, ast.Lambda)):
            todo.extend(ast.iter_child_nodes(node))


def _locals(args: ast.arguments, body: list[ast.stmt]) -> set[str]:
    """What a function binds: its parameters and every name its own body
    stores or defines, less what it declares `global`."""
    names = {a.arg for a in _parameters(args)}
    declared: set[str] = set()
    for node in _own_nodes(body):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            names.add(node.name)
        elif isinstance(node, ast.Name) and isinstance(node.ctx, (ast.Store, ast.Del)):
            names.add(node.id)
        elif isinstance(node, ast.Global):
            declared.update(node.names)
        elif isinstance(node, ast.ExceptHandler) and node.name:
            names.add(node.name)
    return names - declared


def _imports(body: list[ast.stmt]) -> dict[str, Binding]:
    """What a function's own body imports, a nested function's not."""
    return {
        _bound(alias): (node, alias)
        for node in _own_nodes(body)
        if isinstance(node, (ast.Import, ast.ImportFrom))
        for alias in node.names
    }


def read(repo: Path) -> Scripts:
    """Every production script's items, methods and references."""
    paths = module_paths(repo)
    return read_sources(paths, {m: (repo / rel).read_text(encoding="utf-8") for m, rel in paths.items()})


def read_sources(paths: dict[str, str], sources: dict[str, str]) -> Scripts:
    """`read` over sources given by module, each named `scripts::…`."""
    modules: dict[str, _Module] = {}
    for graph_name, rel in paths.items():
        dotted = ".".join(graph_name.split("::")[1:])
        modules[dotted] = _read_module(dotted, graph_name, rel, sources[graph_name])
    resolver = _Resolver(modules)
    refs: list[Reference] = []
    for module in modules.values():
        visitor = _Visitor(resolver, module, refs)
        for stmt in module.tree.body:
            visitor.top(stmt)
        visitor.unused()
    return Scripts(
        dict(paths),
        {m.graph_name: set(m.items) for m in modules.values()},
        {(m.graph_name, cls): set(methods) for m in modules.values() for cls, methods in m.members.items()},
        refs,
    )
