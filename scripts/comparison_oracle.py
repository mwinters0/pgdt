#!/usr/bin/env python3
"""The comparison oracle: what PostgreSQL itself answers, per major.

`docs/design/architecture.md`, "The comparison oracle", is the description;
this module is the case table and the SQL that runs it. It has no container
plumbing of its own -- `generate_fixtures.py` owns that and calls in here,
because the oracle runs against the `types` fixture schema's own database and
so must be generated in the same container, from the same DDL, as
`fixtures/<version>/types/*.sql`.

Three files are written per major, under `fixtures/<version>/oracle/`, all in
PostgreSQL's own COPY TEXT encoding (tab-separated, backslash-escaped, `\\N`
for SQL NULL) because the server writes them with `COPY ... TO STDOUT` and
this repo already reads that encoding in L1:

- `meta.tsv`     -- `key`, `value`. The apparatus: server version, the
                    database's collation, and every session GUC that moves an
                    output spelling.
- `literals.tsv` -- `type`, `literal`, `status`, `output`. Whether the server
                    *accepted* the input, and if so the exact `*_out` text it
                    canonicalizes to.
- `comparisons.tsv` -- `type`, `left`, `right`, `collation`, then one cell per
                    operator in `OPERATORS` order. Each cell is `t`, `f`, `u`
                    (the comparison yielded SQL NULL) or `E<sqlstate>`.

**A pair is asked through two typed columns, not through a cast.** Each
comparison case builds a temp table declared `(a <typ>, b <typ>)` -- carrying
the case's collation on the columns, which is where `pg_dump` writes one (I37)
-- inserts the two literals, and asks `a <op> b`. A column's collation is its
type's own default wherever no clause is written, so a bare case measures what
a bare column in a dump does; a cast would derive the collation of its *input*
instead, which for a `text` parameter is the database's own. It also means a
literal the server refuses never reaches a comparison: all six cells are that
rejection, which is the answer `literals.tsv` records for it.

**`output` is the type's own output function, not a cast to `text`.** The two
differ for `character(n)`: `bpchar::text` strips the blank padding, while
`bpcharout` -- which is what a dump's `COPY` block writes -- keeps it. Since
the whole point of the `output` column is "what does this literal look like in
the file", it is produced by `textin(<typoutput>(...))`, looked up per type.

**The text answers are glibc's, and the collation is a dimension of the case
rather than an accident of the base image.** The fixture containers are the
Debian (`-trixie`) images, so the server is glibc and `datcollate`'s
`en_US.utf8` means glibc's collation. Every text pair is asked twice -- once
under `COLLATE "C"`, once under `COLLATE "default"`, which is the database's
own collation -- so one file holds both halves: pgdq compares bytewise, which
*is* PostgreSQL's answer under `C` and is not under `en_US.utf8`. Equality
comes along with it, and agrees under both, every libc collation being
deterministic.

**"Only glibc" is a limitation of the evidence, not a claim about servers.** A
musl deployment orders text differently and this oracle does not speak for it.
`meta.tsv` records the platform triple, `datcollate` and the default
collation's `collversion` -- the server's own notion of "this collation may
have changed underneath you" -- and `oracle_differences.py` guards all three,
so a major generated on a different base is a reported fault rather than five
hundred silent differences.
"""

from __future__ import annotations

from dataclasses import dataclass, field

# The order the six operators appear in every `comparisons.tsv` row, and the
# spelling the server is asked in. `<>` rather than `!=`: they are the same
# operator, and `<>` is the one the SQL standard writes.
OPERATORS = ["<", "<=", ">", ">=", "=", "<>"]

COMPARISON_COLUMNS = ["type", "left", "right", "collation", *OPERATORS]
LITERAL_COLUMNS = ["type", "literal", "status", "output"]
META_COLUMNS = ["key", "value"]


@dataclass(frozen=True)
class TypeCases:
    """One declared type's cases.

    `values` are compared against each other in **both** directions, all
    pairs including the self-pair -- so the order relation is pinned over the
    whole set rather than along a ladder, and a comparison that is not a total
    order has nowhere to hide. Keep the list short: the row count is `n**2`
    per type.

    `inputs` are the literals that exist to be *parsed* rather than ordered --
    malformed text, and the accepted-but-non-canonical spellings the
    `array_in`/`record_in`/`range_in` superset takes. They appear in
    `literals.tsv` like any other literal, and in `comparisons.tsv` only
    against `values[0]`, in both directions, which is enough for the
    cross-major differ to see a rejection become an acceptance.

    `None` is SQL NULL, and it is deliberately in most `values` lists: the
    three-valued answers (`u`) are the oracle for the evaluator that lands
    later in this phase.

    `collation` names a collation the case's two columns are declared with --
    `C` for bytewise, `default` for the database's own. It is a field on the
    *case*, so a text pair asked under two collations is two cases, rather
    than a pair of extra columns that would be empty for the twelve hundred
    rows where collation means nothing.

    **`None` means the comparison register does not branch on the clause for
    this type**, and nothing else. It used to mean two things at once -- for
    `text` and `character varying`, "asked bare", which is the database's
    collation and so the *divergent* population; for `name`, "asked bare",
    which is that type's own `C` default and so the *agreeing* one -- and a
    join that resolved it per type would be a second copy of the register's
    type-default rule living in Python. So every case of a type whose arm reads
    the clause states its collation explicitly, and `None` is left to the types
    where the register has no branch to name. `scripts/oracle_register.py` is
    the join that reads it.
    """

    type: str
    values: tuple[str | None, ...]
    inputs: tuple[str | None, ...] = field(default=())
    collation: str | None = None


#: The two collations a text case is asked under. `default` rather than the
#: locale's own name (`en_US.utf8`) because `pg_catalog."default"` is the
#: database's collation by definition and exists on every server, where a
#: locale-named collation object exists only if `initdb` imported one. What it
#: resolved to is `meta.tsv`'s `datcollate` and `default_collversion`, which
#: the differ guards.
COLLATIONS = ("C", "default")


# The declared types the comparison register will be keyed on, spelled the way
# `pg_dump` writes them in a `CREATE TABLE` -- which is what resolution reads,
# so a case here resolves to a register arm by string equality.
#
# Every user-defined type named below comes from `fixture_schema_types.sql`,
# which is what the oracle's database is loaded with. Nothing is version-gated:
# a type that does not exist on a major (multiranges before 14) answers
# `E42704` on that major and is accepted on the next, which is precisely the
# "additive" verdict the cross-major differ is built to recognise, and gating
# it here would hide the one transition the differ was written for.
TYPE_CASES: list[TypeCases] = [
    TypeCases("boolean", ("false", "true", None), ("t", "yes", "maybe")),
    TypeCases("smallint", ("-32768", "0", "32767", None), ("32768",)),
    TypeCases(
        "integer",
        ("-2147483648", "0", "2147483647", None),
        ("2147483648", " 42 ", "1e3"),
    ),
    TypeCases(
        "bigint",
        ("-9223372036854775808", "0", "9223372036854775807", None),
        ("9223372036854775808",),
    ),
    # PostgreSQL's one unsigned integer type. The values straddle 2^31, which
    # is where a signed reading of the same text goes wrong, and reach the
    # type's maximum. `-1` is an *input* rather than a value because the server
    # accepts it and wraps it to 4294967295 -- `oidin` has read a leading minus
    # since well before 13, and still does through v18's `uint32in_subr` -- and
    # this build refuses the literal instead, so the wrap belongs in the file as
    # the server's answer rather than as a value pgdq claims to order.
    TypeCases(
        "oid",
        ("0", "2147483647", "2147483648", "4294967295", None),
        ("-1", "4294967296", "abc"),
    ),
    # IEEE has all three specials, so `real`/`double precision` reach them
    # through the column's own decoder rather than as a carried position.
    # `-0` and `0` are equal and written differently, which is the float
    # analogue of the `numeric` scale case below.
    TypeCases(
        "real",
        ("-Infinity", "-0", "0", "1.5", "Infinity", "NaN", None),
        ("1e400", "inf"),
    ),
    TypeCases(
        "double precision",
        ("-Infinity", "-0", "0", "1.5", "Infinity", "NaN", None),
        ("1e400", "inf"),
    ),
    # A typmod'd numeric rounds the literal to the column's scale, so `1.5`
    # and `1.50` are one value written two ways -- the equality trap the
    # canonicalize-the-literal-once path exists for.
    TypeCases(
        "numeric(10,2)",
        ("-1.50", "0", "1.5", "1.50", "NaN", None),
        ("12345678901", "1.005"),
    ),
    TypeCases(
        "numeric(38,10)",
        ("-1.5", "0", "1234567890123456789012345678.1234567890", "NaN", None),
    ),
    # Bare `numeric` preserves scale, so `1.5` and `1.50` are equal and both
    # writable -- one of the two types the fast path must exempt. Its
    # infinities are v14+, and on 13 they are a rejection the differ reads as
    # additive.
    TypeCases(
        "numeric",
        ("-Infinity", "-1", "0", "1.5", "1.50", "Infinity", "NaN", None),
        ("abc",),
    ),
    TypeCases(
        "date",
        ("-infinity", "0001-01-01", "2024-01-01", "9999-12-31", "infinity", None),
        ("2024-13-01", "0044-01-01 BC", "10000-01-01"),
    ),
    TypeCases(
        "time without time zone",
        ("00:00:00", "12:34:56.789012", "24:00:00", None),
        ("25:00:00",),
    ),
    # `timetz` compares by the UTC instant, so `00:00:00-05` sorts above
    # `00:00:00+00` -- the pair that says so is here rather than inferred.
    TypeCases(
        "time with time zone",
        ("00:00:00+00", "00:00:00-05", "05:00:00+00", "24:00:00+00", None),
    ),
    TypeCases(
        "timestamp without time zone",
        (
            "-infinity",
            "0001-01-01 00:00:00",
            "2024-01-01 00:00:00",
            "2024-01-01 00:00:00.123456",
            "infinity",
            None,
        ),
        ("294277-01-01 00:00:00",),
    ),
    TypeCases(
        "timestamp with time zone",
        (
            "-infinity",
            "2024-01-01 00:00:00+00",
            "2024-01-01 05:30:00+05:30",
            "infinity",
            None,
        ),
    ),
    # `interval_cmp_value` collapses months to 30 days and days to 86400s, so
    # the middle three are one value written three ways -- the second type the
    # fast path must exempt. `infinity`/`-infinity` are v17+.
    TypeCases(
        "interval",
        (
            "-infinity",
            "-1 day",
            "00:00:00",
            "1 mon",
            "30 days",
            "720:00:00",
            "infinity",
            None,
        ),
        ("1.5 hours", "P1Y2M", "1 century"),
    ),
    TypeCases(
        "uuid",
        (
            "00000000-0000-0000-0000-000000000000",
            "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
            "A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11",
            None,
        ),
        ("not-a-uuid", "{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}"),
    ),
    TypeCases(
        "bytea",
        (r"\x", r"\x00", r"\xdeadbeef", r"\xff", None),
        (r"\xgg", "abc"),
    ),
    # The collation dimension. The same alphabet is asked twice -- bytewise
    # under `C`, and under the database's own collation, which on this
    # apparatus is glibc's `en_US.utf8` -- so the file holds both halves of
    # the register's text row: pgdq's bytewise order *is* PostgreSQL's answer
    # under `C`, and is not under a libc locale. All ordered pairs, as
    # everywhere else, because whether the two orders coincide is the property
    # under test and a ladder would presume it.
    #
    # Eight pairs earn the alphabet, four of which diverge on glibc 2.41 and
    # four of which agree. They diverge because case is a lower-weight
    # difference than letter (`A`/`a`, `a`/`B`), because an accent sorts with
    # its base letter rather than after `z` (`é`/`f`), and because punctuation
    # is ignored at the primary level (`_x`/`ax`). They agree on a space and a
    # hyphen inside a word (`de luge`/`deluge`, `co-op`/`coop`), on an accent
    # against its own base letter (`e`/`é`) and on a digit against a letter
    # (`1`/`a`). **The agreeing pairs are kept deliberately**: a set in which
    # every row diverged would read as "these two orders never coincide",
    # which is false.
    #
    # `text` has **no third, uncollated case**, and the empty string and
    # `hello` are here rather than in one. A bare case would have to be
    # labelled `default` to keep `None` meaning one thing (see
    # `TypeCases.collation`), and two `default` cases of one type share nine
    # ordered pairs -- duplicate rows in a file the differ aligns positionally.
    *(
        TypeCases(
            "text",
            (
                "", "1", "A", "a", "B", "e", "é", "f",
                "_x", "ax", "co-op", "coop", "de luge", "deluge", "hello", None,
            ),
            collation=collation,
        )
        for collation in COLLATIONS
    ),
    TypeCases(
        "character varying(10)",
        ("", "a", "hello", None),
        ("12345678901",),
        collation="default",
    ),
    # `bpchareq` compares after `bcTruelen` strips the padding, and the dump
    # writes the padded form: `a` and `a` + nine blanks are equal, and the
    # literal table's `output` column is what says the file holds the padded
    # one.
    #
    # **`a` + a tab is the ordering half of I38**, which the other three values
    # cannot reach: pad-and-compare and trim-and-compare disagree only against a
    # byte below `0x20`, and every other value here is printable. Trimmed, the
    # tab ranks above bare `a`; padded, it ranks below, because the tab sorts
    # under the pad space.
    #
    # Asked under both collations, and labelled one slice before the register
    # read the labels: while a `char(n)` was compared padded it diverged for a
    # reason no clause could fix, so the labels bought nothing, and they became
    # coverage the moment the trim made a `char(n)` compare under its own
    # collation. Labelling in a regeneration that was happening anyway cost
    # nothing; labelling later would have cost a six-major run of its own.
    *(
        TypeCases(
            "character(10)",
            ("a", "a         ", "a\t", "hello", None),
            collation=collation,
        )
        for collation in COLLATIONS
    ),
    # One of the two deliberate `None`s among the collatable types --
    # `public.text_c` below is the other -- and it is asked *bare*: `name`'s
    # own type default is `C`, so a bare `name` column is the whole of what
    # the register claims about it. Relabelling these cases
    # `collation="C"` would be true and would be the register's claim rather
    # than the oracle's observation -- a `name` pair asked under an explicit
    # `COLLATE "C"` is a different question from one asked bare, and only the
    # bare one shows what a bare column does.
    #
    # The cells answer it because the pair is two `name` *columns*, whose
    # `attcollation` is the type's own `C`. A cast of a `text` parameter would
    # not: a cast derives its collation from its input, so `$1::name` would
    # carry the parameter's `default` and record the database locale's order
    # under a case that names none.
    TypeCases("name", ("A", "a", "hello", None)),
    # The same rule reached from the other side, and the case that would have
    # caught the cast: a domain whose own DDL carries the clause, so a column
    # of it is bytewise while nothing in the case names a collation. It is the
    # only type in the oracle's schema whose collation comes from neither an
    # explicit clause nor `default`, and it joins `base_domain`,
    # `derived_domain` and `box_domain` on the register's `user/Domain` arm.
    TypeCases("public.text_c", ("A", "a", "hello", None)),
    # Declaration order, not label text: `sad` < `ok` < `happy` on the server
    # and the reverse bytewise, which is the whole of the enum register row.
    TypeCases(
        "public.mood",
        ("sad", "ok", "happy", "has space", None),
        ("nope", "SAD"),
    ),
    # `json` has no comparison operators at all -- no `=`, no order, no
    # default operator class -- so every cell is `E42883`. That is the row
    # this phase closes by statement.
    TypeCases("json", ("1", '{"a": 1}', "null", None), ("{a:1}",)),
    # `jsonb` is the widest arm in the register -- `compareJsonbContainers`
    # decides on the kind, then on a container's size, then on a scalar leaf
    # (I41) -- so its values are chosen one per branch rather than as a
    # sample. Five carry facts that nothing else in the file reaches:
    #
    # - **Both booleans**, because `false < true` is the boolean branch and
    #   `true` alone would only ever reach the kind order.
    # - **A string**, the one scalar kind the four original values missed, and
    #   the kind that sorts below every number.
    # - **`[]` and `{}`**, the empty containers. `[]` is the raw-scalar
    #   anomaly's other half: a top-level scalar is stored in a one-element
    #   pseudo-array and the element count overwrites the flag, so `1 > []`
    #   and `null > []` while `1 < [1, 2]`.
    # - **`{"zzz": 1}` against a two-pair object**, whose key sorts *after*
    #   both of the pair's. A one-pair object below a two-pair one is the
    #   count deciding before any key; had the short object's key sorted
    #   first, count and key would point the same way and the case would
    #   prove nothing.
    # - **`{"z": 1, "aa": 2}` and `{"y": 3, "zz": 4}`**, which separate storage
    #   order from alphabetical: stored, the walk is `z` against `y`, and
    #   sorted the other way it would be `aa` against `y`, which answers the
    #   other way. Storage order also lands in `literals.tsv` with no
    #   comparison at all -- `{"z": 1, "aa": 2}` is its own `output`, not
    #   alphabetized.
    #
    # `{"a": "a"}` against `{"a": "A"}` is the string *leaf*, and it is the
    # one pair here the register is known to answer differently:
    # `compareJsonbScalarValue` passes `DEFAULT_COLLATION_OID` to
    # `varstr_cmp`, so a leaf is ordered by the database's collation, which a
    # plain dump does not record (I32). pgdq compares it bytewise and
    # announces `OrderingDivergence::JsonbStringCollation`.
    #
    # The inputs are I41's input grammar: a number's exponent and a signed
    # zero are accepted and canonicalized away, a duplicate key resolves to
    # the *last* one written, a leading zero is refused, and `{` is
    # truncated.
    TypeCases(
        "jsonb",
        (
            "null",
            "false",
            "true",
            "1",
            '"a"',
            "[]",
            "[1, 2]",
            "{}",
            '{"a": 1}',
            '{"a":1}',
            '{"a": "a"}',
            '{"a": "A"}',
            '{"y": 3, "zz": 4}',
            '{"z": 1, "aa": 2}',
            '{"zzz": 1}',
            None,
        ),
        ("{", "01", "1e2", "-0.0", '{"a":1,"a":2}'),
    ),
    TypeCases("xml", ("<a/>", "<b/>", None), ("<a>",)),
    TypeCases(
        "inet",
        ("10.0.0.1", "192.168.1.0/24", "192.168.1.1", "::1", None),
        ("999.1.1.1",),
    ),
    TypeCases(
        "cidr",
        ("10.0.0.0/8", "192.168.1.0/24", "::/0", None),
        ("192.168.1.1/24",),
    ),
    TypeCases(
        "macaddr",
        ("08:00:2b:01:02:03", "08:00:2b:01:02:04", None),
        ("08-00-2b-01-02-04", "zz:00:2b:01:02:03"),
    ),
    TypeCases(
        "macaddr8",
        ("08:00:2b:01:02:03:04:05", "08:00:2b:01:02:03:04:06", None),
    ),
    TypeCases("public.base_domain", ("-1", "0", "1", None)),
    # The outer domain is NOT NULL, so SQL NULL is a *rejection* here and an
    # ordinary value one line up -- a domain constraint reached through a
    # cast, which nothing else in the tree exercises.
    TypeCases("public.derived_domain", ("-1", "0", "1", None)),
    # `box` orders by area and its `=` is area equality, which is neither
    # bytewise nor structural.
    TypeCases("public.box_domain", ("(1,1),(0,0)", "(3,3),(2,2)", None)),
    # A base type with `textin`/`textout` and no operator class at all: the
    # refusal case, distinct from `json`'s in that the type is user-defined.
    TypeCases("public.mybase", ("hello", "world", None)),
    # `array_eq` memcmps `dims` and `lbs` before it looks at an element, so
    # `[0:1]={1,2}` and `{1,2}` are unequal. `array_in` skips ASCII whitespace
    # around elements, matches an unquoted `NULL` case-insensitively, and
    # accepts the `[lb:ub]=` decoration even when every lower bound is 1 --
    # the superset the literal grammar has to implement.
    TypeCases(
        "integer[]",
        ("{}", "{1,2}", "{1,NULL}", "[0:1]={1,2}", "{{1,2},{3,4}}", None),
        ("{ 1 , 2 }", "{1,null}", "[1:2]={1,2}", "{1,2", "{1,,2}", "{{1,2},{3}}"),
    ),
    TypeCases(
        "text[]",
        ("{}", "{a,b}", "{a,B}", "{NULL}", '{"NULL"}', None),
        ("{ a , b }", '{"a","b"}', r"{a\,b}"),
    ),
    TypeCases("public.mood[]", ("{sad,ok}", "{ok,sad}", None)),
    # I26: an array whose element is itself an array, which `array_out` writes
    # one brace deep because it force-quotes any element containing `{`.
    TypeCases("public.intarr[]", ('{"{1,2}","{3}"}', '{"{}"}', None)),
    # `record_in` does **not** skip whitespace the way `array_in` does -- it
    # is preserved inside an unquoted field -- which is why the composite and
    # array supersets are two grammars rather than one.
    TypeCases(
        "public.point2d",
        ("(1,a)", "(1,b)", "(2,a)", "(,a)", "(1,)", None),
        ("()", "( 1 , a )", "(1,a", "(1,a,b)"),
    ),
    TypeCases("public.tagged", ('(a,"{x,y}")', "(a,)", None)),
    # `record_out` writes a zero-field composite `()`, which is also what a
    # one-field composite holding NULL writes.
    TypeCases("public.empty_comp", ("()", None), ("(1)",)),
    # A discrete range canonicalizes on input, so `[1,10]`, `(0,10]` and
    # `[1,11)` are one value: the comparison is over the canonical form, never
    # the text.
    TypeCases(
        "int4range",
        ("empty", "[1,10)", "[1,10]", "(0,10)", "[1,)", "(,5)", None),
        ("[1,10", "[10,1)"),
    ),
    TypeCases("numrange", ("empty", "[1,10)", "(1,10)", "[1,10]", None)),
    # `daterange_canonical` skips any bound that is `DATE_NOT_FINITE`, so the
    # infinite upper stays inclusive -- canonicalization is not total, and
    # this is the pair that says where it stops.
    TypeCases(
        "daterange",
        (
            "empty",
            "[2020-01-01,2020-01-02)",
            "[2020-01-01,2020-01-01]",
            "[2020-01-01,infinity]",
            None,
        ),
    ),
    TypeCases(
        "tsrange",
        ("empty", "[2020-01-01 00:00:00,2020-01-02 00:00:00)", None),
    ),
    TypeCases(
        "tstzrange",
        ("empty", "[2020-01-01 00:00:00+00,2020-01-02 00:00:00+00)", None),
    ),
    # A user-defined range over a continuous subtype: no canonicalization, so
    # `[1.5,10.5)` and `[1.5,10.5]` stay distinct.
    TypeCases("public.myrange", ("empty", "[1.5,10.5)", "[1.5,10.5]", None)),
    # The only range whose bounds need quoting: `range_bound_escape` doubles
    # `"` where an array would backslash it (I20).
    TypeCases(
        "public.textrange",
        ('["a,b","c""d")', '["","a")', '(,"z")', "empty", None),
        ('["a,b","c"d")',),
    ),
    # PG14+. On 13 every cell is `E42704`, which is the differ's additive
    # verdict rather than a gap in the table.
    TypeCases("int4multirange", ("{}", "{[1,10)}", "{[1,5),[6,10)}", None)),
    TypeCases("public.myrange_multi", ("{}", "{[1.5,10.5)}", None)),
    # An enum with no labels: only SQL NULL fits such a column, so it has
    # exactly one value and one rejected input.
    TypeCases("public.empty_enum", (None,), ("x",)),
]


def literal_cases() -> list[tuple[str, str | None]]:
    """Every `(type, literal)` the literal table asks about, in file order.

    **A literal is asked once per type**, wherever it is first named, whether
    that is `values` or `inputs` and whichever case carries it. Collation does
    not enter: an input function and an output function do not consult one, so
    a type asked under two collations would otherwise repeat every literal of
    the first case a second time. Deduplicating rather than skipping a collated
    case is what keeps a value that exists only there -- `character(10)`'s tab,
    `text`'s alphabet -- in the file.
    """
    out: list[tuple[str, str | None]] = []
    seen: set[tuple[str, str | None]] = set()
    for case in TYPE_CASES:
        for literal in (*case.values, *case.inputs):
            if (case.type, literal) in seen:
                continue
            seen.add((case.type, literal))
            out.append((case.type, literal))
    return out


def comparison_cases() -> list[tuple[str, str | None, str | None, str | None]]:
    """Every `(type, left, right, collation)` the comparison table asks about,
    in file order: all ordered pairs of `values`, then each `input` against
    `values[0]` in both directions.

    **The `input` pairs look redundant and are the cross-check.** A literal the
    server refuses cannot reach an operator -- building the pair is its own
    subtransaction -- so all six of those cells are a mechanical function of
    `literals.tsv`, which states the same rejection once. Around 6% of the
    committed rows are of that kind. They are kept because they are the only
    place two separately generated halves of the oracle are forced to agree,
    which is what would catch a `pgdq_cmp` whose subtransaction split had
    drifted; `test_comparison_oracle.py` asserts the agreement over the
    committed tree. Do not drop the pairing to shrink the file.
    """
    out: list[tuple[str, str | None, str | None, str | None]] = []
    for case in TYPE_CASES:
        for left in case.values:
            for right in case.values:
                out.append((case.type, left, right, case.collation))
        anchor = case.values[0]
        for extra in case.inputs:
            out.append((case.type, extra, anchor, case.collation))
            out.append((case.type, anchor, extra, case.collation))
    return out


def sql_literal(value: str | None) -> str:
    """A Python string as a SQL literal, under `standard_conforming_strings`
    -- which the session block below pins rather than assumes, because a
    backslash in a `bytea` or array case would otherwise depend on it."""
    if value is None:
        return "NULL"
    return "'" + value.replace("'", "''") + "'"


# Everything that moves an output spelling, pinned. The first three are what
# `pg_dump`'s own `_doSetFixedOutputState` sets, so `literals.tsv`'s `output`
# column is the form a dump writes; `TimeZone` and `client_encoding` are
# pinned because `pg_dump` leaves them to the server and this file is
# compared across containers.
SESSION_SQL = """\
SET standard_conforming_strings = on;
SET DateStyle = 'ISO, MDY';
SET IntervalStyle = 'postgres';
SET extra_float_digits = 3;
SET TimeZone = 'UTC';
SET client_encoding = 'UTF8';
SET bytea_output = 'hex';
SET array_nulls = on;
"""

#: The six operators as a SQL array literal, so the loop inside `pgdq_cmp` and
#: the cells `comparisons_script` projects out of it are one list in one order.
_OPERATOR_ARRAY = "ARRAY[" + ", ".join(sql_literal(op) for op in OPERATORS) + "]"

# Both helpers answer with a *string* rather than raising, because a rejected
# input is an answer this file records rather than a failure of the run: the
# cross-major differ reads "older rejects, newer accepts" as additive. The
# `EXCEPTION` blocks are what turn each case into its own subtransaction, so
# one bad literal cannot abort the surrounding `COPY`.
FUNCTIONS_SQL = f"""\
-- The pair is asked through two *columns* of the declared type, because a
-- column is what a dump holds: `attcollation` is the type's own default
-- wherever the DDL writes no clause, and a case that names a collation
-- declares it on the column, which is where `pg_dump` writes one (I37).
--
-- `coll` is SQL NULL for a case that names no collation, which is every case
-- but the text ones: the comparison is then whatever the column's declared
-- type gives it, which is what a bare column in a dump does.
--
-- Building the pair and comparing it are separate subtransactions, and the
-- split is what decides how far a rejection reaches. A type the server does
-- not have, or a literal it refuses, is a pair that cannot exist, so it
-- answers all six cells; a type with no `<` fails that cell alone.
CREATE FUNCTION pg_temp.pgdq_cmp(typ text, lhs text, rhs text, coll text)
RETURNS text[] LANGUAGE plpgsql AS $pgdq$
DECLARE
    c text := CASE WHEN coll IS NULL THEN '' ELSE ' COLLATE ' || quote_ident(coll) END;
    cells text[] := ARRAY[]::text[];
    setup text := NULL;
    op text;
    r boolean;
BEGIN
    BEGIN
        EXECUTE format('CREATE TEMP TABLE pgdq_pair (a %s%s, b %s%s)', typ, c, typ, c);
        EXECUTE format('INSERT INTO pg_temp.pgdq_pair VALUES ($1::%s, $2::%s)', typ, typ)
            USING lhs, rhs;
    EXCEPTION WHEN others THEN
        setup := 'E' || SQLSTATE;
    END;
    FOREACH op IN ARRAY {_OPERATOR_ARRAY} LOOP
        IF setup IS NOT NULL THEN
            cells := cells || setup;
            CONTINUE;
        END IF;
        BEGIN
            EXECUTE format('SELECT a %s b FROM pg_temp.pgdq_pair', op) INTO r;
            cells := cells || CASE WHEN r IS NULL THEN 'u' WHEN r THEN 't' ELSE 'f' END;
        EXCEPTION WHEN others THEN
            cells := cells || ('E' || SQLSTATE);
        END;
    END LOOP;
    -- Only where the setup succeeded: its own rollback took the table with it.
    IF setup IS NULL THEN
        EXECUTE 'DROP TABLE pg_temp.pgdq_pair';
    END IF;
    RETURN cells;
END
$pgdq$;

-- The type's own output function, reached through `textin(typoutput(...))`
-- rather than a cast to `text`: `bpchar::text` strips the blank padding that
-- `bpcharout` -- and therefore the dump -- keeps.
CREATE FUNCTION pg_temp.pgdq_lit(typ text, lit text, OUT status text, OUT out_text text)
LANGUAGE plpgsql AS $pgdq$
DECLARE outfn text;
BEGIN
    SELECT quote_ident(n.nspname) || '.' || quote_ident(p.proname) INTO outfn
      FROM pg_type t
      JOIN pg_proc p ON p.oid = t.typoutput
      JOIN pg_namespace n ON n.oid = p.pronamespace
     WHERE t.oid = typ::regtype;
    EXECUTE format('SELECT pg_catalog.textin(%s($1::%s))', outfn, typ)
        INTO out_text USING lit;
    status := 'ok';
EXCEPTION WHEN others THEN
    status := 'E' || SQLSTATE;
    out_text := NULL;
END
$pgdq$;
"""


def _values_list(rows: list[tuple[str | None, ...]], names: str, join: str = "") -> str:
    """A `VALUES` list with an explicit ordinal, cast on its first row so the
    columns are `text` even where the first value is NULL.

    `join` is whatever has to sit between the list and the `ORDER BY` -- the
    lateral call that asks one row's pair, for the comparisons script, and
    nothing for the literals one.
    """
    lines = []
    for ordinal, row in enumerate(rows):
        cells = [sql_literal(v) for v in row]
        if ordinal == 0:
            cells = [f"{c}::text" for c in cells]
            lines.append(f"    ({ordinal}::int, " + ", ".join(cells) + ")")
        else:
            lines.append(f"    ({ordinal}, " + ", ".join(cells) + ")")
    return (
        "  FROM (VALUES\n"
        + ",\n".join(lines)
        + f"\n  ) AS c({names})\n"
        + join
        + "  ORDER BY c.o"
    )


def meta_script() -> str:
    """`meta.tsv`: the apparatus, so a reader can see which server and which
    collation produced the answers beside it."""
    return (
        SESSION_SQL
        + """
COPY (
  SELECT * FROM (VALUES
    ('server_version', current_setting('server_version')),
    ('server_version_num', current_setting('server_version_num')),
    ('datcollate', (SELECT datcollate FROM pg_database
                     WHERE datname = current_database())),
    ('datctype', (SELECT datctype FROM pg_database
                   WHERE datname = current_database())),
    -- The default collation's version, which for a libc provider is the libc
    -- version verbatim: the server's own notion of "this collation may have
    -- changed underneath you". Read off the collation object `initdb`
    -- imported for the database's locale, because `pg_collation` records no
    -- version for `default` itself and `pg_collation_actual_version(100)`
    -- answers SQL NULL before v15.
    ('default_collversion', (SELECT c.collversion
                               FROM pg_collation c, pg_database d
                              WHERE d.datname = current_database()
                                AND c.collname = d.datcollate
                                AND c.collprovider = 'c'
                              LIMIT 1)),
    -- The platform triple, parsed out of `version()` so the version number
    -- itself may still differ between majors. This is the key that says which
    -- libc the text answers were taken under.
    ('platform', substring(version() from ' on ([^,]*)')),
    ('DateStyle', current_setting('DateStyle')),
    ('IntervalStyle', current_setting('IntervalStyle')),
    ('TimeZone', current_setting('TimeZone')),
    ('extra_float_digits', current_setting('extra_float_digits')),
    ('bytea_output', current_setting('bytea_output')),
    ('array_nulls', current_setting('array_nulls')),
    ('client_encoding', current_setting('client_encoding')),
    ('version', version())
  ) AS m(key, value)
) TO STDOUT;
"""
    )


def literals_script() -> str:
    """`literals.tsv`: acceptance and canonical output form, per literal."""
    rows = [(t, lit) for t, lit in literal_cases()]
    return (
        SESSION_SQL
        + FUNCTIONS_SQL
        + "\nCOPY (\n  SELECT c.ty, c.lit, (pg_temp.pgdq_lit(c.ty, c.lit)).*\n"
        + _values_list(rows, "o, ty, lit")
        + "\n) TO STDOUT;\n"
    )


def comparisons_script() -> str:
    """`comparisons.tsv`: one row per `(type, left, right, collation)`, one
    cell per operator.

    **One call per row, not one per cell.** The pair is a table the call
    builds, so asking each operator separately would build it six times -- and
    would lose the thing the split buys, that a pair which cannot exist
    answers all six cells with the same rejection. The six answers come back
    as an array and are projected back out positionally, in `OPERATORS` order.
    """
    rows = list(comparison_cases())
    cells = ",\n".join(f"         x.cells[{n}]" for n in range(1, len(OPERATORS) + 1))
    lateral = (
        "  CROSS JOIN LATERAL pg_temp.pgdq_cmp(c.ty, c.l, c.r, c.coll) AS x(cells)\n"
    )
    return (
        SESSION_SQL
        + FUNCTIONS_SQL
        + "\nCOPY (\n  SELECT c.ty, c.l, c.r, c.coll,\n"
        + cells
        + "\n"
        + _values_list(rows, "o, ty, l, r, coll", lateral)
        + "\n) TO STDOUT;\n"
    )


# The three files an oracle generation writes, in the order they are written.
# `generate_fixtures.py` walks this mapping; nothing else knows the names.
SCRIPTS = {
    "meta.tsv": meta_script,
    "literals.tsv": literals_script,
    "comparisons.tsv": comparisons_script,
}

# Directory name under `fixtures/<version>/`. It sits beside the schema
# directories rather than inside one because it is not a `pg_dump` output and
# has no flag set; `all_fixtures()` in the Rust test vocabulary walks for
# `*.sql` and so steps over it.
ORACLE_DIRNAME = "oracle"


_UNESCAPE = {
    "b": "\b",
    "f": "\f",
    "n": "\n",
    "r": "\r",
    "t": "\t",
    "v": "\v",
    "\\": "\\",
}


def unescape_copy_text(field_text: str) -> str | None:
    """One COPY TEXT field back to its value; `\\N` is SQL NULL."""
    if field_text == r"\N":
        return None
    out: list[str] = []
    i = 0
    while i < len(field_text):
        ch = field_text[i]
        if ch != "\\":
            out.append(ch)
            i += 1
            continue
        nxt = field_text[i + 1]
        out.append(_UNESCAPE.get(nxt, nxt))
        i += 2
    return "".join(out)


def parse_tsv(text: str) -> list[list[str | None]]:
    """A written oracle file back to rows of values."""
    return [
        [unescape_copy_text(f) for f in line.split("\t")]
        for line in text.split("\n")
        if line
    ]


#: The inverse of `_UNESCAPE`, holding only what `CopyAttributeOutText` escapes:
#: the backslash, the six named control characters, and the delimiter (which is
#: the tab, already in the set). Every other byte is written as itself --
#: notably, an ordinary control character is **not** escaped, so this is not
#: `repr`.
_ESCAPE = {
    "\\": "\\\\",
    "\b": "\\b",
    "\f": "\\f",
    "\n": "\\n",
    "\r": "\\r",
    "\t": "\\t",
    "\v": "\\v",
}


def escape_copy_text(value: str | None) -> str:
    """One value as a COPY TEXT field: the inverse of [`unescape_copy_text`].

    The server writes the oracle files; this writes the differences file
    derived from them, so the two must agree on the encoding or a `bytea`
    literal -- which is a backslash and hex digits -- comes back wrong.
    """
    if value is None:
        return r"\N"
    return "".join(_ESCAPE.get(ch, ch) for ch in value)


def format_tsv(rows: list[list[str | None]]) -> str:
    """Rows of values back to a COPY TEXT file, newline-terminated."""
    return "".join(
        "\t".join(escape_copy_text(v) for v in row) + "\n" for row in rows
    )
