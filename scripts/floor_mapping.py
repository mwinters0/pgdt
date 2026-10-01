#!/usr/bin/env python3
"""The floor-to-mapping reconciliation: every type the ADBC driver gives a real
Arrow type to is answered by our own mapping, and every arm of that mapping is
answered by the floor.

`docs/design/decisions.md`, "D37", states the rule and carries the
stance every row outside it declares; `docs/design/decisions.md`, "D38", describes the evidence. This module is the join.

**The rule, restated only far enough to compute it.** Wherever
`adbc_driver_postgresql` yields a *non-opaque* Arrow type for a declared type,
ours is never a widening of it. `fixtures/<major>/adbc/floor.tsv` is what the
driver answers; `builtin_scalar` in `pgdump_query/src/pgtype.rs` is what we
answer, with `Utf8View` the fallback for every name it does not carry. Neither
knows about the other, and both move: a type mapped without evidence and a
floor row nothing answers are the two ways this decays, so both directions
fail.

* **every floor row the rule reaches is answered** -- either our Arrow type is
  the driver's, or the row carries a [`Disposition`] saying why not;
* **every `builtin_scalar` arm resolves to a floor row** -- an arm naming a
  type no supported major has is a mapping decision with nothing behind it;
* **every disposition is about a row that still needs one** -- the direction
  that makes a waiting row close itself. When 12.6 maps `int2vector`, its row
  starts meeting the floor and the exemption standing over it is reported as
  stale rather than sitting there excusing a type that no longer needs it.

**What places a row outside the rule is a column of the file, not a list.**
`status` is `E<sqlstate>` where the driver cannot read the type at all, and
`extension` is `arrow.opaque` where its answer is raw wire bytes plus a type
name -- the driver's bottom, which our own bottom (the file's text) is not
comparable with and is more useful than. That is the whole long tail --
`bit`, `inet`, the geometric family, every range and multirange, `numeric`,
`oidvector` -- carried with no per-type line, which is what keeps the phase's
coverage unbounded while its work stays bounded.

**A disposition covers what the columns cannot say**, and there are four.
Three are D2's stances proper; the fourth is `oid`, which satisfies the rule by
answering a *different* type rather than a wider one, and says so because "not
equal" is all this check can compute on its own.

**The comparison is over the Arrow type, not the field's metadata.** Release 24
stamps `arrow.json` on `json`/`jsonb` and nothing else; we stamp the same two
plus `arrow.uuid`, so the metadata floor is already met everywhere and joining
on it would check a thing that cannot fail. The `extension` column is read
here only for `arrow.opaque`, which is a statement about the *type*.

**Arrow types are compared in the file's own vocabulary.** [`ARROW_RENDERING`]
takes each `DataType` expression `builtin_scalar` writes to the spelling
pyarrow gives the same values, so `Utf8View` is `string` -- one layout of the
same value space, and a floor is a claim about values. A `DataType` this table
does not carry is reported rather than guessed at, which is what makes a new
arm arrive loudly.

**The rule binds the typed mode alone.** The untyped mode, like
`--schema-mode strings`, is the user asking for a type wider than the floor,
widening only a column the map says holds a value its type cannot
(`docs/design/decisions.md`, "D38"), so nothing here is about it.

**Every typed arm has its extremes, and every extremes column an arm.** The
`types` fixture's `public.t_extremes` holds PostgreSQL's least, greatest and
special values of each type an arm maps to an Arrow type other than
`Utf8View`, one column per arm, and the category of values an Arrow type
cannot hold is read off those values rather than written per type
(`docs/design/decisions.md`, "D102"). So a floor row a new
major brings forces a mapping decision above, and a typed mapping forces its
extremes here: a typed arm no column declares, a column no typed arm answers,
and a major whose `types` dump holds no such table are each a problem.

**The pin is asserted here** (the spec's D8): the driver version every floor
row records must equal `scripts/pyproject.toml`'s. The floor is a claim about
one release, and bumping the pin is what obliges re-taking the oracle -- so the
two are made unable to drift rather than trusted not to. The manual page that
publishes the floor to a user names the release too, and is held to the pin
the same way.

Usage:

    cd scripts
    uv run floor_mapping.py
    uv run python -m unittest test_floor_mapping
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Sequence

import adbc_floor
import deficiencies

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "fixtures"

#: The mapping itself. Arms are read out of it because a `match` is not data --
#: `oracle_register.py`'s reading of the same file, for the same reason.
MAPPING = REPO / "pgdump_query" / "src" / "pgtype.rs"

#: Where the driver pin lives. One value, in one place, asserted against the
#: rows rather than restated here.
PYPROJECT = REPO / "scripts" / "pyproject.toml"

#: The page that publishes the floor to a user, which names the release it
#: holds for, so moving the pin obliges re-taking the sweep.
MANUAL = REPO / "docs" / "manual" / "datafusion-cli-pgdump.md"

#: The register the `money` disposition cites.
REGISTER = deficiencies.REGISTER

#: The checklist a waiting disposition names a slice of.
STATUS = deficiencies.STATUS

#: The extension name that takes a row out of the rule. The driver's bottom is
#: raw binary plus a type name; ours is the file's own text, and the two are
#: incomparable rather than one being below the other.
OPAQUE = "arrow.opaque"

#: The table holding each typed arm's extremes, in each major's `types` dump.
EXTREMES_TABLE = "public.t_extremes"

#: What our fallback is for a declared type `builtin_scalar` does not name.
#: Stated once here because the join needs it for two thirds of the file:
#: `"char"` and `refcursor` meet the floor precisely because the driver's own
#: answer for them is a string too.
FALLBACK = "string"


# --------------------------------------------------------------------------
# Rendering our Arrow types into the file's vocabulary
# --------------------------------------------------------------------------

#: Each `DataType` expression `builtin_scalar` can yield, in the spelling
#: pyarrow writes for the same values -- which is what `floor.tsv`'s `arrow`
#: column holds.
#:
#: **Value space, not layout.** `Utf8View` renders as `string` because a floor
#: is a claim about which values a column can hold, and `Utf8`, `LargeUtf8` and
#: `Utf8View` hold the same ones in three layouts. Reading the difference as a
#: floor violation would report every text column in the file forever.
#:
#: An expression absent from this table is a **problem**, never a guess. A new
#: arm is exactly the moment somebody must say what the driver's answer for
#: that type is, and a table that fell back to "assume it meets" would let the
#: arm land silently -- the decay this whole module exists to make loud.
ARROW_RENDERING = {
    "Int16": "int16",
    "Int32": "int32",
    "Int64": "int64",
    "UInt32": "uint32",
    "Boolean": "bool",
    "Float32": "float",
    "Float64": "double",
    "Utf8View": "string",
    "Binary": "binary",
    "FixedSizeBinary(16)": "fixed_size_binary[16]",
    "Date32": "date32[day]",
    "Time64(Microsecond)": "time64[us]",
    "Timestamp(Microsecond, None)": "timestamp[us]",
    'Timestamp(Microsecond, Some("UTC".into()))': "timestamp[us, tz=UTC]",
    "Interval(MonthDayNano)": "month_day_nano_interval",
    # The one container `builtin_scalar` yields. pyarrow spells the element
    # field's *name* into the type, and `list_of` names it `item`, so this is
    # keyed on the whole expression like every other entry rather than being
    # composed out of the element's row.
    "list_of(Int16)": "list<item: int16>",
}


# --------------------------------------------------------------------------
# The dispositions
# --------------------------------------------------------------------------

#: What a disposition claims about a row the floor reaches and our mapping does
#: not meet. The first four are D2's stances; `narrower` is the fifth case,
#: which is *inside* the rule rather than outside it.
STANCES = {
    "different-encodings": (
        "the COPY TEXT and binary encodings denote different values, so "
        "'narrower than theirs' is not a question the text can be asked"
    ),
    "below-by-decision": (
        "refused by our own bar -- the dump alone does not determine the value"
    ),
    "waiting": "a floor row this phase closes, in the slice named",
    "narrower": (
        "a different Arrow type over the same bytes, which the rule permits; "
        "only a *wider* one is a violation"
    ),
}

#: A stance that must sit on a row whose Arrow type we do not model at all.
#: `narrower` is the one that must not: it claims both sides are real types.
BELOW_STANCES = {"different-encodings", "below-by-decision", "waiting"}


@dataclass(frozen=True)
class Disposition:
    """Why one floor row the rule reaches is not simply met.

    An entry here is a claim that goes stale from both sides, the way
    `oracle_register.py`'s exemptions do: the row starts meeting the floor,
    which is reported so the entry goes; or the slice or register entry it
    cites stops existing, which is reported too.
    """

    #: `format_type(oid, NULL)`, the file's own join key.
    declared: str
    #: One of [`STANCES`].
    stance: str
    #: The specific reason, beyond what the stance says generically.
    reason: str
    #: The slice that closes it, for a `waiting` stance. Resolved against
    #: `STATUS.md`'s checklist, so a re-slice is reported rather than owed on
    #: discipline.
    closes: str | None = None
    #: The register entry that carries it, for a row nothing will ever close.
    #: Resolved against `deficiencies.md`'s index.
    deficiency: str | None = None


#: Every row the floor reaches that our mapping does not meet. Three, and the
#: file's own columns place everything else.
DISPOSITIONS = (
    Disposition(
        "money",
        "below-by-decision",
        "`cash_out` reads the monetary locale -- `frac_digits`, "
        "`mon_decimal_point`, `mon_thousands_sep`, `currency_symbol`, "
        "`mon_grouping` -- and `pg_dump` sets `lc_monetary` nowhere, so "
        "`1.234,56` and `1,234.56` cannot be told apart from the file. ADBC "
        "escapes this only because binary hands it the raw int64",
        deficiency="KD13",
    ),
    Disposition(
        "regproc",
        "different-encodings",
        "`regprocout` writes the function's *name* -- schema-qualified where "
        "the bare name would not resolve, `-` for InvalidOid -- where the "
        "binary encoding ADBC reads is the OID. The one member of the `reg*` "
        "family the driver gives a real Arrow type to",
    ),
    Disposition(
        "oid",
        "narrower",
        "`oidout` is `snprintf(\"%u\")` (I39), so `UInt32` is what the file "
        "says and the driver's `Int32` turns every OID at or above 2^31 "
        "negative -- a narrower reading of the same bytes, not a wider one",
    ),
)


# --------------------------------------------------------------------------
# The arms
# --------------------------------------------------------------------------

#: What `parse_mapping` must find, or the source has moved under it. The
#: `ANCHORS` idiom `oracle_register.py` reads the same file with: the
#: signature, plus every string this module's reading of the function hangs
#: off.
ANCHORS = ("fn builtin_scalar(", "_ => return None,")

_ARM_HEAD_RE = re.compile(r'^\s*((?:"[^"]*"\s*\|\s*)*"[^"]*")\s*=>')
_NAME_RE = re.compile(r'"([^"]*)"')


def _function_body(text: str, name: str) -> str | None:
    """The source of one `fn`, from its signature's opening brace to the
    matching close. Brace counting is enough: `builtin_scalar` holds no brace
    inside a string or a comment, and one that did would fail the anchor check
    rather than be mis-parsed."""
    match = re.search(rf"fn {name}\(", text)
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


def _first_tuple_element(body: str) -> str | None:
    """The `DataType` half of an arm's `(DataType, ComparisonPlan)`.

    `None` where the arm does not open a tuple at all -- `numeric` delegates to
    `map_numeric(typmod)`, whose answer depends on the typmod. That is not a
    parse failure to paper over: it is reported wherever the floor actually
    needs our type, and silent where the floor is undefined for the row, which
    is where `numeric` sits.
    """
    i = 0
    while i < len(body) and body[i] in " \t\r\n{":
        i += 1
    if i >= len(body) or body[i] != "(":
        return None
    depth = 0
    start = i + 1
    for j in range(i, len(body)):
        if body[j] == "(":
            depth += 1
        elif body[j] == ")":
            depth -= 1
            if depth == 0:
                return None
        elif body[j] == "," and depth == 1:
            return " ".join(body[start:j].split())
    return None


@dataclass
class Mapping:
    """`builtin_scalar`, as the source states it."""

    #: Declared base name -> the pyarrow spelling of the type it yields, or
    #: `None` where the arm's `DataType` could not be read.
    arms: dict[str, str | None] = field(default_factory=dict)
    #: The raw `DataType` expression per arm, for a report that has to name
    #: what it could not render.
    expressions: dict[str, str | None] = field(default_factory=dict)
    problems: list[str] = field(default_factory=list)


def parse_mapping(path: Path = MAPPING) -> Mapping:
    """Every arm of `builtin_scalar`, and the Arrow type each yields."""
    out = Mapping()
    if not path.is_file():
        out.problems.append(f"{path} does not exist")
        return out
    text = path.read_text()
    body = _function_body(text, "builtin_scalar")
    if body is None or ANCHORS[0] not in text:
        out.problems.append(
            f"{path}: no `builtin_scalar` to read arms from — the mapping has moved"
        )
        return out
    for anchor in ANCHORS[1:]:
        if anchor not in body:
            out.problems.append(
                f"{path}: `builtin_scalar` no longer contains {anchor!r} — this "
                "check's reading of it is out of date"
            )

    lines = body.splitlines()
    heads = [(i, m) for i, line in enumerate(lines) if (m := _ARM_HEAD_RE.match(line))]
    for position, (start, match) in enumerate(heads):
        stop = heads[position + 1][0] if position + 1 < len(heads) else len(lines)
        chunk = "\n".join(lines[start:stop])
        expression = _first_tuple_element(chunk[chunk.index("=>") + 2 :])
        rendered = None if expression is None else ARROW_RENDERING.get(expression)
        if expression is not None and rendered is None:
            out.problems.append(
                f"{path}: `builtin_scalar` yields `{expression}`, which this "
                "check cannot render into the floor's vocabulary — add it to "
                "ARROW_RENDERING beside the pyarrow spelling of the same values"
            )
        for name in _NAME_RE.findall(match.group(1)):
            out.arms[name] = rendered
            out.expressions[name] = expression
    if not out.arms:
        out.problems.append(f"{path}: `builtin_scalar` has no named arms")
    return out


# --------------------------------------------------------------------------
# The pin
# --------------------------------------------------------------------------


def pinned_driver(path: Path = PYPROJECT) -> tuple[str | None, list[str]]:
    """The `adbc_driver_postgresql` version `scripts/pyproject.toml` pins."""
    if not path.is_file():
        return None, [f"{path} does not exist"]
    data = tomllib.loads(path.read_text())
    wanted = adbc_floor.DRIVER_DIST.replace("_", "-")
    for requirement in data.get("project", {}).get("dependencies", []):
        name, _, version = requirement.partition("==")
        if name.strip().replace("_", "-").lower() == wanted and version:
            return version.strip(), []
    return None, [
        f"{path.name} pins no exact `{wanted}` version — the floor is a claim "
        "about one release (the spec's D8) and nothing else records which"
    ]


def published_pin_problems(pin: str | None, manual: Path = MANUAL) -> list[str]:
    """The manual names the pinned release as `` `adbc-driver-postgresql` <pin> ``.

    A moved pin re-takes the sweep, and a page still naming the old release
    would publish a floor nobody checked against the new one."""
    if pin is None:
        return []
    wanted = f"`{adbc_floor.DRIVER_DIST.replace('_', '-')}` {pin}"
    if not manual.is_file():
        return [f"{manual} does not exist — it is where the floor is published"]
    if wanted not in manual.read_text():
        return [
            f"{manual.name} does not name {wanted}: the floor it publishes is the "
            "pinned release's, so a moved pin re-writes the page with the sweep"
        ]
    return []


# --------------------------------------------------------------------------
# Placing a floor row
# --------------------------------------------------------------------------


def majors(fixtures: Path = FIXTURES) -> list[str]:
    return sorted(
        (p.name for p in fixtures.iterdir() if p.is_dir()),
        key=lambda name: int(name) if name.isdigit() else 0,
    )


@dataclass(frozen=True)
class Verdict:
    """What the join says about one declared type."""

    declared: str
    #: `meets`, `differs`, `below`, `unreadable`, or `undefined`.
    kind: str
    #: The floor's Arrow type, or `None` where the floor is undefined.
    theirs: str | None
    #: Ours, in the same vocabulary.
    ours: str | None
    #: Why the floor is undefined, where it is.
    why: str = ""


def verdict_for(row: dict[str, str | None], mapping: Mapping) -> Verdict:
    """One floor row against our mapping.

    The two `undefined` answers come off the row's own columns, which is what
    keeps the opaque tail free: a refusal (`status`) is a floor of nothing, and
    `arrow.opaque` is a floor our own bottom is not comparable with.
    """
    declared = row["declared"] or ""
    if row["status"] != "ok":
        return Verdict(
            declared, "undefined", None, None, f"the driver refused it ({row['status']})"
        )
    if row["extension"] == OPAQUE:
        return Verdict(
            declared,
            "undefined",
            None,
            None,
            f"{OPAQUE} over {row['arrow']} — raw wire bytes plus a type name",
        )
    theirs = row["arrow"]
    if declared in mapping.arms:
        ours = mapping.arms[declared]
        if ours is None:
            return Verdict(declared, "unreadable", theirs, None)
    else:
        ours = FALLBACK
    if ours == theirs:
        return Verdict(declared, "meets", theirs, ours)
    if ours == FALLBACK:
        return Verdict(declared, "below", theirs, ours)
    return Verdict(declared, "differs", theirs, ours)


# --------------------------------------------------------------------------
# The extremes
# --------------------------------------------------------------------------

_TYPMOD_RE = re.compile(r"\(\s*-?\d+(?:\s*,\s*-?\d+)?\s*\)")


def extremes_columns(dump: Path, table: str = EXTREMES_TABLE) -> dict[str, str] | None:
    """Each column of `table`'s `CREATE TABLE` in `dump` but `id`, and the
    base name of the type it declares -- a typmod dropped, the way
    `builtin_scalar` is keyed. `None` where the dump holds no such table."""
    if not dump.is_file():
        return None
    lines = dump.read_text().splitlines()
    head = f"CREATE TABLE {table} ("
    try:
        start = lines.index(head)
    except ValueError:
        return None
    columns: dict[str, str] = {}
    for line in lines[start + 1 :]:
        if line.startswith(")"):
            break
        name, _, declared = line.strip().rstrip(",").partition(" ")
        if name != "id":
            columns[name] = " ".join(_TYPMOD_RE.sub("", declared).split())
    return columns


def typed_arms(mapping: Mapping) -> list[str]:
    """Every arm whose type is not our fallback's -- `numeric` among them,
    whose `map_numeric` answers a decimal wherever a typmod allows one."""
    return sorted(name for name, arrow in mapping.arms.items() if arrow != FALLBACK)


def extremes_problems(mapping: Mapping, fixtures: Path, versions: Sequence[str]) -> list[str]:
    """Both directions between the typed arms and each major's extremes."""
    typed = set(typed_arms(mapping))
    problems: list[str] = []
    for version in versions:
        dump = fixtures / version / "types" / "default.sql"
        columns = extremes_columns(dump)
        if columns is None:
            problems.append(
                f"{version}: {dump.relative_to(fixtures)} holds no {EXTREMES_TABLE} — "
                "regenerate the `types` fixture"
            )
            continue
        declared = set(columns.values())
        for arm in sorted(typed - declared):
            problems.append(
                f"{version}: the arm `{arm}` maps to an Arrow type and "
                f"{EXTREMES_TABLE} declares no column of it — add its least, "
                "greatest and special values to fixture_schema_types.sql"
            )
        for column, base in sorted(columns.items()):
            if base not in typed:
                problems.append(
                    f"{version}: {EXTREMES_TABLE}.{column} declares `{base}`, which "
                    "no typed arm answers — its extremes test nothing"
                )
    return problems


# --------------------------------------------------------------------------
# The reconciliation
# --------------------------------------------------------------------------


@dataclass
class Reconciliation:
    """Both directions, and everything that stopped either being answered."""

    #: Declared type -> its verdict, for every row of every major. A type whose
    #: verdict moves between majors is a problem rather than a merge.
    verdicts: dict[str, Verdict] = field(default_factory=dict)
    #: Declared type -> the disposition standing over it.
    dispositions: dict[str, Disposition] = field(default_factory=dict)
    mapping: Mapping = field(default_factory=Mapping)
    #: `builtin_scalar` arms no major's floor has a row for.
    unevidenced: list[str] = field(default_factory=list)
    #: Where the extremes table and the typed arms disagree, per major.
    extremes: list[str] = field(default_factory=list)
    problems: list[str] = field(default_factory=list)

    @property
    def unanswered(self) -> list[Verdict]:
        """Rows the rule reaches that neither meet the floor nor say why."""
        return [
            v
            for v in self.verdicts.values()
            if v.kind not in ("meets", "undefined") and v.declared not in self.dispositions
        ]


def _citation_problems(
    dispositions: Sequence[Disposition], register: Path, status: Path
) -> list[str]:
    """Whatever stops a disposition's citation resolving.

    A `waiting` row names the slice that closes it (the spec's D10) and the
    `below-by-decision` row names the register entry that carries it. Both are
    pointers, into `STATUS.md` and `deficiencies.md`, and both go stale
    silently: a re-slice renumbers the first, a strike deletes the second. So
    they are resolved rather than trusted, which is `deficiencies.py`'s own
    argument for the slice pairing, reused against the same files.
    """
    problems: list[str] = []
    for path in (register, status):
        if not path.is_file():
            problems.append(f"{path} does not exist")
    if problems:
        return problems
    entries, _ = deficiencies.parse_index(register.read_text())
    indexed = {entry.id for entry in entries}
    checklists = deficiencies.parse_checklists(status.read_text())
    listed = {item.id for items in checklists.values() for item in items}
    for disposition in dispositions:
        if disposition.stance not in STANCES:
            problems.append(
                f"{disposition.declared} declares stance "
                f"{disposition.stance!r}, which is not one of {sorted(STANCES)}"
            )
        if disposition.stance == "waiting" and disposition.closes is None:
            problems.append(
                f"{disposition.declared} is waiting on a slice and names none — "
                "a row this phase closes says which slice closes it"
            )
        if disposition.closes is not None and disposition.closes not in listed:
            problems.append(
                f"{disposition.declared} names slice {disposition.closes}, which "
                "no STATUS checklist lists — the phase was re-sliced and this "
                "row is aimed at a number that no longer exists"
            )
        if disposition.deficiency is not None and disposition.deficiency not in indexed:
            problems.append(
                f"{disposition.declared} cites {disposition.deficiency}, which "
                "the deficiency register does not index — the entry was struck or "
                "never written"
            )
    return problems


def reconcile(
    mapping_path: Path = MAPPING,
    fixtures: Path = FIXTURES,
    pyproject: Path = PYPROJECT,
    status: Path = STATUS,
    manual: Path = MANUAL,
    register: Path = REGISTER,
) -> Reconciliation:
    out = Reconciliation()
    out.mapping = parse_mapping(mapping_path)
    out.problems += out.mapping.problems
    out.dispositions = {d.declared: d for d in DISPOSITIONS}
    if len(out.dispositions) != len(DISPOSITIONS):
        out.problems.append("two dispositions name one declared type")
    out.problems += _citation_problems(DISPOSITIONS, register, status)

    pin, pin_problems = pinned_driver(pyproject)
    out.problems += pin_problems
    out.problems += published_pin_problems(pin, manual)

    seen_versions = majors(fixtures)
    if not seen_versions:
        out.problems.append(f"{fixtures} holds no major to read a floor from")
        return out

    drivers: set[str] = set()
    present: set[str] = set()
    for version in seen_versions:
        path = adbc_floor.floor_path(fixtures, version)
        if not path.is_file():
            out.problems.append(f"{version} has no adbc/floor.tsv — regenerate it")
            continue
        for row in adbc_floor.read_floor(fixtures, version):
            drivers.add(row["driver"] or "")
            declared = row["declared"] or ""
            present.add(declared)
            verdict = verdict_for(row, out.mapping)
            existing = out.verdicts.get(declared)
            if existing is None:
                out.verdicts[declared] = verdict
            elif existing != verdict:
                out.problems.append(
                    f"{declared}: the majors disagree — {existing.kind} "
                    f"({existing.theirs}) at one and {verdict.kind} "
                    f"({verdict.theirs}) at {version}"
                )

    # D8: the pin and the rows are one claim, so they are made unable to drift.
    if pin is not None:
        for driver in sorted(drivers):
            if driver != pin:
                out.problems.append(
                    f"floor.tsv rows were taken with driver {driver!r} and "
                    f"pyproject.toml pins {pin!r} — bumping the pin obliges "
                    "re-taking the oracle (the spec's D8)"
                )

    # The direction that decays: an arm mapping a type no major declares.
    out.unevidenced = sorted(name for name in out.mapping.arms if name not in present)

    # And the direction a typed mapping forces: its extremes.
    out.extremes = extremes_problems(out.mapping, fixtures, seen_versions)

    # The direction that makes a waiting row close itself.
    for declared, disposition in out.dispositions.items():
        verdict = out.verdicts.get(declared)
        if verdict is None:
            out.problems.append(
                f"{declared} carries a disposition and no major's floor has a "
                "row for it — the type it was written about is gone"
            )
        elif verdict.kind == "meets":
            out.problems.append(
                f"{declared} now meets the floor ({verdict.ours}) and still "
                f"carries a {disposition.stance} disposition — drop it"
            )
        elif verdict.kind == "undefined":
            out.problems.append(
                f"{declared} carries a disposition and the floor is undefined "
                f"for it ({verdict.why}) — the row is placed by its own columns"
            )
        elif disposition.stance in BELOW_STANCES and verdict.kind != "below":
            out.problems.append(
                f"{declared} is dispositioned {disposition.stance}, which says "
                f"we model no Arrow type for it, but we answer {verdict.ours!r}"
            )
        elif disposition.stance == "narrower" and verdict.kind != "differs":
            out.problems.append(
                f"{declared} is dispositioned narrower, which says both sides "
                f"are real Arrow types, but the verdict is {verdict.kind}"
            )
    return out


# --------------------------------------------------------------------------
# Reporting
# --------------------------------------------------------------------------


def report(found: Reconciliation, out=sys.stdout) -> None:
    counts: dict[str, int] = {}
    for verdict in found.verdicts.values():
        counts[verdict.kind] = counts.get(verdict.kind, 0) + 1
    shown = ", ".join(f"{counts[k]} {k}" for k in sorted(counts))
    print(
        f"{len(found.mapping.arms)} builtin_scalar arms, "
        f"{len(found.verdicts)} floor rows ({shown}).\n",
        file=out,
    )

    reached = [v for v in found.verdicts.values() if v.kind != "undefined"]
    print("Rows the floor rule reaches:", file=out)
    for verdict in sorted(reached, key=lambda v: v.declared):
        disposition = found.dispositions.get(verdict.declared)
        if verdict.kind == "meets":
            mark, note = "  ", "meets"
        elif disposition is not None:
            mark, note = "  ", disposition.stance
        else:
            mark, note = "!!", f"{verdict.kind}, and nothing says why"
        ours = verdict.ours if verdict.ours is not None else "(unreadable)"
        print(
            f" {mark} {verdict.declared:<28} theirs {str(verdict.theirs):<24} "
            f"ours {ours:<24} {note}",
            file=out,
        )
    print(file=out)

    if found.dispositions:
        print("Rows our mapping does not simply meet, and why:", file=out)
        for declared in sorted(found.dispositions):
            disposition = found.dispositions[declared]
            cites = disposition.closes or disposition.deficiency or ""
            head = f"  {declared} ({disposition.stance}"
            print(f"{head}{', ' + cites if cites else ''}): {disposition.reason}", file=out)
        print(file=out)

    if found.unevidenced:
        print(
            "Arms mapping a type no major's floor has a row for — "
            "a mapping decision with no evidence:",
            file=out,
        )
        for name in found.unevidenced:
            print(f"  {name}", file=out)
        print(file=out)

    if found.extremes:
        print(
            f"Typed arms and {EXTREMES_TABLE}'s columns that do not answer each other:",
            file=out,
        )
        for problem in found.extremes:
            print(f"  {problem}", file=out)
        print(file=out)

    unanswered = found.unanswered
    if unanswered:
        print(
            "Rows below the floor with no disposition — write one beside "
            '"The bar", or map the type:',
            file=out,
        )
        for verdict in unanswered:
            print(
                f"  {verdict.declared}: theirs {verdict.theirs}, "
                f"ours {verdict.ours or '(unreadable)'}",
                file=out,
            )
        print(file=out)

    if found.problems:
        print("Problems:", file=out)
        for problem in found.problems:
            print(f"  {problem}", file=out)
    elif not unanswered and not found.unevidenced and not found.extremes:
        print(
            f"Every floor row the rule reaches is answered (bar "
            f"{len(found.dispositions)} carrying a stance), every "
            "builtin_scalar arm has a floor row, and every typed arm its "
            "extremes.",
            file=out,
        )


def check(
    mapping_path: Path = MAPPING,
    fixtures: Path = FIXTURES,
    pyproject: Path = PYPROJECT,
    status: Path = STATUS,
    out=sys.stdout,
    manual: Path = MANUAL,
    register: Path = REGISTER,
) -> int:
    found = reconcile(mapping_path, fixtures, pyproject, status, manual, register)
    report(found, out=out)
    failing = found.problems or found.unanswered or found.unevidenced or found.extremes
    return 1 if failing else 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--fixtures", type=Path, default=FIXTURES, help="fixture root (tests use this)"
    )
    parser.add_argument(
        "--mapping", type=Path, default=MAPPING, help="the Rust file holding the arms"
    )
    parser.add_argument(
        "--pyproject", type=Path, default=PYPROJECT, help="where the driver pin lives"
    )
    parser.add_argument(
        "--status", type=Path, default=STATUS, help="the slice checklist"
    )
    parser.add_argument(
        "--register", type=Path, default=REGISTER, help="the deficiency register"
    )
    parser.add_argument(
        "--manual", type=Path, default=MANUAL, help="the page publishing the floor"
    )
    args = parser.parse_args(argv)
    return check(
        args.mapping,
        args.fixtures,
        args.pyproject,
        args.status,
        manual=args.manual,
        register=args.register,
    )


if __name__ == "__main__":
    raise SystemExit(main())
