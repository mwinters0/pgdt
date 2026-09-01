# P11.6 — the `character(n)` trim

What 11.6.1 and the rest of P11 inherit. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
register works now is [`architecture.md`](architecture.md), "Ordering operators
compare typed".

## What landed

`character` stopped being the register's one clause-blind collatable arm.

- **`CompareKind::PaddedText`** — bytewise over the field text with trailing
  `0x20` taken off *both* sides first. One arm of `order_key`, one line:
  `text.trim_end_matches(' ')`.
- **`collated_text` gained a `kind` parameter**, so `character` reaches the
  same three verdicts `text` does — an explicit `C`/`POSIX` **agrees**, another
  stated collation is `NonBytewiseCollation`, no clause at all is
  `UnknownCollation`. The function's name and its `type_default ==
  TypeCollation::Bytewise` line are load-bearing: `scripts/oracle_register.py`
  reads both as anchors and reads *which* built-in arms branch on the clause
  out of whether their body calls `collated_text(`. Adding the parameter is
  what made `character` start counting there, with no edit to the script.
- **`OrderingDivergence::BlankPadded` is gone**, and `AsText`'s sentence was
  rewritten: its one member is `json`, for which PostgreSQL defines no
  comparison at all, so the note now says the difference runs the *other* way.
- **`ComparisonPlan::text_diverging` is gone** too, folded into `diverging`,
  which is now `pub(crate)`. A text-only shortcut stopped being the common case
  the moment a second bytewise kind existed.

## The order is the whole of the correctness argument

Trim, **then** consult the collation — never the other way, and never pad
instead. `bpcharcmp` calls `bcTruelen` on both operands before `varstr_cmp`
sees a collation at all (I38), so the padding is not a value the collation is
ever asked to rank. Padding both sides to `n` instead is sound for `=` and
unsound for `<`: a byte below `0x20` sorts *under* the pad space, where the
server, having stripped the pad, ranks the longer string above. That is I38's
corollary, and it is the only reason the two orders ever differ — every other
value in a `char(n)` column is printable and the two agree.

## The evidence was already in the tree, and that is the point

11.11.1 labelled the oracle's `character(10)` cases with a collation and gave
them a tab-bearing value one slice before anything read either — deliberately,
so this change could be checked against evidence it did not produce. Landing
the trim retired **all eight** `character(10)` entries from
`the_register_answers_every_committed_oracle_cell`'s exception set at once:
four ordered pairs × two collations, each disagreeing in all 24 cells of its
case before and in none after, at six majors. The exception set is now 32
entries in two populations, and both are the same statement — a collation the
file does not carry — asked at two depths.

**Both collations closed, which was not guaranteed.** Under `default` our
answer is trim-then-bytewise and the server's is trim-then-`en_US.utf8`; they
coincide here because the case values (`a`, `a` + nine blanks, `a` + tab,
`hello`) hold no pair glibc and `memcmp` order differently. So `character(10)`
under `default` is now the same shape as `character varying(10)`: it announces
`UnknownCollation` and never disagrees. That is honest — the pairing is
one-directional, a disagreement must be announced and an announcement need not
disagree — but a stronger `char` case list would put a `default` disagreement
back in the file, and nobody would have to guess which half was doing the work.

## `KD7` was rewritten, not struck

The spec binds *"`KD7` is retired by this phase"* and the checklist line said
this slice strikes it. It did not, and the reasoning is filed as an open
question under `STATUS.md`'s "Decisions worth another look".

In short: of the four statements `KD7` carried, three close as **properties** —
a database collation no plain dump records (I32), reached through a bare
`text`/`varchar`/`char(n)` column and, one level down, through a `jsonb` string
leaf; and `json`, which the server does not order at all. Neither has a remedy
anybody could write. The fourth does: a column that *states* `COLLATE
"de_DE.utf8"` carries the fact in the file, and implementing that order is a
collation library nobody holds intent for — a defect with a known fix and no
owner, which is `(c) unowned` by the process's own definition, not a property.
So the entry survives at one statement and drops from `(b) owned by P11` to
`(c) unowned`; the source marker moved from `ComparisonPlan::AS_TEXT` to
`OrderingDivergence::NonBytewiseCollation`.

Reversibility decided it: an entry kept and later struck costs one edit, where
a number struck and later reinstated is a state the register cannot express.

**One repo-state assertion was relaxed to a skip.**
`test_deficiencies.ThisRepo.test_every_sliced_owner_is_paired_both_ways`
asserted that the tree holds at least one `(b)` entry owned by a sliced phase,
as an anti-vacuity guard. `KD7` was the only one, and dropping it to `(c)` made
the guard fail on a state that is legitimate: the population is empty between
one such entry being allocated and the next. The rule itself is asserted in
both directions by the fixture tests above it, which is why the guard could
become a `skipTest` rather than being deleted or satisfied by inventing an
owner.

## What 11.6.1 inherits

**The `char(n)` canonicalization is already here.** Equality's third category —
"narrow the field per row" — is `CompareKind::PaddedText`, so `=` on a
`char(n)` column is `Eq` over two trimmed slices and nothing new has to be
decided about padding.

**The spec's "two decode-per-row exceptions" is short by at least two**, and
both were found by reading the committed oracle rather than the source:

- **`jsonb`** re-renders numbers through `numeric_out` of the parsed value
  (`fixtures/16/oracle/literals.tsv`: `1e2` → `100`, `-0.0` → `0.0`), and
  `jsonb_eq` is `compareJsonbContainers == 0` — which compares numbers by value.
  So `{"a": 1.50}` and `{"a": 1.5}` are one value written two ways, and no
  canonical rendering of the literal can make them bytewise equal. Either
  `jsonb` decodes per row, or `Jsonb`'s number key has to stop normalizing and
  start rendering — and it cannot do both, since ordering needs the normalized
  form.
- **`real`/`double precision`**: the oracle answers `-0 = 0` **true**
  (`double precision -0 0 \N f t f t t f`), while `render_f64(decode_f64("-0"))`
  is `-0` by design. A dump can write `-0`, so canonicalize-once is wrong for
  any float column that holds one.

**Two worries that turned out not to be real**, checked in the same files:
`float8eq(NaN, NaN)` is **true** — PostgreSQL routes the float operators
through the same internal comparison the btree opclass uses, so `=` and `<=`
agree about `NaN` and no per-operator split is needed; and `timetz` output is a
function of `(instant, zone)`, so its canonical text is unique per value.

**The literal-side renderers are the bulk of the work.** Everything mapped has
a `render_*` in `decode.rs` already; the text-held kinds have none — `timetz`,
`inet`/`cidr` (whose `inet_out` omits `/32`, so `10.0.0.1/32` and `10.0.0.1`
are one value spelled two ways), `macaddr`/`macaddr8`, and `jsonb` if it is not
made an exception. Sizing 11.6.1 by that list rather than by "route `=` through
the plan" is the lesson 11.6's own mis-sizing teaches.

**One inconsistency ships in the meantime, and the manual says so.** `>=` and
`<=` now call a padded `char(n)` field equal to an unpadded literal while `=`
does not, which is PostgreSQL's answer on the two operators that changed and
the old answer on the one that has not. `docs/manual/type-handling.md` names
the two spellings that work today.

**The oracle test still skips `=`/`<>`.** Turning those two cells on is 11.6.1's
strongest check and its own piece of work: the walk currently feeds
`output(right)` as the bound, which is *already* the server's canonical form —
so it would exercise the comparison and not the canonicalization. Feeding the
raw right literal instead is what would test both, and it is a change to how
every cell is asked.
