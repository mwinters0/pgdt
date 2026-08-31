# P11.2.2 — The fixture family moves to glibc

What the next slices inherit. How the oracle and the differ *work* is
[`architecture.md`](architecture.md), "The comparison oracle" and "The
cross-major differ"; this doc is the part that is not description — the calls
that are not obvious from the code, and what the regeneration turned up.

## What landed

- `generate_fixtures.py`'s `ROUTINE_VERSIONS` on `postgres:<minor>-trixie` for
  all six majors, with the paragraph saying the suffix is what pins the libc.
- `comparison_oracle.py`: a fourth field, `collation`, on `TypeCases`; two new
  text cases over one 14-value alphabet, asked under `C` and under `default`;
  `meta.tsv` gains `platform` and `default_collversion`; `pgdq_cmp` takes the
  collation and quotes it with `quote_ident`.
- `oracle_differences.py`: `platform` and `default_collversion` in
  `APPARATUS_KEYS`; `collation` as a field of `Difference` and a column of the
  differences file.
- Every fixture regenerated on the Debian images, and
  `fixtures/oracle-differences.tsv` re-filed.
- **I36** in [`postgres-invariants.md`](postgres-invariants.md) — the version
  header's string is the build's own and a packager may extend it.
- The manual's first sentence about collation, in
  [`type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise,
  and your server's may not be".

No library code, per the slice's contract — see "What was left out" below.

## The numbers

`comparisons.tsv` goes 1224 → **1616** rows per major, the 392 being the two
collated text cases (14 values, all ordered pairs, twice). `literals.tsv` stays
at 279: a collated case asks no literals, since an input function and an output
function do not consult a collation, and `literals.tsv` carries no collation
column to tell two copies apart.

`fixtures/oracle-differences.tsv` still holds **509 differences, all
additive**, unchanged by the family switch and by the new cases — no text
comparison moved between two majors, under either collation. I35's evidence is
therefore the same evidence, taken on a different libc.

## What the regeneration turned up

**The dumps were *not* byte-identical, and the fourth mover is the version
header.** The spec predicted byte-identity outside the three known movers
(`\restrict` tokens, `logged_at`, `--binary-upgrade` OIDs). A Debian build
configures `--with-extra-version`, so `PG_VERSION` and the server's
`server_version` GUC both carry a package suffix, and every fixture's two
header lines now read `16.15 (Debian 16.15-1.pgdg13+2)` where the Alpine build
wrote `16.15`. That is **I36**, and nothing broke on it: `version_header_field`
already keeps the rest of the line whole, the strings are reported and never
compared, and the one insta snapshot in the tree does not cover them. The koji
sample keeps the bare form, so the tree now holds both shapes rather than only
one — which is the better state, since a real-world dump can be either.

**The collated cells came out exactly as the spec's eight pairs said they
would**, on all six majors: `A`/`a`, `a`/`B`, `é`/`f` and `_x`/`ax` answer `<`
differently under `C` and `default`; `de luge`/`deluge`, `e`/`é`, `1`/`a` and
`co-op`/`coop` answer the same under both. `=` and `<>` agree under both
collations for every pair, which is the equality half the spec expected for
free.

**No performance figure moved.** `uv run measure.py --stale` reports exactly
what it reported before the slice — `session-drift` alone, for its own
pre-existing reason. No figure declares `fixtures/` or `generate_fixtures.py`.

## Calls worth knowing about

**`COLLATE "default"` rather than `COLLATE "en_US.utf8"`.**
`pg_catalog."default"` *is* the database's collation by definition and exists
on every server; a locale-named collation object exists only if `initdb`
imported one, which is a second thing to be true. What it resolved to is
`meta.tsv`'s `datcollate` and `default_collversion`, and the differ guards
both, so nothing is lost by not spelling the locale in the case table.

**`default_collversion` is read off the imported collation object, not the
default collation.** `pg_collation` records no `collversion` for `default`
itself, and `pg_collation_actual_version(100)` answers SQL NULL before v15 —
verified on `13.23-trixie`. The query joins `pg_database.datcollate` to
`pg_collation.collname` with `collprovider = 'c'` instead, which reads `2.41`
on all six.

**The platform triple is parsed server-side, in the meta script, not in the
differ.** `substring(version() from ' on ([^,]*)')` — so it is a key in the
file like every other apparatus fact, visible to a reader of `meta.tsv` rather
than derived at check time, and the version number may still differ between
majors without the guard firing.

**The collated case's `values` list is 14 long, against the 3–8 the other
cases keep.** All ordered pairs is the rule everywhere in this table, and the
reason applies here more than anywhere else: whether the bytewise order and the
collated order coincide is the property under test, so a ladder would presume
what is being asked. The four *agreeing* pairs are in the alphabet
deliberately — a set in which every row diverged would read as "these two
orders never coincide", which is false.

**The plain uncollated `text` case is kept.** It is what a bare `text` column
in a dump actually does, and on this apparatus its cells are now the glibc
answers rather than the bytewise ones — which is the visible half of the family
switch. The collated cases sit beside it rather than replacing it.

**`test_comparison_oracle.py` gained `test_every_oracle_was_taken_on_the_same_libc`**,
which pins `platform` to `x86_64-pc-linux-gnu` in every committed `meta.tsv`.
The differ's apparatus guard only says the six majors *agree*; this says which
libc they agree on, which is the claim I35's scope limit makes.

## What was left out, and why

**No `deficiency:`-style code marker for I36.** The natural place was a doc
comment on `map.rs`'s `version_header_field`, and it was written and then
removed: `pgdump_query/src/map.rs` is a declared path of eight performance
figures, so a comment-only edit turns eight figures red, and an acknowledgement
entry needs a commit sha this slice does not have. The slice's row also says
"no library code". I36's *Relied on by* names the function instead, and
[`architecture.md`](architecture.md)'s "Fixtures" section carries the fact
where a session regenerating the tree will meet it.

**11.11 still owns reading the `COLLATE` clause.** The manual's new section
says plainly that the warning is printed for every text column a filter orders,
including one declared `COLLATE "C"` and including a `name` column, both of
which pgdq in fact answers exactly. That is a property with a named
destination, not a deficiency: the answers are already right, and what 11.11
changes is which columns are told they diverge.

## What later slices inherit

**`comparison_cases()` returns 4-tuples now.** `(type, left, right,
collation)`, and `alignment_problems` compares four key columns rather than
three. 11.2.1's reconciliation joins on `case.type` alone, which is unchanged —
a collated case names the same `text` arm as the plain one, so the "every
oracle case resolves to a register arm" direction is unaffected. The other
direction (every arm reaches a case) is likewise unchanged: the collated cases
add no type.

**A `Difference` now carries a collation, and the file has eleven columns.**
Anything reading `fixtures/oracle-differences.tsv` positionally must count from
the header, not from memory; `read_committed` requires the header and checks
the width.

**The apparatus guard is now load-bearing rather than decorative.** Before this
slice a major reverted to a different base image would have passed it in
silence. `platform` and `default_collversion` are what close that, and
`apparatus_problems` is still the place a future session adds a key when
`SESSION_SQL` pins one more.
