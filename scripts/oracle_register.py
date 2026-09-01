#!/usr/bin/env python3
"""The register-to-oracle reconciliation: every comparison arm has evidence,
and every piece of evidence is about an arm.

`docs/design/architecture.md`, "The register-to-oracle reconciliation", is the
description. This module is the join.

**What it exists to check.** The comparison register in
`pgdump_query/src/pgtype.rs` says how a column of a declared type compares and
whether that is PostgreSQL's own order; `scripts/comparison_oracle.py`'s case
table is what PostgreSQL actually answers. Neither knows about the other. The
register grows one arm at a time as types are closed, and an arm added with no
case is a claim nothing checks -- coverage decaying quietly, which is the
failure this reconciliation exists to make loud. The reverse direction is
smaller and real too: a case for a type the oracle's own database does not have
answers `E42704` in every cell, which looks like coverage and is not.

So both directions, failing on either:

* **every register arm resolves to at least one oracle case** -- the direction
  that decays;
* **every oracle case resolves to an arm it can exercise** -- the type is one
  the register walk places, and one at least one supported major actually has.

**An arm is a match arm, read out of the source.** The register's arms cannot
be enumerated from a running program -- a `match` is not data -- so they are
parsed from `pgtype.rs` itself, which is the same direction `deficiencies.py`
reads its markers in. Three functions are read:

* `builtin_scalar` -- **one arm per declared base name**, not per match arm:
  several names share `(Utf8View, text)` and they are separately closable, so
  `character varying` needing a case is not answered by `text` having one.
* `comparison_user_type` -- one arm per `TypeKind` match arm. `Composite |
  Range` and `Base | Shell` are each one arm because each is one decision;
  rustc's exhaustiveness is what makes a *new* kind choose, and a kind that
  joins an existing arm has joined an answer that already has evidence.
* `comparison_for` -- its two branches that are not a match arm at all, the
  array shape and the built-in name nothing recognises, plus
  `comparison_user_type`'s early return for a type absent from the dump's
  `CREATE TYPE` list (I10's multirange companion arrives there). Those three
  are named here rather than parsed, and the parse asserts the anchors they
  hang off so a rewrite is a reported problem rather than a silent pass.
* `collated_text` -- **four more arms, on a second dimension.** A collatable
  built-in's answer depends on the column's `COLLATE` clause as well as its
  declared type, so one match arm carries four answers: a clause the dump
  declares non-deterministic diverges under equality too, an explicit
  `C`/`POSIX` clause agrees, an explicit clause that is not bytewise diverges,
  and no clause at all falls back to the type's own default. Joining on the
  declared type alone collapses those to one, and a fifth could be added with
  nothing behind it. Which built-in arms branch is read from the source too --
  an arm whose body calls `collated_text` is one -- so `character` stops
  counting the moment its arm starts consulting a clause, and starts counting
  the moment it does.

**One of those four carries an exemption, and an exemption names where the
evidence is.** No oracle case can reach `collation/non-deterministic`: a
non-deterministic collation is ICU-only (I42) and an ICU case would import a
`collversion` that moves with the base image, which is the drift this oracle
excludes ICU to avoid. That reason is why the *oracle* cannot cover the arm; it
says nothing about whether anything else does, and an exemption that stops
there is a claim nothing checks -- which is the failure this whole module
exists to make loud, arriving inside the module itself. So an exemption carries
[`Evidence`] pointers as well: `(file, needle)` pairs the check resolves, in
the idiom [`ANCHORS`] already uses for the arms. An exemption then goes stale
from both sides -- it acquires an oracle case, which is reported, or the
evidence it points at disappears, which is reported too.

The pointers name **sufficient** evidence, not exhaustive: a later fixture
covering the same arm adds evidence and owes no edit here.

**Three arms, two case groups.** The oracle asks each text pair under `COLLATE
"C"` and under `COLLATE "default"` only, so the non-`C`-clause arm has no group
of its own: it joins to the `default` group, as the no-clause arm does, because
"asked under something that is not `C`" is one population and the database's
own collation is a member of it. **That mapping is asserted rather than
assumed** -- it holds only while the database's collation is not itself
bytewise, so `datcollate` is read out of each major's `meta.tsv` and a `C` or
`POSIX` apparatus is a reported problem. Without that check, an apparatus
initdb'd under `C` would invert the `default` group's meaning and this join
would go on passing while meaning the opposite thing.

**A case is placed by re-walking `comparison_for`'s three steps** -- array,
then schema-qualified, then the built-in table. That is a second statement of
the walk and it is deliberately the *only* thing this module restates: it
decides which arm a case belongs to, never what the arm answers. The kinds it
needs for a schema-qualified name come from `scripts/fixture_schema_types.sql`,
which is the DDL the oracle's database is loaded from -- so the classification
reads the same source the server did.

Usage:

    cd scripts
    uv run oracle_register.py
    uv run python -m unittest test_oracle_register
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Sequence

import comparison_oracle as co

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "fixtures"

#: The register itself. Arms are read out of it because a `match` is not data.
REGISTER = REPO / "pgdump_query" / "src" / "pgtype.rs"

#: The DDL the oracle's database is loaded from, and therefore the only source
#: that says what kind a `public.*` case names.
SCHEMA = REPO / "scripts" / "fixture_schema_types.sql"

#: SQLSTATE 42704, `undefined_object`, is what a server answers for a type it
#: does not have. A case every major answers that for is a case about nothing:
#: it names an arm and exercises none of it. Every other rejection is an
#: answer -- `json` has no operator (42883), a malformed literal is 22P02.
UNDEFINED_OBJECT = "E42704"


# --------------------------------------------------------------------------
# The arms
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Evidence:
    """Where an exempt arm's coverage actually is: a repo-relative file, and a
    string in it this check resolves.

    Same idiom as [`ANCHORS`], for the same reason. An exemption's *reason*
    says why the oracle cannot reach the arm; only a pointer says what does
    reach it, and only a pointer the check resolves can notice that the
    evidence has been renamed or deleted. It names **sufficient** evidence
    rather than exhaustive, so covering the arm a second way owes no edit here.
    """

    #: Relative to the repository root.
    path: str
    #: A string that must appear in that file -- a test's `fn` line, where the
    #: evidence is a test.
    needle: str


@dataclass(frozen=True)
class Arm:
    """One answer the register can give, and where it is written."""

    #: Stable identity, and what a report prints.
    key: str
    #: The function it was read out of.
    where: str
    #: What kind of arm this is, for grouping a report.
    group: str
    #: Why no oracle case can exist for this arm, or `None` where one is
    #: owed. An arm carrying a reason is not counted uncovered -- and a case
    #: that *does* land on it is reported, because the exemption has then gone
    #: stale and the arm is owed a case after all.
    unoracled: str | None = None
    #: Where the arm's evidence is instead. Required of an exempt arm and
    #: meaningless on any other, both of which [`exemption_problems`] reports.
    evidence: tuple[Evidence, ...] = ()


#: The three branches of the walk that are not match arms. They are named here
#: because there is nothing to enumerate: each is an `if`/`else` in
#: `comparison_for` or `comparison_user_type`, and `parse_register` checks the
#: anchor each hangs off rather than trusting this list to stay true.
STRUCTURAL_ARMS = (
    Arm("array", "comparison_for", "structural"),
    Arm("builtin/unrecognised", "builtin_scalar", "structural"),
    Arm("user/absent", "comparison_user_type", "structural"),
)

#: The second dimension: what a collatable arm answers, given the column's
#: clause. Three branches of one `if`, named here for the same reason the
#: structural arms are, with `parse_register` asserting the anchor they hang
#: off.
COLLATION_ARMS = (
    Arm("collation/bytewise", "collated_text", "collation"),
    Arm("collation/other", "collated_text", "collation"),
    Arm("collation/absent", "collated_text", "collation"),
    Arm(
        "collation/non-deterministic",
        "collated_text",
        "collation",
        unoracled=(
            "a non-deterministic collation is ICU-only (I42), and an ICU case "
            "would carry a collversion that moves with the base image -- the "
            "drift this oracle excludes ICU to avoid"
        ),
        evidence=(
            Evidence(
                "pgdump_query/src/pgtype.rs",
                "fn a_collation_the_dump_declares_non_deterministic_diverges_under_equality_too(",
            ),
            Evidence(
                "pgdump_query/src/pgtype.rs",
                "fn only_the_non_deterministic_collation_reaches_equality(",
            ),
            Evidence(
                "pgdump_query/src/predicate.rs",
                "fn a_non_deterministic_collation_announces_under_equality_and_ordering(",
            ),
        ),
    ),
)

#: Which collation arms a case labelled with each collation exercises. `C` is
#: the bytewise branch; `default` covers *both* remaining branches, because the
#: oracle asks no third collation by name -- `datcollate` is `en_US.utf8` at
#: every major, so `COLLATE "en_US.utf8"` and `COLLATE "default"` would be the
#: same collation and every added cell byte-identical to one already in the
#: file. See [`database_collation_problems`] for what makes that sound.
COLLATION_GROUPS = {
    "C": ("collation/bytewise",),
    "default": ("collation/other", "collation/absent"),
}

#: The call that makes a `builtin_scalar` arm collation-branching. Read out of
#: the arm's own body rather than listed here, so an arm that gains or loses
#: the branch moves on its own.
COLLATED_CALL = "collated_text("

#: A database collation that is itself bytewise would make the `default` case
#: group mean the opposite of what [`COLLATION_GROUPS`] says.
BYTEWISE_COLLATIONS = {"C", "POSIX", "C.UTF-8", "C.utf8"}

#: What `parse_register` must find, or the source has moved under it. Each
#: value is the signature plus **every** string the arm list read out of that
#: function hangs off, so a branch named in [`COLLATION_ARMS`] and deleted from
#: the source is a reported problem rather than an arm nothing can reach.
ANCHORS = {
    "builtin_scalar": ("fn builtin_scalar(", ("_ => return None,",)),
    "comparison_user_type": ("fn comparison_user_type(", ("let Some(def) =",)),
    "comparison_for": (
        "pub fn comparison_for(",
        ("array_element(declared).is_some()",),
    ),
    "collated_text": (
        "fn collated_text(",
        ("type_default == TypeCollation::Bytewise", "states_non_deterministic("),
    ),
}

#: The one match guard this check understands. An arm carrying any other guard
#: is reported rather than classified: a guard decides which arm a case lands
#: in, so mis-reading one is a silent wrong answer.
EMPTY_ENUM_GUARD = "labels.is_empty()"

_FN_RE = "fn {name}\\("
_ARM_NAMES_RE = re.compile(r'^\s*((?:"[^"]*"\s*\|\s*)*"[^"]*")\s*=>')
_KIND_ARM_RE = re.compile(
    r"^\s*((?:TypeKind::\w+(?:\s*\{[^}]*\})?\s*\|\s*)*"
    r"TypeKind::\w+(?:\s*\{[^}]*\})?)\s*(?:if\s+(.+?)\s*)?=>"
)
_KIND_RE = re.compile(r"TypeKind::(\w+)")


def _function_body(text: str, name: str) -> str | None:
    """The source of one `fn`, from its signature's opening brace to the
    matching close. Brace counting is enough here: neither function holds a
    brace inside a string or a comment, and one that did would be reported as
    an anchor failure rather than mis-parsed."""
    match = re.search(_FN_RE.format(name=name), text)
    if not match:
        return None
    start = text.find("{", match.end())
    if start < 0:
        return None
    depth = 0
    for i in range(start, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
    return None


@dataclass
class Register:
    """The arms, as the source states them."""

    arms: list[Arm] = field(default_factory=list)
    #: Declared base name -> arm key, for the built-in table.
    builtin: dict[str, str] = field(default_factory=dict)
    #: `TypeKind` name -> the arm keys that name it, in source order. An arm
    #: carrying the empty-enum guard is the first of two for `Enum`.
    kinds: dict[str, list[str]] = field(default_factory=dict)
    #: Arm keys that only match when an enum has no labels.
    guarded_empty: set[str] = field(default_factory=set)
    #: Arm keys whose answer depends on the column's `COLLATE` clause, read
    #: from the arm's own body. A case on one of these carries its collation
    #: through to [`COLLATION_ARMS`]; a case on any other arm does not, however
    #: it is labelled.
    collatable: set[str] = field(default_factory=set)
    problems: list[str] = field(default_factory=list)


def parse_register(path: Path = REGISTER) -> Register:
    """Every arm of the comparison register, read out of `pgtype.rs`."""
    out = Register()
    if not path.is_file():
        out.problems.append(f"{path} does not exist")
        return out
    text = path.read_text()

    bodies: dict[str, str] = {}
    for name, (signature, anchors) in ANCHORS.items():
        body = _function_body(text, name)
        if body is None or signature not in text:
            out.problems.append(
                f"{path}: no `{name}` to read arms from — the register has moved"
            )
            continue
        for anchor in anchors:
            if anchor not in body:
                out.problems.append(
                    f"{path}: `{name}` no longer contains {anchor!r} — this check's "
                    "reading of it is out of date"
                )
        bodies[name] = body
    if len(bodies) != len(ANCHORS):
        return out

    # One chunk per arm — head line through to the next arm's head — because
    # whether the arm branches on the clause is a fact about its *body*, and a
    # body is routinely on the line after the `=>`.
    lines = bodies["builtin_scalar"].splitlines()
    heads = [(i, m) for i, line in enumerate(lines) if (m := _ARM_NAMES_RE.match(line))]
    for position, (start, match) in enumerate(heads):
        stop = heads[position + 1][0] if position + 1 < len(heads) else len(lines)
        collatable = COLLATED_CALL in "\n".join(lines[start:stop])
        for name in re.findall(r'"([^"]*)"', match.group(1)):
            key = f"builtin/{name}"
            out.builtin[name] = key
            out.arms.append(Arm(key, "builtin_scalar", "builtin"))
            if collatable:
                out.collatable.add(key)
    if not out.builtin:
        out.problems.append(f"{path}: `builtin_scalar` has no named arms")
    if not out.collatable:
        out.problems.append(
            f"{path}: no arm of `builtin_scalar` calls `collated_text` — the "
            "collation dimension has moved and this check can no longer place a "
            "collated case"
        )

    for line in bodies["comparison_user_type"].splitlines():
        match = _KIND_ARM_RE.match(line)
        if not match:
            continue
        kinds = _KIND_RE.findall(match.group(1))
        guard = match.group(2)
        key = "user/" + "|".join(kinds)
        if guard is not None:
            if guard != EMPTY_ENUM_GUARD:
                out.problems.append(
                    f"{path}: `comparison_user_type` has an arm guarded by "
                    f"{guard!r}, which this check cannot place a case against"
                )
                continue
            key += "(empty)"
            out.guarded_empty.add(key)
        out.arms.append(Arm(key, "comparison_user_type", "user"))
        for kind in kinds:
            out.kinds.setdefault(kind, []).append(key)
    if not out.kinds:
        out.problems.append(f"{path}: `comparison_user_type` has no `TypeKind` arms")

    out.arms.extend(STRUCTURAL_ARMS)
    out.arms.extend(COLLATION_ARMS)
    return out


# --------------------------------------------------------------------------
# The kinds the oracle's own database holds
# --------------------------------------------------------------------------


@dataclass(frozen=True)
class Declared:
    """One type the oracle's database declares, and its `TypeKind`."""

    name: str
    kind: str
    #: Whether an enum's label list is empty, which is the one thing an arm
    #: guard turns on. `False` for every other kind.
    empty_enum: bool = False


_CREATE_DOMAIN_RE = re.compile(r"^CREATE DOMAIN\s+(\S+)\s+AS\b", re.MULTILINE)
_CREATE_TYPE_RE = re.compile(r"^CREATE TYPE\s+([^\s;(]+)\s*(.*)$", re.MULTILINE)
_MULTIRANGE_RE = re.compile(r"multirange_type_name\s*=\s*([^\s,)]+)")


def parse_schema(path: Path = SCHEMA) -> tuple[dict[str, Declared], set[str], list[str]]:
    """What `fixture_schema_types.sql` declares: every type by name, and the
    multirange companions PG14+ creates on its own.

    A companion is named but never declared -- `pg_dump` writes no `CREATE
    TYPE` for it either (I10), which is exactly why it reaches the register's
    absent arm. It is tracked separately so that "absent" can be told from "a
    case naming a type nothing creates".

    One entry per type name, the completion winning over the shell it follows
    (I11), mirrors `preamble.rs`'s `record_type` rather than being a local
    convenience: the two walks must agree on which arm a case lands on, and a
    dict that kept the shell would place `public.mybase` on `Shell` while the
    library placed it on `Base`.
    """
    problems: list[str] = []
    if not path.is_file():
        return {}, set(), [f"{path} does not exist"]
    text = path.read_text()

    declared: dict[str, Declared] = {}
    for match in _CREATE_DOMAIN_RE.finditer(text):
        declared[match.group(1)] = Declared(match.group(1), "Domain")
    for match in _CREATE_TYPE_RE.finditer(text):
        name, tail = match.group(1), match.group(2).strip()
        if tail.startswith("AS ENUM"):
            # `AS ENUM ()` is the one CREATE TYPE form with no labels, and the
            # only thing an arm guard turns on.
            statement = text[match.start() :].split(";", 1)[0]
            declared[name] = Declared(name, "Enum", empty_enum="'" not in statement)
        elif tail.startswith("AS RANGE"):
            declared[name] = Declared(name, "Range")
        elif tail.startswith("AS ("):
            declared[name] = Declared(name, "Composite")
        elif tail.startswith("("):
            # The completed half of a base type, which follows its own shell.
            declared[name] = Declared(name, "Base")
        elif tail in ("", ";"):
            # A shell, unless a completion later in the file overrides it.
            declared.setdefault(name, Declared(name, "Shell"))
        else:
            problems.append(f"{path}: `CREATE TYPE {name} {tail}` was not understood")

    companions = set(_MULTIRANGE_RE.findall(text))
    return declared, companions, problems


# --------------------------------------------------------------------------
# Placing a case
# --------------------------------------------------------------------------


def split_typmod(declared: str) -> str:
    """The base name of a declared type — `pgtype.rs`'s `split_typmod`, first
    half only. This check never needs the typmod: `numeric` is one arm of the
    built-in table however its precision reads, and which Arrow decimal that
    precision picks is a mapping question the Rust register test owns."""
    i = declared.find("(")
    if i >= 0 and declared.endswith(")"):
        return declared[:i].rstrip()
    return declared


def is_array(declared: str) -> bool:
    """Whether the declared type is an array — `pgtype.rs`'s `array_element`,
    to the depth a case table can hold. The full grammar takes `ARRAY[n]` and
    repeated bounds; a case that used one and was misplaced here would still
    be placed in *some* arm, so the risk this simplification carries is a
    weaker report, never a wrong one."""
    stripped = declared.rstrip()
    return stripped.endswith("]") or stripped.upper().endswith(" ARRAY")


def arm_for(
    declared: str,
    register: Register,
    schema: dict[str, Declared],
    companions: set[str],
) -> tuple[str | None, str]:
    """Which arm a declared type reaches, and why — `comparison_for`'s three
    steps in order. `None` is a type the walk places nowhere useful, which is
    the second direction's failure."""
    declared = declared.strip()
    if is_array(declared):
        return "array", "an array shape, refused before anything else is asked"
    base = split_typmod(declared)
    if "." in base:
        found = schema.get(base)
        if found is None:
            if base in companions:
                return (
                    "user/absent",
                    "a multirange companion, which no CREATE TYPE declares (I10)",
                )
            return None, (
                f"{base} is not declared by {SCHEMA.name} and is no range's "
                "multirange companion — every cell of this case is a server "
                "saying the type does not exist"
            )
        keys = register.kinds.get(found.kind)
        if not keys:
            return None, f"the register has no arm for TypeKind::{found.kind}"
        # Source order, guards honoured — which is how the `match` reads it.
        for key in keys:
            if key in register.guarded_empty and not found.empty_enum:
                continue
            return key, f"a {found.kind.lower()} declared by {SCHEMA.name}"
        return None, f"no arm of the register accepts TypeKind::{found.kind} here"
    key = register.builtin.get(base.lower())
    if key:
        return key, "a built-in this register maps"
    return (
        "builtin/unrecognised",
        "no arm of the built-in table names it, so it has no order here",
    )


# --------------------------------------------------------------------------
# The evidence a case actually carries
# --------------------------------------------------------------------------


def majors(fixtures: Path = FIXTURES) -> list[str]:
    return sorted(
        (p.name for p in fixtures.iterdir() if p.is_dir()),
        key=lambda name: int(name) if name.isdigit() else 0,
    )


def database_collation_problems(fixtures: Path = FIXTURES) -> list[str]:
    """Whatever stops the `default` case group meaning "not bytewise".

    [`COLLATION_GROUPS`] maps a case asked under `COLLATE "default"` onto the
    two arms a *non*-bytewise clause reaches, which is sound only while the
    database's own collation is non-bytewise. An apparatus initdb'd under `C`
    would invert that and the join would go on passing while meaning the
    opposite thing, so the premise is read off the apparatus rather than
    assumed.
    """
    problems: list[str] = []
    for version in majors(fixtures):
        path = fixtures / version / co.ORACLE_DIRNAME / "meta.tsv"
        if not path.is_file():
            problems.append(f"{version} has no meta.tsv — regenerate its oracle")
            continue
        meta = {row[0]: row[1] for row in co.parse_tsv(path.read_text()) if len(row) > 1}
        collation = meta.get("datcollate")
        if collation is None:
            problems.append(f"{version}/meta.tsv records no datcollate")
        elif collation in BYTEWISE_COLLATIONS:
            problems.append(
                f"{version} was generated under datcollate={collation!r}, which is "
                "bytewise — a case asked under COLLATE \"default\" then exercises "
                "the agreeing arm, not the diverging ones this check maps it to"
            )
    return problems


def exemption_problems(arms: Sequence[Arm], root: Path = REPO) -> list[str]:
    """Whatever stops an exemption meaning what it says.

    Three things, and the first is the one this check was rewritten for: an
    exempt arm that names no evidence is a reason nobody can falsify, so the
    arm is uncovered in every sense that matters and nothing says so. The
    second is the pointer going stale -- a renamed test resolves nowhere, which
    is the state an exemption asserting its own sufficiency cannot reach. The
    third is the reverse, an arm carrying evidence with no exemption to
    justify it, which is a pointer nothing reads.
    """
    problems: list[str] = []
    for arm in arms:
        if arm.unoracled is None:
            if arm.evidence:
                problems.append(
                    f"{arm.key} names evidence but is not exempt — evidence stands "
                    "in for an oracle case, and this arm is owed one"
                )
            continue
        if not arm.evidence:
            problems.append(
                f"{arm.key} is exempt from needing an oracle case and names no "
                "evidence — an exemption says where the arm's coverage is, not "
                "only why the oracle cannot be it"
            )
            continue
        for pointer in arm.evidence:
            path = root / pointer.path
            if not path.is_file():
                problems.append(
                    f"{arm.key}: {pointer.path} does not exist — the evidence this "
                    "exemption stands on has moved"
                )
            elif pointer.needle not in path.read_text():
                problems.append(
                    f"{arm.key}: {pointer.path} no longer contains "
                    f"{pointer.needle!r} — the evidence this exemption stands on "
                    "has gone"
                )
    return problems


def types_the_servers_have(fixtures: Path = FIXTURES) -> tuple[set[str], list[str]]:
    """Every case type at least one major answered something other than
    "no such type" for.

    A case whose every cell on every major is `E42704` names an arm and
    exercises none of it: the server never had the type, so the answers say
    nothing about how it compares. That is `comparison_oracle`'s own warning --
    a case needing a type `fixture_schema_types.sql` does not declare must be
    added there first -- turned into a check.
    """
    problems: list[str] = []
    have: set[str] = set()
    first_op = len(co.COMPARISON_COLUMNS) - len(co.OPERATORS)
    for version in majors(fixtures):
        path = fixtures / version / co.ORACLE_DIRNAME / "comparisons.tsv"
        if not path.is_file():
            problems.append(f"{version} has no comparisons.tsv — regenerate its oracle")
            continue
        for row in co.parse_tsv(path.read_text()):
            if any(cell != UNDEFINED_OBJECT for cell in row[first_op:]):
                have.add(row[0])
    return have, problems


# --------------------------------------------------------------------------
# The reconciliation
# --------------------------------------------------------------------------


@dataclass
class Reconciliation:
    """Both directions, and everything that stopped either being answered."""

    #: Arm key -> the cases that reach it, in case-table order, labelled the
    #: way [`case_label`] writes them.
    cases_by_arm: dict[str, list[str]] = field(default_factory=dict)
    #: Case label -> why it reaches no arm.
    unplaced: dict[str, str] = field(default_factory=dict)
    arms: list[Arm] = field(default_factory=list)
    problems: list[str] = field(default_factory=list)

    @property
    def uncovered(self) -> list[Arm]:
        return [
            arm
            for arm in self.arms
            if arm.unoracled is None and not self.cases_by_arm.get(arm.key)
        ]

    @property
    def exempt(self) -> list[Arm]:
        """Arms for which no oracle case can exist, with the reason. Reported
        rather than hidden: an exemption nobody reads is the same silence as an
        arm nobody covers."""
        return [arm for arm in self.arms if arm.unoracled is not None]


def case_label(type_name: str, collation: str | None) -> str:
    """How a case is named in a report: the declared type, plus the collation
    it is asked under where it is asked under one."""
    return type_name if collation is None else f'{type_name} COLLATE "{collation}"'


def reconcile(
    register_path: Path = REGISTER,
    schema_path: Path = SCHEMA,
    fixtures: Path = FIXTURES,
    evidence_root: Path = REPO,
) -> Reconciliation:
    out = Reconciliation()
    register = parse_register(register_path)
    out.arms = register.arms
    out.problems += register.problems
    schema, companions, schema_problems = parse_schema(schema_path)
    out.problems += schema_problems
    if out.problems:
        return out

    have, oracle_problems = types_the_servers_have(fixtures)
    out.problems += oracle_problems
    out.problems += database_collation_problems(fixtures)
    out.problems += exemption_problems(out.arms, evidence_root)

    # The key is `(type, collation)`, not the type alone: a text pair asked
    # under two collations is two cases about two different arms, which is the
    # whole of what the second dimension buys.
    seen: set[tuple[str, str | None]] = set()
    for case in co.TYPE_CASES:
        if (case.type, case.collation) in seen:
            continue
        seen.add((case.type, case.collation))
        label = case_label(case.type, case.collation)
        key, why = arm_for(case.type, register, schema, companions)
        if key is None:
            out.unplaced[label] = why
            continue
        if case.type not in have:
            out.unplaced[label] = (
                "no major answers anything but “no such type” for it, so it "
                "exercises nothing"
            )
            continue
        out.cases_by_arm.setdefault(key, []).append(label)
        if case.collation is None or key not in register.collatable:
            # Either the case names no collation, or the arm it lands on does
            # not read one. Every collatable built-in reads one today, so the
            # second branch is what keeps a label on a *non*-collatable arm
            # from buying coverage it does not have.
            continue
        groups = COLLATION_GROUPS.get(case.collation)
        if groups is None:
            out.problems.append(
                f"{label}: `{case.collation}` is not a collation this check can "
                f"place — it knows {sorted(COLLATION_GROUPS)}"
            )
            continue
        for arm_key in groups:
            out.cases_by_arm.setdefault(arm_key, []).append(label)

    # An exemption that acquired a case has gone stale: the arm is no longer
    # one no evidence can exist for, and leaving it exempt would excuse the
    # *next* arm someone hangs off the same reason.
    for arm in out.exempt:
        covering = out.cases_by_arm.get(arm.key)
        if covering:
            out.problems.append(
                f"{arm.key} is marked as needing no oracle case and now has one "
                f"({', '.join(covering)}) — drop the exemption"
            )
    return out


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def report(found: Reconciliation, out=sys.stdout) -> None:
    cases = len({label for labels in found.cases_by_arm.values() for label in labels})
    print(
        f"{len(found.arms)} register arms, {cases} oracle cases "
        f"({len(found.unplaced)} placed nowhere).\n",
        file=out,
    )
    for group, label in (
        ("builtin", "builtin_scalar"),
        ("user", "comparison_user_type"),
        ("structural", "the walk itself"),
        ("collation", "collated_text (the clause)"),
    ):
        arms = [arm for arm in found.arms if arm.group == group]
        if not arms:
            continue
        print(f"{label}:", file=out)
        for arm in arms:
            covering = found.cases_by_arm.get(arm.key, [])
            if covering:
                mark, shown = "  ", ", ".join(covering)
            elif arm.unoracled is not None:
                mark, shown = "  ", "no case can exist (see below)"
            else:
                mark, shown = "!!", "no case"
            print(f" {mark} {arm.key:<38} {shown}", file=out)
        print(file=out)

    if found.exempt:
        print("Arms no oracle case can cover, and where their evidence is:", file=out)
        for arm in found.exempt:
            print(f"  {arm.key} ({arm.where}): {arm.unoracled}", file=out)
            for pointer in arm.evidence:
                print(f"      {pointer.path}: {pointer.needle}", file=out)
        print(file=out)

    if found.unplaced:
        print("Cases that resolve to no arm they exercise:", file=out)
        for name, why in found.unplaced.items():
            print(f"  {name}: {why}", file=out)
        print(file=out)

    uncovered = found.uncovered
    if uncovered:
        print("Arms with no oracle case — add one to comparison_oracle.py:", file=out)
        for arm in uncovered:
            print(f"  {arm.key} ({arm.where})", file=out)
        print(file=out)

    if found.problems:
        print("Problems:", file=out)
        for problem in found.problems:
            print(f"  {problem}", file=out)
    elif not uncovered and not found.unplaced:
        exempt = f" (bar {len(found.exempt)} no case can cover)" if found.exempt else ""
        print(
            f"Every arm of the comparison register has an oracle case{exempt}, and "
            "every oracle case exercises an arm.",
            file=out,
        )


def check(
    register_path: Path = REGISTER,
    schema_path: Path = SCHEMA,
    fixtures: Path = FIXTURES,
    out=sys.stdout,
    evidence_root: Path = REPO,
) -> int:
    found = reconcile(register_path, schema_path, fixtures, evidence_root)
    report(found, out=out)
    return 1 if found.problems or found.uncovered or found.unplaced else 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--fixtures", type=Path, default=FIXTURES, help="fixture root (tests use this)"
    )
    parser.add_argument(
        "--register", type=Path, default=REGISTER, help="the Rust file holding the arms"
    )
    parser.add_argument(
        "--schema", type=Path, default=SCHEMA, help="the DDL the oracle's database uses"
    )
    args = parser.parse_args(argv)
    return check(args.register, args.schema, args.fixtures)


if __name__ == "__main__":
    raise SystemExit(main())
