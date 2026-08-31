# P11.11.1 — The collated fixture columns

What the next slices inherit. How the fixtures, the oracle and the
reconciliation *work* is [`architecture.md`](architecture.md), "Fixtures",
"The comparison oracle" and "The register-to-oracle reconciliation"; this doc
is the part that is not description — the calls that are not obvious from the
diff, and what the two regenerations turned up.

## What landed

- **`public.t_collate`** in `scripts/fixture_schema_types.sql`, with
  `public.text_c` (`CREATE DOMAIN … AS text COLLATE "C"`) and
  `public.collated_pair` (`CREATE TYPE … AS (plain text, c text COLLATE "C")`)
  beside it. Six columns, nine rows, one alphabet replicated across every
  column.
- **All three of I37's emission sites in committed bytes**, at all six majors.
  Its **Observed** paragraph now quotes `fixtures/<13–18>/types/default.sql`.
- **`character(10)` asked under both collations**, its values gaining `"a"` +
  a tab — I38's *ordering* corollary, which the printable values could not
  reach.
- **`TypeCases.collation = None` means one thing**: the register does not
  branch on the clause for this type. `text` and `character varying(10)` say
  `"default"`; `name` stays the one deliberate `None` among the collatable
  types.
- **`oracle_register.py` reads the collation dimension** — three
  `collated_text` arms, two case groups, and a `datcollate` guard that makes
  the mapping between them sound rather than assumed.
- **`generate_fixtures.py` reports its own elapsed time**, with a
  past-30-minutes warning naming the figure it invalidates.
- Two tests in `pgdump_query/tests/ordering.rs`, one assertion per column.

## What the two regenerations proved

**A regeneration moves four things, not one, and the spec's assertion named
one.** The first run regenerated with `fixture_schema_types.sql` unchanged and
diffed against the committed tree: 109 files moved, and after excluding the
`\restrict`/`\unrestrict` tokens the residue was **not** empty. Every remaining
line was a wall-clock timestamp — `logs.events.logged_at` and its
`pgdq_tenant` counterpart, and `--verbose`'s `-- Started on` /
`-- Completed on` header pair, which is the mover nothing had written down.
Nothing else moved: no DDL, and no `--binary-upgrade` OID drift on that run.
So the assumption the slice split rests on holds, with a wider ignore list
than the spec wrote:

```sh
git diff -I'^\\(un)?restrict ' -- fixtures | grep -E '^[+-]' | grep -v 2026-08-31
```

That must print nothing. Note the regex: the spec's `^\\(un\\)?restrict `
matches `\restrict` and `\un\restrict`, not `\unrestrict`, so it under-matches
and leaves the token lines in the diff.

**A regeneration is 80 seconds**, twice, with the images already pulled — the
figure now in [`architecture.md`](architecture.md), "Fixtures", with the
script's own elapsed print as its staleness trigger. An oracle-only pass
(`--skip-dumps`) is 35 s. Nothing here needs handing off.

**Nothing broke across majors.** `fixtures/oracle-differences.tsv` is
byte-identical to what was committed — 509 differences, all additive — so the
new case groups introduced no cross-major movement at all, and the "a
non-additive difference is a stop" branch was never reached.

## Calls worth knowing about

**`text` has no third, uncollated case, and that is a duplicate-row problem
rather than a stylistic one.** Labelling the plain `text` case `"default"` —
which the `None` rule demands — left two `default` cases of one type sharing
`A`, `a` and NULL, which is nine identical `(type, left, right, collation)`
rows. `comparisons.tsv` carries no case identifiers and the differ aligns two
majors positionally, so a duplicate is exactly the failure
`test_no_duplicate_comparison_case` exists to catch. The plain case was folded
into the alphabet instead: `""` and `hello` joined the sixteen values, and
`text` is now structurally identical to `character(10)` — one case per
collation, the same values under both.

That is where the row count went: **1616 → 1745** comparisons, 279 → 291
literals. The alphabet at 16 values is 256 pairs per collation.

**`literal_cases()` dedupes globally instead of skipping a collated case.**
The old rule dropped every literal of a collated case, which was harmless while
`text`'s alphabet was the only collated thing in the table and lossy the moment
`character varying(10)` and `character(10)` were labelled — `12345678901`'s
length rejection and the `char` tab's padded `output` both live only there. The
new rule is "a literal is asked once per type, wherever it is first named",
which keeps them and still writes each row once.

**Which built-in arms branch on the clause is read from the source, not
listed.** `parse_register` chunks `builtin_scalar` arm by arm and marks an arm
collatable when its *body* calls `collated_text` — so `character` is not
collatable today and joins the set on the day 11.6 makes a trimmed `char(n)`
compare under its own collation, with no edit to the check. Its cases are
already labelled, so that day costs no regeneration.

**`default` covers two arms and `C` covers one**, because the oracle asks no
third collation. The soundness of that is read off the apparatus:
`database_collation_problems` fails if any major's `datcollate` is bytewise,
since a `C`-initdb'd apparatus would make the `default` group mean the opposite
and the join would go on passing.

**`pg_dump` writes `pg_catalog.ucs_basic` unquoted**, where `C` gets
`pg_catalog."C"`. That is `fmtQualifiedDumpable` quoting only where quoting is
needed, and it is now in committed bytes rather than inferred — it is also
precisely the shape `collation_is_bytewise` must not fold to the built-in, and
`v_text_ucs`'s assertion is what says it does not.

**The `t_collate` values are the divergent alphabet, not placeholders.** The
Rust assertions are about *notes*, which are independent of the data, so any
values would pass them. `every_collation_answers_the_same_bytewise_row_set` is
the one that uses the data: `_x` surviving `> B` is the divergence made
concrete, since underscore is above `B` in ASCII and ignored at glibc's primary
level.

## What later slices inherit

**11.6 opens with its `character` evidence already committed.** Both
collations, and the tab value that separates pad-and-compare from
trim-and-compare, are in `fixtures/<13–18>/oracle/comparisons.tsv` and swept
across all six majors — where I38's corollary previously rested on a single
probe on 16.15. When the arm starts consulting a clause, the reconciliation
picks the char cases up on its own.

**11.10 gets its collated composite.** `public.collated_pair`'s second
attribute carries `COLLATE pg_catalog."C"`, and `t_collate.v_pair` is a column
of it, so the collation boundary a nested comparison has to cross exists in a
real dump. `TypeKind::Composite`'s field list is already `Vec<ColumnDef>`.

**A new fixture table costs one insta snapshot review and no more**, which was
worth confirming: only `scan__edge_case_dump_event_stream.snap` exists and it
is on `edge_cases`. `--binary-upgrade`'s OIDs shift for the whole `types`
schema when a table is added, which is the fourth mover doing what
[`architecture.md`](architecture.md) says it does.

## What was left out, and why

**I37's placement consequence still rests on a one-off probe.** The clause is
appended after `DEFAULT`/`GENERATED` and after `NOT NULL` in a table column,
which is why `extract_collation` scans the whole fragment; no committed fixture
shows it, because no collated fixture column carries a default or a `NOT NULL`.
Adding one is a sixth collated column the spec's row does not name, and it
would cost the "one alphabet replicated across every column" property a `NOT
NULL` exception — so the probe is kept, labelled as a probe, and the residue is
under `STATUS.md`'s "Decisions worth another look".

**No figure turned stale.** Nothing this slice touched is a declared path of
any measurement figure; `uv run measure.py --stale` names the same seven as
before, for the same reasons.
