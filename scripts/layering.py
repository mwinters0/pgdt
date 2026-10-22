#!/usr/bin/env python3
"""The crate graph and each crate's boundary, read off `cargo metadata` and
the members' sources.

The rules are `docs/design/decisions.md`, "D68", "D74" and "D106". Cargo
refuses a cycle but not an edge the graph does not draw, and nothing in Rust
refuses a name a crate could reach but should not, so this holds:

* **every member is in the graph** (`GRAPH`), and only those, and **each
  member's dependencies on another member are the ones its row lists** --
  normal and build dependencies; a test may reach what the library may not,
  so dev-dependencies are not read;
* **no crate below the query layer depends on Arrow**, the codec taking
  `arrow-schema` and `arrow-buffer` alone and the mapping pass
  `arrow-schema`, and **the codec names no `arrow_buffer` item but `i256`**,
  which a dependency list cannot see;
* **the core names no dependency whose error its error type would carry**,
  and **the model and the codec do no I/O**, the codec synchronous
  (`CORE_NAMES_NO_DEPENDENCY`, `NO_IO`);
* **no embedder names a crate's `internal` module, nor `timed!` or
  `row_evaluated!`**, which `#[macro_export]` roots at `pgdt_core` whatever
  module declares them -- in any of its sources, tests included;
* **the embedder test (`EMBEDDER_TEST`) exists and is held to the same
  rule**, it and every file its `mod` lines reach, and **names no
  `pgdt_fixtures`**, which no embedder can depend on (`GRAPH`): it walks the
  library naming only what an embedder outside the workspace could;
* **no module loads by `#[path]`**, in any member, tests included, so a file
  is the module its name says and a test's sources are the files its `mod`
  lines reach (`check.py`'s `module_files`) -- a crate's tests reach another's
  vocabulary through `pgdt-fixtures`;
* **every library module is private but `internal` and the namespaces
  `NAMESPACES` lists**, a namespace's own children included, so an item's
  only paths are its API one and `internal`'s;
* **a namespaced API item is not also re-exported at its crate's root**: an
  API item has one path.

A library's production code is its module tree from `lib.rs`, every module a
`#[cfg(test)]` declares left out and every `#[cfg(test)] mod {}` block
blanked. Everything is read through `file_limits.masked`, so a comment or a
string literal naming a thing is not a use of it.

Usage:

    cd scripts && uv run layering.py
    cd scripts && uv run python -m unittest test_layering
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import TextIO

from file_limits import masked, test_blocks

REPO = Path(__file__).resolve().parent.parent

#: The seven library crates, in the graph's order (D68).
LIBRARIES = (
    "pgdt-core",
    "pgdt-model",
    "pgdt-io",
    "pgdt-dump",
    "pgdt-codec",
    "pgdt-mapping",
    "pgdt-query",
)

#: The embedders: everything above the query layer that links a library.
EMBEDDERS = ("datafusion-pgdump", "datafusion-cli-pgdump", "pgdt")

#: Every member and the members it may depend on, so a new member is placed
#: deliberately (D68). The dump's parsers and the codec are siblings, neither
#: reaching the other; `namespace-init` installs a binary's signal handlers
#: (D26), which no library does; `gz-seek` is a gzip file's positioned read
#: beneath io, a leaf depending on no member; and `peak-rss`,
#: `count-instructions` and `gzip-inflate` are the harness's. `pgdt-fixtures`
#: is the tests' vocabulary, which no row lists, so only a dev-dependency
#: reaches it; its edges, behind its
#: `mapping` feature, are for a sweep's request builders and a fixture mapped
#: beside its cache, which takes the mapping pass and what its call names.
GRAPH: dict[str, tuple[str, ...]] = {
    "pgdt-core": (),
    "pgdt-model": ("pgdt-core",),
    "pgdt-io": ("pgdt-core", "pgdt-model", "gz-seek"),
    "pgdt-dump": ("pgdt-core", "pgdt-model", "pgdt-io"),
    "pgdt-codec": ("pgdt-core", "pgdt-model"),
    "pgdt-mapping": ("pgdt-core", "pgdt-model", "pgdt-io", "pgdt-dump", "pgdt-codec"),
    "pgdt-query": (
        "pgdt-core",
        "pgdt-model",
        "pgdt-io",
        "pgdt-dump",
        "pgdt-codec",
        "pgdt-mapping",
    ),
    "datafusion-pgdump": LIBRARIES,
    "datafusion-cli-pgdump": (*LIBRARIES, "datafusion-pgdump", "namespace-init"),
    "pgdt": (*LIBRARIES, "datafusion-pgdump", "datafusion-cli-pgdump", "namespace-init"),
    "namespace-init": (),
    "gz-seek": (),
    "peak-rss": (),
    "count-instructions": (),
    "gzip-inflate": (),
    "pgdt-fixtures": ("pgdt-model", "pgdt-io", "pgdt-dump", "pgdt-mapping"),
}

#: Each library's API namespaces: the public modules an embedder names. Any
#: other public module but `internal` is refused, so a new namespace is an
#: edit here, made deliberately (D68).
NAMESPACES: dict[str, tuple[str, ...]] = {
    "pgdt-core": ("instrument",),
    "pgdt-dump": ("cache",),
    "pgdt-codec": ("compare", "decode", "grammar", "key", "nested", "pgtype"),
    "pgdt-mapping": ("statistics",),
}

#: The Arrow crates a library below the query layer never depends on (D74):
#: the codec and the mapping pass name `arrow-schema`'s types and build no
#: array, and the crates below them name no Arrow at all.
NAMES_ARROW_SCHEMA = ("pgdt-codec", "pgdt-mapping")
ARROW_ABOVE_THE_CODEC = ("arrow",)
ARROW_BELOW_THE_CODEC = ("arrow", "arrow-schema", "arrow-buffer")

#: What the core never names: every dependency whose error the core's error
#: type would otherwise carry (D74).
CORE_NAMES_NO_DEPENDENCY = ("xz_seek", "gz_seek", "bincode", "tokio", "object_store")

#: What the model and the codec never name: they do no I/O, and the codec is
#: synchronous (D74).
NO_IO = ("async fn", "ByteRangeSource", "std::fs", "tokio", "xz_seek", "gz_seek")

#: The macros an embedder never invokes or imports.
INTERNAL_MACROS = ("timed", "row_evaluated")

#: The integration test written as an embedder: its crate, and its root
#: there. It is the surface an embedder is promised
#: (`docs/design/roadmap.md`, "Pre-1.0: no compatibility obligations").
EMBEDDER_TEST = ("pgdt-query", "tests/embedder.rs")

INTERNAL_PATH_RE = re.compile(r"\bpgdt_\w+\s*::\s*(?:internal\b|\{[^;]*?\binternal\b)")
MACRO_RE = re.compile(
    r"\b(" + "|".join(INTERNAL_MACROS) + r")\s*!(?!=)"
    r"|\bpgdt_core\s*::\s*(?:\{[^;]*?)?\b(" + "|".join(INTERNAL_MACROS) + r")\b"
)
ARROW_BUFFER_RE = re.compile(r"\barrow_buffer\s*::\s*(\{[^}]*\}|\w+)")
FIXTURES_RE = re.compile(r"\bpgdt_fixtures\b")
PATH_ATTR_RE = re.compile(r"#\s*\[\s*path\b")
MOD_DECL_RE = re.compile(r"(?m)^[ \t]*(pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*([{;])")
PUB_ITEM_RE = re.compile(
    r"(?m)^pub\s+(?:async\s+|unsafe\s+|const\s+)*"
    r"(?:fn|struct|enum|trait|type|const|static|mod|union)\s+([A-Za-z_]\w*)"
)
PUB_USE_RE = re.compile(r"(?m)^pub\s+use\s+([^;]*);")


def metadata(repo: Path) -> dict:
    """The workspace's members as `cargo metadata` describes them."""
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return json.loads(out)


def members(meta: dict) -> dict[str, Path]:
    """Each member's name and directory."""
    ids = set(meta["workspace_members"])
    return {
        p["name"]: Path(p["manifest_path"]).parent for p in meta["packages"] if p["id"] in ids
    }


def dependencies(meta: dict, name: str) -> list[str]:
    """A member's normal and build dependencies, by package name."""
    package = next(p for p in meta["packages"] if p["name"] == name)
    return [d["name"] for d in package["dependencies"] if d.get("kind") != "dev"]


def _depth_at(text: str) -> list[int]:
    depth, out = 0, []
    for ch in text:
        out.append(depth)
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
    out.append(depth)
    return out


def production_text(path: Path) -> str:
    """One file masked, its `#[cfg(test)] mod {}` blocks blanked."""
    text = masked(path.read_text(encoding="utf-8"))
    lines = text.split("\n")
    ranges, _ = test_blocks(text)
    for lo, hi in ranges:
        for k in range(lo, hi + 1):
            lines[k] = ""
    return "\n".join(lines)


def _is_cfg_test(text: str, at: int) -> bool:
    """Whether the item starting at `at` carries `#[cfg(test)]`."""
    return text[:at].rstrip().endswith("#[cfg(test)]")


def module_tree(src: Path, root: str = "lib.rs") -> dict[Path, str]:
    """A crate's production files from its root down, each with its
    production text: a module a `#[cfg(test)]` declares is not followed."""
    out: dict[Path, str] = {}
    pending = [src / root]
    while pending:
        file = pending.pop()
        if file in out or not file.is_file():
            continue
        text = production_text(file)
        out[file] = text
        base = file.parent if file.name in ("lib.rs", "main.rs", "mod.rs") else file.with_suffix("")
        for m in MOD_DECL_RE.finditer(text):
            if m.group(3) != ";" or _is_cfg_test(text, m.start()):
                continue
            name = m.group(2)
            for child in (base / f"{name}.rs", base / name / "mod.rs"):
                if child.is_file():
                    pending.append(child)
                    break
    return out


def test_target(root: Path) -> list[Path]:
    """A test target's root and every file its `mod name;` lines reach, at
    the conventional paths -- no member loads a module by `#[path]` -- as
    `check.py`'s `module_files` follows them. A declaration under
    `#[cfg(test)]` is followed too: in a test target, it is compiled."""
    out: list[Path] = []
    pending = [(root, True)]
    while pending:
        file, is_mod_root = pending.pop()
        if file in out or not file.is_file():
            continue
        out.append(file)
        base = file.parent if is_mod_root else file.with_suffix("")
        text = masked(file.read_text(encoding="utf-8"))
        for m in MOD_DECL_RE.finditer(text):
            if m.group(3) != ";":
                continue
            name = m.group(2)
            for child, root_like in ((base / f"{name}.rs", False), (base / name / "mod.rs", True)):
                if child.is_file():
                    pending.append((child, root_like))
                    break
    return out


def names_internal(text: str, who: str) -> list[str]:
    """What `who`'s masked `text` names that no embedder may: a crate's
    `internal` module, or one of `INTERNAL_MACROS`."""
    found = []
    if INTERNAL_PATH_RE.search(text):
        found.append(f"{who} names a crate's `internal` module")
    for m in MACRO_RE.finditer(text):
        found.append(f"{who} names `{m.group(1) or m.group(2)}!`")
    return found


def _use_names(tree: str) -> list[str]:
    """The names a `use` tree brings in: each leaf's alias or last segment,
    `self` naming its parent, and `*` a glob."""
    tree = re.sub(r"\s+", " ", tree).strip()

    def expand(prefix: str, s: str) -> list[str]:
        s = s.strip()
        if not s:
            return []
        brace = s.find("{")
        if brace >= 0 and s.endswith("}"):
            head = s[:brace].rstrip(": ").strip()
            parent = f"{prefix}::{head}" if prefix and head else (head or prefix)
            names: list[str] = []
            depth, cur = 0, ""
            for ch in s[brace + 1 : -1]:
                depth += ch == "{"
                depth -= ch == "}"
                if ch == "," and depth == 0:
                    names += expand(parent, cur)
                    cur = ""
                else:
                    cur += ch
            return names + expand(parent, cur)
        if " as " in s:
            return [s.split(" as ")[1].strip()]
        last = s.split("::")[-1].strip()
        if last == "self":
            return [(prefix.split("::")[-1] if prefix else s).strip()]
        return [last]

    return expand("", tree)


def top_level_api(text: str) -> tuple[set[str], list[str]]:
    """What a file exports at its top level -- names declared `pub` or
    re-exported by `pub use` -- and the names of its public modules."""
    depth = _depth_at(text)
    names: set[str] = set()
    modules: list[str] = []
    for m in PUB_ITEM_RE.finditer(text):
        if depth[m.start()] == 0:
            names.add(m.group(1))
            if re.match(r"pub\s+mod\b", m.group(0)):
                modules.append(m.group(1))
    for m in PUB_USE_RE.finditer(text):
        if depth[m.start()] == 0:
            names.update(_use_names(m.group(1)))
    return names, modules


def rust_files(directory: Path) -> list[Path]:
    return sorted(
        p for p in directory.rglob("*.rs") if "target" not in p.relative_to(directory).parts
    )


def problems(meta: dict, repo: Path) -> list[str]:
    found: list[str] = []
    dirs = members(meta)

    def rel(path: Path) -> str:
        return path.relative_to(repo).as_posix() if path.is_relative_to(repo) else str(path)

    # The graph.
    for name in sorted(set(dirs) - set(GRAPH)):
        found.append(f"{name} is a member the graph does not list (D68)")
    for name in sorted(set(GRAPH) - set(dirs)):
        found.append(f"{name} is in the graph but not a member (D68)")
    for name in sorted(set(dirs) & set(GRAPH)):
        for dep in dependencies(meta, name):
            if dep in dirs and dep != name and dep not in GRAPH[name]:
                found.append(f"{name} depends on {dep}, an edge the graph does not draw (D68)")
            if name in LIBRARIES and name != "pgdt-query":
                refused = (
                    ARROW_ABOVE_THE_CODEC if name in NAMES_ARROW_SCHEMA else ARROW_BELOW_THE_CODEC
                )
                if dep in refused:
                    found.append(f"{name} depends on {dep} (D74)")

    libraries = {name: module_tree(dirs[name] / "src") for name in LIBRARIES if name in dirs}

    # What a library's production code names.
    for name, tree in libraries.items():
        for file, text in tree.items():
            where = rel(file)
            needles = ()
            if name == "pgdt-core":
                needles = CORE_NAMES_NO_DEPENDENCY
            elif name in ("pgdt-model", "pgdt-codec"):
                needles = NO_IO
            for needle in needles:
                if needle in text:
                    found.append(f"{where} ({name}) names `{needle}` (D74)")
            if name == "pgdt-codec":
                for m in ARROW_BUFFER_RE.finditer(text):
                    items = [i.strip() for i in m.group(1).strip("{}").split(",") if i.strip()]
                    for item in items:
                        if item != "i256":
                            found.append(f"{where} names `arrow_buffer::{item}`, not `i256` (D74)")

    # No `#[path]`, in any member.
    for name in sorted(dirs):
        for file in rust_files(dirs[name]):
            text = masked(file.read_text(encoding="utf-8"))
            if PATH_ATTR_RE.search(text):
                found.append(f"{rel(file)} loads a module by `#[path]`")

    # Private modules, and one path an item.
    for name in libraries:
        src = dirs[name] / "src"
        lib = src / "lib.rs"
        if not lib.is_file():
            continue
        tree = libraries[name]
        root_names, root_modules = top_level_api(tree[lib])
        namespaces = NAMESPACES.get(name, ())
        for module in root_modules:
            if module != "internal" and module not in namespaces:
                found.append(f"{name}: `pub mod {module}` is not a namespace `NAMESPACES` lists")
        for namespace in namespaces:
            file = next(
                (f for f in (src / f"{namespace}.rs", src / namespace / "mod.rs") if f in tree),
                None,
            )
            if file is None:
                found.append(f"{name}: namespace `{namespace}` has no module file")
                continue
            api, children = top_level_api(tree[file])
            for child in children:
                found.append(f"{rel(file)}: `pub mod {child}` inside namespace `{namespace}`")
            for item in sorted(api & root_names):
                found.append(f"{name}: `{item}` is at its root and in `{namespace}` (one path)")

    # What an embedder names.
    for name in EMBEDDERS:
        if name not in dirs:
            continue
        for file in rust_files(dirs[name]):
            text = masked(file.read_text(encoding="utf-8"))
            found += names_internal(text, f"{rel(file)} ({name})")

    # What the embedder test names.
    crate, path = EMBEDDER_TEST
    if crate in dirs:
        root = dirs[crate] / path
        if not root.is_file():
            found.append(f"{rel(root)}, the embedder test, is missing")
        for file in test_target(root):
            text = masked(file.read_text(encoding="utf-8"))
            who = f"{rel(file)} (the embedder test)"
            found += names_internal(text, who)
            if FIXTURES_RE.search(text):
                found.append(f"{who} names `pgdt_fixtures`, which no embedder depends on")
    return found


def check(repo: Path, out: TextIO, meta: dict | None = None) -> int:
    found = problems(meta if meta is not None else metadata(repo), repo)
    for line in found:
        print(f"!! {line}", file=out)
    print(f"layering: {len(found)} problems" if found else "layering: ok", file=out)
    return 1 if found else 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.parse_args(argv)
    return check(REPO, sys.stdout)


if __name__ == "__main__":
    sys.exit(main())
