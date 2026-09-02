# P11.8 — `--where`

What the rest of P11 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md); how the
mechanism works now is [`architecture.md`](architecture.md), "`--where` builds
an expression out of those terms" and "A filter term is parsed for two
audiences".

## What landed

`pgdump_query-cli/src/where_expr.rs` — a tokenizer and a recursive-descent
parser, `parse_where(&str) -> Result<Expr>`. `main.rs` gains
`--where <EXPR>` on `query`, and the two flags combine into one conjunction.
Nothing in the library moved: 11.7 had already built every node the grammar
emits.

The term grammar gained the two `IS DISTINCT FROM` spellings, in
`split_filter_op`. That is not on 11.8's spec row and is the call recorded
under `STATUS.md`'s "Decisions worth another look" — the short version is that
the phase's operator surface commits to both forms and no other slice would
have spelled them.

## The five calls the next slice should know about

**The leaf is delegated, and the delegation is the whole design.** A leaf is
whatever text sits between parens and keywords, handed to `parse_filter`
untouched — quoting, trimming, the operator split and the `IS` fallback all
stay in one place. So `--where` needs no table of operators, no second quoting
rule, and no way to disagree with `--filter` about what `note=a` means. 11.9's
literal grammar lands under `parse_filter`, which means `--where` picks it up
with no edit here.

**A keyword needs whitespace or a paren beside it, which is stricter than a
word boundary and had to be.** `=` is not a word byte, so a bare word-boundary
rule reads `--where 'tag=and'` as the term `tag=` followed by `AND`. Requiring
structure on both sides leaves it one leaf. `v_and=1`, `nota=1` and
`name=android` all fall out of the same rule, and a byte at or above `0x80`
counts as a word byte so a keyword can never begin inside a multi-byte
character.

**A `NOT` whose preceding word is `is` belongs to the term.** `IS NOT NULL`
and `IS NOT DISTINCT FROM` each carry one, and this is the only place the
grammar could have silently meant something else: `v IS NOT NULL` would
otherwise tokenize as `NOT` applied to a leaf `NULL`. It is a one-word
lookback (`preceding_word`), not a general phrase matcher, because those are
the only two term-level constructs with a `NOT` in them and a lookback is
checkable at a glance.

**Parens are always structure, so a value holding one must be quoted.** This
is the collision 11.9 and 11.10 inherit: a composite literal is
`(1,a)` and a bare `(` groups, so `--where "v='(1,a)'"` is the spelling and
`--where 'v=(1,a)'` is a loud structural fault. Array literals (`{…}`) and
range literals (`[1,2)` — one bracket, one paren) are affected the same way,
the range case being the awkward one since `[1,2)` ends in a `)` that closes
nothing. The alternative — recognising `(` as grouping only where an
expression is expected — was rejected as context-sensitive tokenization
bought against a case the existing quoting rule already covers, but **11.9
should re-read that call** once its literal grammar exists, because it is the
slice that decides how a nested literal is typed on a command line.
`--filter` is unaffected either way: it has no parens to recognise.

**Juxtaposition is not an implicit `AND`.** Only a paren or a keyword ends a
leaf, so `a=1 b=2` is one leaf and the term grammar's earliest-operator rule
makes it `a` equal to `1 b=2` — the same thing that string means under
`--filter`. Two leaves can only end up adjacent across a paren, and that is
the one "not expected after a complete expression" fault.

## `IS DISTINCT FROM` in the term grammar

It is a **candidate at a position**, offered to `split_filter_op`'s existing
scan beside the six punctuation spellings, not a pass of its own. That is what
keeps the earliest-operator rule true of the whole set in both directions:
`note=a is distinct from b` stays the equality it reads as because `=` is
further left, and `a is distinct from b=c` is the distinctness test because
the phrase is. A separate pass in either order would have reinterpreted one of
those.

The phrase needs whitespace on both sides. That is the property that makes the
addition safe for terms that already parsed: a column named `is distinct from`
is still askable unquoted as `is distinct from=x`, because what follows the
phrase there is `=` rather than a space. The one term whose meaning changes is
one whose *column* carries the phrase between spaces, and it is loud — the
column it now names is the text to the phrase's left, which the library refuses
as unknown.

Words are separated by any run of ASCII whitespace and matched
case-insensitively; each word is bounded by requiring real whitespace after it,
which is also what rejects `is distinctly from` and a trailing `FROM` with no
value behind it.

## What 11.9 and 11.10 inherit

- **The literal grammar lands under `parse_filter`, and `--where` follows it
  for free.** Nothing in `where_expr.rs` looks at a value.
- **The paren collision above is 11.9's to re-read**, and it is the only place
  the two grammars interact.
- **`ResolvedTerm` will need a path for a nested comparison** (11.7's note);
  nothing in this slice touches that, and the CLI passes whatever
  `parse_filter` returns.
- The usage message and `PredicateOp::symbol` now agree on ten operators;
  `query_ordering.rs`'s `the_usage_message_names_every_operator` reads the
  message and is where a new spelling gets asserted.

## Where the tests are

`where_expr.rs`'s own `mod tests` covers the grammar — shape, precedence,
keyword boundaries, the `is not` lookback, and every structural fault — by
rendering the parsed tree back to a string, so shape and leaves are one
assertion. `main.rs`'s `mod tests` covers the two worded operators in the term
grammar, including both earliest-wins directions.

`pgdump_query-cli/tests/query_where.rs` is the part only the binary can say,
and the load-bearing test there is
`the_same_string_means_different_things_under_the_two_flags`: `name=alpha and
beta` is refused under `--where` and is an equality under `--filter`. That is
the whole reason there are two flags, and it cannot be seen from either alone.
`negation_drops_the_null_row_and_is_distinct_from_keeps_it` is the other one —
it is the row-level evidence that `NOT` and `IS DISTINCT FROM` are not the
same operator.
