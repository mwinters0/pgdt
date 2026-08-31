# P11.11 — The declared collation is read

What the next slices inherit. How the register and the preamble grammar *work*
now is [`architecture.md`](architecture.md), "Ordering operators compare typed"
and "The preamble grammar and `DumpMetadata`"; this doc is the part that is not
description — the calls that are not obvious from the code, and what reading
the clause turned up.

## What landed

- **`ColumnDef`** in `preamble.rs` — `{ name, declared_type, collation }`,
  replacing the `(String, String)` pair in `DatabaseMetadata::tables`,
  `StatementShape::Table`, `SpanBody::Table` **and** `TypeKind::Composite`'s
  field list. `ColumnDef::new` is the no-clause constructor.
- **`TypeKind::Domain` gains `collation`**, with `TypeKind::domain(base)` as the
  no-clause constructor. A domain's clause is a *type default*, so it is kept
  where `NOT NULL` is discarded.
- **`extract_collation`** — a top-level, quote-aware scan of a column or domain
  fragment for the `COLLATE` keyword, returning the reference **verbatim**
  (`pg_catalog."C"`).
- **`comparison_for(declared, collation, types)`** — a third argument.
  `builtin_scalar` takes it too, and its three collatable arms read it:
  `text`/`character varying` (type default = the database's),
  `name` (type default = `C`), and `character` (never consults it).
- **Three new `OrderingDivergence` variants** — `UnknownCollation`,
  `NonBytewiseCollation`, `BlankPadded` — each with its own sentence in
  `OrderingNote::message`. `ComparisonPlan` and `OrderingDivergence` are still
  `Copy`.
- **`FORMAT_VERSION` 12 → 13**, because `SpanBody::Table` and `TypeKind` both
  changed shape on disk.
- **I37** (the `COLLATE` clause's emission condition, placement and spelling at
  every site) and **I38** (`character(n)` is blank-padded and `bpcharcmp`
  trims) in [`postgres-invariants.md`](postgres-invariants.md).
- The manual's collation section rewritten: it no longer says the warning is
  printed for every text column, because it no longer is.

## What actually changed, for a user

Nothing about *how* two values compare. Bytewise is still the comparison for
every text column; what moved is the verdict, and only for columns the file has
something specific to say about:

| Column | Before | Now |
|---|---|---|
| `text` with no clause | diverges | diverges — with a sentence that names the database default rather than "collation generally" |
| `text COLLATE "C"` / `"POSIX"` | diverges | **agrees**, silently |
| `name` with no clause | diverges | **agrees**, silently |
| `text COLLATE "en_US.utf8"` | diverges | diverges, naming the clause |
| `char(n)`, any collation | diverges | diverges — for blank padding, not for collation |

## Calls worth knowing about

**`char(n)` is not promoted, and finding that out is the slice's one plan
change.** The spec had `char` with `text` and `varchar`, all three closing under
an explicit `COLLATE "C"`. They do not: `bpcharcmp` calls `bcTruelen` on both
sides *before* `varstr_cmp` sees a collation (I38), and a dump writes every
`char(n)` value padded to `n`. So a field whose significant text equals the
filter's literal compares greater bytewise and equal on the server — a
divergence a collation clause cannot close. The spec row is amended and the
evidence is in [`../status/history/2026-08-31.md`](../status/history/2026-08-31.md).
**11.6 inherits it**: trimming both sides is the same canonicalization question
that slice already opens for `=`, and closing it there closes both operators at
once.

**The clause is stored verbatim, and L2 parses it.** `pg_catalog."C"`, not `C`.
That is the L1 rule (store what the dump said) applied to a second string, and
it matters here for a concrete reason: `pg_catalog."en_US.utf8"` has a dot
*inside* the quoted name, so any pre-parsed `schema.name` join would be
ambiguous. `collation_parts` in `pgtype.rs` splits it with the same `Cursor`
identifier grammar the rest of the parser uses.

**`collation_is_bytewise` checks the schema, and rejects an unquoted `C`.**
Only `pg_catalog."C"`, `pg_catalog."POSIX"` and their unqualified spellings
answer yes. An unquoted `COLLATE C` names the collation `c`, which is what the
server would resolve it to, and `public."C"` is somebody's own collation. Both
answer *diverges*. The asymmetry is deliberate: a wrong "diverges" costs a
warning, a wrong "agrees" costs a silently wrong row set.

**The clause is not next to the type.** `pg_dump` appends it after
`DEFAULT`/`GENERATED` and after `NOT NULL` in a table column (I37, verified in
all six majors), so `extract_collation` scans the whole fragment rather than
looking at the token after the type words. `COLLATE` stays in `STOP_WORDS`
regardless — the *type* still ends there when the clause happens to follow it
directly, which is the shape a domain and a composite attribute always take.

**The scan is depth- and quote-aware for shapes that really occur.** A
`DEFAULT 'collate …'` literal, a `CHECK ((v COLLATE "C") > 'a')` and a
`GENERATED ALWAYS AS (upper(a COLLATE "C")) STORED` are all stepped over. Only
the first is common, but all three are cheap to exclude and each would have
attached a collation to the wrong column.

**A domain's clause is the column's default, and the column overrides it.**
`comparison_user_type`'s `Domain` arm passes `collation.or(domain_collation)`
down the recursion. That is exactly the rule `pg_dump` writes a column-level
clause to express: it emits one only where the column's collation differs from
its type's, and for a domain-typed column "its type" is the domain.

**A composite's field list became `Vec<ColumnDef>` too**, though nothing reads a
field's collation yet — a nested column is refused a step earlier, on the
`NestedPlan`. It went uniform rather than staying a pair because `pg_dump` emits
a per-attribute `COLLATE` in `dumpCompositeType` as well (I37), so the fact is
in the file either way, and **11.10** — nested structural comparison, where a
`text[]` or a composite with a `text` field inherits the collation boundary — is
the slice that will want it. Storing it now costs one cache-format bump instead
of two.

## What later slices inherit

**11.6 owns the `char(n)` trim.** See above. Its "canonicalize the literal
once" fast path is where blank-padding a `char(n)` literal already belongs; the
same canonicalization settles ordering, and `OrderingDivergence::BlankPadded`
is the row it would close.

**11.10 gets a collation per composite field for free**, and `OrderingNote`'s
path — which the spec says that slice adds — is where a `text[]` column's
element-level divergence will hang. The element type of an array has no clause
of its own in the DDL; the *column* may carry one, and it applies to the
elements, which is the join 11.10 will need to make.

**The register's key is now `(declared type, collation)`, not the declared type
alone.** `scripts/oracle_register.py` still joins on the declared type and
still passes — splitting one `builtin_scalar` arm into three added no declared
base name — but an oracle case cannot yet *distinguish* the collated arms,
because `TypeCases` carries a collation for the comparison it asks and not for
the column it asks it of. That is not a hole the reconciliation can see, and
11.11.1 is where a real collated column arrives.

**`ComparisonPlan` is still `Copy`.** 11.4's enum labels are still the thing
that breaks it, exactly as 11.3's notes said; nothing here made that closer or
further away.

## What was left out, and why

**No fixture carries a `COLLATE` clause or a `name` column**, so the two cases
where this slice's answer changes *to agreement* have unit tests rather than a
dump behind them. That is `11.11.1`, earned on entry and written into the spec's
slice table with its reasoning. The divergent halves — `UnknownCollation` on
`v_text`/`v_varchar` and `BlankPadded` on `v_char` — *are* exercised end to end,
in `tests/ordering.rs`, on the existing `public.t_text`.

**The divergence does not name the collation it found.** `OrderingDivergence`
is `Copy` and a collation name is not, so `NonBytewiseCollation`'s sentence says
"a collation other than C/POSIX" rather than quoting it. Carrying the name means
either giving up `Copy` — which 11.4 will do anyway, for the enum labels — or a
fifth positional vector in `ResolvedSchema`. Neither is worth doing for a
sentence, and 11.4 makes the first one free.

**`eca96be` is acknowledged for seven figures, and three are deliberately left
out.** The commit's touches to declared paths carry no work — `map.rs` is a
type change with no new call, `batch.rs` is one line inside `#[cfg(test)]`,
`cache.rs` is a `FORMAT_VERSION` bump — and its one executable addition on a
scan path is `extract_collation`, once per column of DDL. That excuses every
figure whose input comes from `generate_perf_data.py`, which writes exactly one
`CREATE TABLE` per file.

It does not excuse the three whose input is `blocks4000`. `map-only` and
`per-block-quadratic` run the same addition 16000 times, over 4000 tables of
four columns; `preamble-prepass` *is* the measurement of the prepass the work
was added to. Unlike `M28`'s entry, reachability argues nothing here —
`blocks4000` carries no type DDL, but every one of its tables has columns. All
three stay red until a sweep, which is the state the rule asks for: red with a
reason.
