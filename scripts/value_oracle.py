#!/usr/bin/env python3
"""The value oracle: the server's own second reading of every typed value in
the `types` fixture, and the reconciliation holding it to the typed arms.

docs/design/roadmap-P31-correctness-evidence.md, "The value oracle", is the
intent; docs/design/decisions.md, "D73", says which test holds which half of
"decode is right". `generate_fixtures.py` runs the pass in the database the
comparison oracle is asked in, and writes `fixtures/<major>/oracle/values.tsv`;
`pgdump_query/tests/value_oracle.rs` holds every typed read of every `types`
flag set that has DDL to it.

**A reading is the server's, never its output spelling.** Each scalar type a
`builtin_scalar` arm maps to Arrow is read through its binary send function or
through arithmetic the server does ([`READINGS`]): an integer's, an `oid`'s and
a `boolean`'s bytes in hex, the epoch in microseconds
for a time or timestamp, the Julian day for a `date`, the unscaled integer and
scale for a `numeric`, `float8send`'s hex for a float, the hex of a `bytea` or
a `uuid`, and months, days and microseconds for an `interval`. A special value
-- an infinity, `NaN` -- has no number to read, and is its own text.

**Every column of every table is walked by its catalog type**, not by what pgdt
maps it to, which the server cannot know: a domain to its base, an array
element by element through `unnest ... WITH ORDINALITY` (a multi-dimensional
one flattened, as `unnest` reads it), a composite field by field through
`(v).field`, a range through its bounds and its three flags, a multirange
range by range. A container is a row of its own -- its dimension lengths
(`2x2`, `0` when empty) or `()` -- so a NULL container and an empty one differ.
A leaf no reading names (`text`, an enum, `json`) contributes no row; the
round trip holds those.

**One row per node**: `table`, `column`, `row` (1-based, in `ctid` order, which
is the order `COPY` writes), `path` (`''` for the column's own value, then
`[k]` and `.field` steps), `type` (`format_type` of the node read) and
`reading` (`\\N` for SQL NULL).

**The reconciliation, D71's shape**: every `builtin_scalar` arm mapping to an
Arrow type other than text (`floor_mapping.typed_arms`) has a non-NULL reading
of its type at every major, so a mapping cannot land with nothing checking its
values.

Usage:

    cd scripts
    uv run value_oracle.py
    uv run python -m unittest test_value_oracle
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Sequence

import comparison_oracle
import floor_mapping

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "fixtures"

#: Beside the comparison oracle's files, which it is not one of: the
#: cross-major differ walks `comparison_oracle.SCRIPTS` alone.
VALUES_FILENAME = "values.tsv"

VALUE_COLUMNS = ["table", "column", "row", "path", "type", "reading"]

#: The schema whose tables are read, the only one `fixture_schema_types.sql`
#: declares tables in.
SCHEMA = "public"

#: The reading of each scalar type, by `pg_type.typname`, over the SQL
#: expression `{e}`.
READINGS = {
    "int2": "encode(int2send({e}), 'hex')",
    "int4": "encode(int4send({e}), 'hex')",
    "int8": "encode(int8send({e}), 'hex')",
    "oid": "encode(oidsend({e}), 'hex')",
    "bool": "encode(boolsend({e}), 'hex')",
    "float4": "encode(float4send({e}), 'hex')",
    "float8": "encode(float8send({e}), 'hex')",
    # `scale` is NULL for `NaN` and the infinities, whose reading is their text.
    "numeric": (
        "CASE WHEN scale({e}) IS NULL THEN ({e})::text"
        " ELSE trunc(({e}) * 10::numeric ^ scale({e}))::text || ' ' || scale({e}) END"
    ),
    "date": (
        "CASE WHEN isfinite({e}) THEN (({e}) - DATE '2000-01-01' + 2451545)::text"
        " ELSE ({e})::text END"
    ),
    # The send functions carry microseconds from 2000-01-01; 946684800000000
    # moves them to 1970's, in `numeric` because the sum can pass `bigint`.
    "timestamp": (
        "CASE WHEN isfinite({e}) THEN"
        " (('x' || encode(timestamp_send({e}), 'hex'))::bit(64)::bigint::numeric"
        " + 946684800000000)::text ELSE ({e})::text END"
    ),
    "timestamptz": (
        "CASE WHEN isfinite({e}) THEN"
        " (('x' || encode(timestamptz_send({e}), 'hex'))::bit(64)::bigint::numeric"
        " + 946684800000000)::text ELSE ({e})::text END"
    ),
    "time": "(('x' || encode(time_send({e}), 'hex'))::bit(64)::bigint)::text",
    # `interval_send` is the time part, then days, then months.
    "interval": (
        "CASE WHEN isfinite({e}) THEN"
        " (('x' || substr(encode(interval_send({e}), 'hex'), 25, 8))::bit(32)::int)::text"
        " || ' ' || (('x' || substr(encode(interval_send({e}), 'hex'), 17, 8))::bit(32)::int)::text"
        " || ' ' || (('x' || substr(encode(interval_send({e}), 'hex'), 1, 16))::bit(64)::bigint)::text"
        " ELSE ({e})::text END"
    ),
    "uuid": "encode(uuid_send({e}), 'hex')",
    "bytea": "encode({e}, 'hex')",
}

#: The one built-in read as a container without being an array type, through
#: the cast to the array type it is binary-coercible with.
INT2VECTOR = "int2vector"

#: Every type, every range and every attribute of the schema's tables and
#: composites: what the walk needs, in one round trip. `pg_range` is read whole
#: because its multirange column exists from 14 only.
CATALOG_SQL = f"""\
SELECT json_build_object(
  'types', (SELECT json_agg(json_build_object(
      'oid', t.oid::int8,
      'typname', t.typname,
      'typtype', t.typtype,
      'base', t.typbasetype::int8,
      'base_typmod', t.typtypmod,
      'base_name', CASE WHEN t.typbasetype <> 0 THEN format_type(t.typbasetype, t.typtypmod) END,
      'elem', t.typelem::int8,
      'is_array', EXISTS (SELECT 1 FROM pg_type e WHERE e.typarray = t.oid),
      'relid', t.typrelid::int8))
    FROM pg_type t),
  'ranges', (SELECT json_agg(to_jsonb(r)) FROM pg_range r),
  'attributes', (SELECT json_agg(json_build_object(
      'relid', a.attrelid::int8,
      'name', a.attname,
      'type', a.atttypid::int8,
      'typmod', a.atttypmod,
      'num', a.attnum,
      'generated', a.attgenerated <> '') ORDER BY a.attrelid, a.attnum)
    FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid
    WHERE a.attnum > 0 AND NOT a.attisdropped
      AND c.relnamespace = '{SCHEMA}'::regnamespace),
  'tables', (SELECT json_agg(json_build_object('relid', c.oid::int8, 'name', c.relname)
      ORDER BY c.relname)
    FROM pg_class c
    WHERE c.relnamespace = '{SCHEMA}'::regnamespace AND c.relkind = 'r')
);
"""

#: How deep a walk may go before a type is taken to be cyclic.
MAX_DEPTH = 16


@dataclass(frozen=True)
class PgType:
    oid: int
    typname: str
    typtype: str
    base: int
    base_typmod: int
    #: `format_type` of a domain's base, which the cast to it is written in.
    base_name: str | None
    elem: int
    is_array: bool
    relid: int


@dataclass(frozen=True)
class Attribute:
    name: str
    type: int
    typmod: int
    num: int
    generated: bool


@dataclass
class Catalog:
    types: dict[int, PgType]
    #: Range type -> its subtype.
    subtypes: dict[int, int]
    #: Multirange type -> its range type.
    multiranges: dict[int, int]
    #: Relation -> its attributes, in `attnum` order.
    attributes: dict[int, list[Attribute]]
    #: Table name -> relation, sorted by name.
    tables: dict[str, int]

    def type_named(self, typname: str) -> int:
        return next(t.oid for t in self.types.values() if t.typname == typname)


def parse_catalog(text: str) -> Catalog:
    """What `CATALOG_SQL` answered."""
    data = json.loads(text)
    types = {row["oid"]: PgType(**row) for row in data["types"]}
    subtypes: dict[int, int] = {}
    multiranges: dict[int, int] = {}
    for row in data["ranges"] or []:
        subtypes[int(row["rngtypid"])] = int(row["rngsubtype"])
        if row.get("rngmultitypid"):
            multiranges[int(row["rngmultitypid"])] = int(row["rngtypid"])
    attributes: dict[int, list[Attribute]] = {}
    for row in data["attributes"] or []:
        relid = row.pop("relid")
        attributes.setdefault(relid, []).append(Attribute(**row))
    tables = {row["name"]: row["relid"] for row in data["tables"] or []}
    return Catalog(types, subtypes, multiranges, attributes, tables)


def quote_ident(name: str) -> str:
    return '"' + name.replace('"', '""') + '"'


def sql_text(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def format_type(oid: int, typmod: int) -> str:
    return f"format_type({oid}, {'NULL' if typmod < 0 else typmod})"


@dataclass(frozen=True)
class Frame:
    """Where a node sits: the lateral `unnest`s reaching it, the guards keeping
    a NULL container's children out, its path as SQL, and the ordinals it
    sorts by."""

    froms: tuple[str, ...] = ()
    guards: tuple[str, ...] = ()
    path: str = "''"
    ords: tuple[str, ...] = ()

    def step(self, literal: str) -> Frame:
        return Frame(self.froms, self.guards, f"{self.path} || {sql_text(literal)}", self.ords)

    def guarded(self, guard: str) -> Frame:
        return Frame(self.froms, self.guards + (guard,), self.path, self.ords)


@dataclass(frozen=True)
class Node:
    frame: Frame
    type_sql: str
    reading_sql: str


@dataclass
class Walk:
    catalog: Catalog
    nodes: list[Node] = field(default_factory=list)
    aliases: int = 0

    def emit(self, frame: Frame, oid: int, typmod: int, reading: str) -> None:
        self.nodes.append(Node(frame, format_type(oid, typmod), reading))

    def unnest(self, frame: Frame, expr: str, element: int) -> tuple[Frame, str]:
        """A child frame per element of the array or multirange `expr`, and
        the element's expression in it.

        `unnest` reads a multi-dimensional array flat, which is the path the
        test walks; but a function returning a composite expands into its
        columns in `FROM`, so an array of one is read by subscript instead,
        one dimension deep."""
        self.aliases += 1
        alias = f"u{self.aliases}"
        if self.composite(element):
            lateral = f"CROSS JOIN LATERAL generate_subscripts({expr}, 1) AS {alias}(s)"
            ordinal = f"({alias}.s - array_lower({expr}, 1) + 1)"
            value = f"({expr})[{alias}.s]"
        else:
            lateral = f"CROSS JOIN LATERAL unnest({expr}) WITH ORDINALITY AS {alias}(v, i)"
            ordinal = f"{alias}.i"
            value = f"{alias}.v"
        child = Frame(
            frame.froms + (lateral,),
            frame.guards,
            f"{frame.path} || '[' || {ordinal} || ']'",
            frame.ords + (ordinal,),
        )
        return child, value

    def composite(self, oid: int) -> bool:
        t = self.catalog.types[oid]
        while t.typtype == "d":
            t = self.catalog.types[t.base]
        return t.typtype == "c"

    def walk(self, expr: str, oid: int, typmod: int, frame: Frame, depth: int = 0) -> None:
        if depth > MAX_DEPTH:
            raise ValueError(f"type {oid} nests past {MAX_DEPTH} levels")
        t = self.catalog.types[oid]
        if t.typtype == "d":
            cast = f"({expr})::{t.base_name}"
            self.walk(cast, t.base, t.base_typmod, frame, depth + 1)
        elif t.typname in READINGS:
            self.emit(frame, oid, typmod, READINGS[t.typname].format(e=expr))
        elif t.typname == INT2VECTOR or t.is_array:
            array = f"({expr})::int2[]" if t.typname == INT2VECTOR else expr
            lengths = (
                f"CASE WHEN num_nulls({array}) = 0 THEN coalesce((SELECT string_agg("
                f"array_length({array}, d)::text, 'x' ORDER BY d) FROM generate_series(1, "
                f"array_ndims({array})) AS d), '0') END"
            )
            self.emit(frame, oid, typmod, lengths)
            child, element = self.unnest(frame, array, t.elem)
            self.walk(element, t.elem, typmod, child, depth + 1)
        elif t.typtype == "c":
            self.emit(frame, oid, typmod, f"CASE WHEN num_nulls({expr}) = 0 THEN '()' END")
            inner = frame.guarded(f"num_nulls({expr}) = 0")
            for a in self.catalog.attributes.get(t.relid, []):
                self.walk(f"({expr}).{quote_ident(a.name)}", a.type, a.typmod,
                          inner.step(f".{a.name}"), depth + 1)
        elif t.typtype == "r":
            self.emit(frame, oid, typmod, f"CASE WHEN num_nulls({expr}) = 0 THEN '()' END")
            inner = frame.guarded(f"num_nulls({expr}) = 0")
            subtype = self.catalog.subtypes[oid]
            boolean = self.catalog.type_named("bool")
            self.walk(f"lower({expr})", subtype, -1, inner.step(".lower"), depth + 1)
            self.walk(f"upper({expr})", subtype, -1, inner.step(".upper"), depth + 1)
            self.walk(f"lower_inc({expr})", boolean, -1, inner.step(".lower_inclusive"), depth + 1)
            self.walk(f"upper_inc({expr})", boolean, -1, inner.step(".upper_inclusive"), depth + 1)
            self.walk(f"isempty({expr})", boolean, -1, inner.step(".empty"), depth + 1)
        elif t.typtype == "m":
            count = (
                f"CASE WHEN num_nulls({expr}) = 0 THEN "
                f"(SELECT count(*) FROM unnest({expr}))::text END"
            )
            self.emit(frame, oid, typmod, count)
            child, element = self.unnest(frame, expr, self.catalog.multiranges[oid])
            self.walk(element, self.catalog.multiranges[oid], -1, child, depth + 1)


def values_script(catalog: Catalog) -> str:
    """The `COPY ... TO STDOUT` producing `values.tsv`, one `SELECT` per node
    of every column of every table, in a stable order."""
    selects: list[str] = []
    for table, relid in catalog.tables.items():
        qualified = f"{SCHEMA}.{table}"
        source = (
            f"(SELECT row_number() OVER (ORDER BY ctid) AS pgdt_n, * "
            f"FROM {quote_ident(SCHEMA)}.{quote_ident(table)}) AS r"
        )
        for attribute in catalog.attributes.get(relid, []):
            if attribute.generated:
                # Not in the `COPY` column list, so no row of the dump holds it.
                continue
            walk = Walk(catalog)
            walk.walk(f"r.{quote_ident(attribute.name)}", attribute.type, attribute.typmod, Frame())
            for seq, node in enumerate(walk.nodes):
                frame = node.frame
                ords = ", ".join(frame.ords)
                where = " AND ".join(frame.guards) or "true"
                selects.append(
                    f"SELECT {sql_text(qualified)} AS tbl, {sql_text(attribute.name)} AS col, "
                    f"{attribute.num} AS attnum, r.pgdt_n AS n, ARRAY[{ords}]::int8[] AS ords, "
                    f"{seq} AS seq, {frame.path} AS path, {node.type_sql} AS type, "
                    f"{node.reading_sql} AS reading\n  FROM {source} "
                    + " ".join(frame.froms)
                    + f"\n  WHERE {where}"
                )
    body = "\nUNION ALL\n".join(selects)
    return (
        "COPY (\nSELECT tbl, col, n, path, type, reading FROM (\n"
        + body
        + "\n) AS q ORDER BY tbl, attnum, n, ords, seq\n) TO STDOUT;\n"
    )


# --------------------------------------------------------------------------
# The reconciliation
# --------------------------------------------------------------------------

_TYPMOD_RE = re.compile(r"\(\s*-?\d+(?:\s*,\s*-?\d+)?\s*\)")


def base_type(spelled: str) -> str:
    """`format_type`'s spelling with its typmod dropped, wherever it sits --
    `timestamp(3) without time zone` is `timestamp without time zone`."""
    return " ".join(_TYPMOD_RE.sub("", spelled).split())


def values_path(version: str, fixtures: Path = FIXTURES) -> Path:
    return fixtures / version / comparison_oracle.ORACLE_DIRNAME / VALUES_FILENAME


def read_values(path: Path) -> list[list[str | None]]:
    return comparison_oracle.parse_tsv(path.read_text())


def problems(arms: Sequence[str], fixtures: Path, versions: Sequence[str]) -> list[str]:
    """Every typed arm read at every major, and every file well formed."""
    found: list[str] = []
    for version in versions:
        path = values_path(version, fixtures)
        if not path.is_file():
            found.append(
                f"{version}: no {path.relative_to(fixtures)} -- run "
                f"`generate_fixtures.py --version {version} --skip-dumps --skip-oracle --skip-floor`"
            )
            continue
        read: set[str] = set()
        for number, row in enumerate(read_values(path), start=1):
            if len(row) != len(VALUE_COLUMNS):
                found.append(f"{version}: {VALUES_FILENAME} line {number} has {len(row)} fields")
                continue
            if row[4] is not None and row[5] is not None:
                read.add(base_type(row[4]))
        for arm in arms:
            if arm not in read:
                found.append(
                    f"{version}: the arm `{arm}` maps to an Arrow type and the value "
                    "oracle reads no value of it -- give a `types` table a column of it, "
                    "or give READINGS its reading"
                )
    return found


def check(fixtures: Path = FIXTURES, out=sys.stdout) -> int:
    mapping = floor_mapping.parse_mapping()
    found = list(mapping.problems)
    versions = floor_mapping.majors(fixtures)
    found += problems(floor_mapping.typed_arms(mapping), fixtures, versions)
    for problem in found:
        print(f"problem: {problem}", file=out)
    if not found:
        print(
            f"every typed arm has a value-oracle reading at {', '.join(versions)}",
            file=out,
        )
    return 1 if found else 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.parse_args(argv)
    return check()


if __name__ == "__main__":
    raise SystemExit(main())
