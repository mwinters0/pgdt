# P11.13 — the two flags may not disagree

What the rest of P11 inherits. The spec is
[`roadmap-P11-typed-predicates.md`](roadmap-P11-typed-predicates.md),
"`--where` is a new flag, and the two flags may not disagree"; how the
mechanism works now is [`architecture.md`](architecture.md), "`--where` builds
an expression out of those terms".

## What landed

`where_expr::refuse_where_structure(&str) -> Result<()>` — tokenize the term,
accept it only where the result is a single `Token::Leaf`. `main.rs` gains
`parse_filter_flag`, which runs that check and then `parse_filter`; the
`--filter` argument list is mapped through it instead of through `parse_filter`
directly. No library code, no new dependency edge beyond `main.rs` →
`where_expr`, which the spec already licensed (both L4).

`KD11` is **struck**: index line, detail paragraph and the module-doc marker in
`where_expr.rs`, in this change.

## The four calls the next slice should know about

**The check is on the `--filter` path alone, and `parse_filter` is untouched.**
That is not only the spec's economy argument — it is required. A `--where`
leaf is what came *out* of the tokenizer, and a leaf need not tokenize to
itself once cut out of its expression: `--where 'x=(and b)'` yields a leaf
`and b`, whose leading `and` had a paren before it in the full string and has
nothing before it standing alone. Putting the check inside `parse_filter` would
therefore refuse a `--where` string on a rule that has nothing to do with
`--where`. (That particular string is refused anyway, one token later, for the
stray `(`; the general shape is not safe.)

**Order: structure first, then the term grammar.** `--filter 'and is null'` is
told that `AND` is a reserved spelling rather than that its term did not parse,
which is what the spec asks for — that string named a column `and` before, and
the remedy in the message (`--filter '"and" is null'`) is the one that keeps
working.

**An empty term is left to the term grammar**, and it is the one string the
"single `Leaf`" rule does not literally cover. `tokenize("")` and
`tokenize("   ")` give *no* tokens, and no tokens is no structure: there is
nothing to name in a structural message, and `parse_filter`'s usage message is
the accurate one. The single-meaning property is untouched because neither flag
accepts an empty term. The exception the spec forbids is the other direction —
letting a *structural* term through because it returned no wrong rows — and
that one is not carved.

**The first non-`Leaf` token is what the message names**, which is well-defined
because leaves are only ever separated by structure: a token list longer than
one always holds a non-leaf. `Token::describe` is reused, so the refusal says
`` `AND` ``/`` `(` ``/`` `)` `` in the same vocabulary `--where`'s own faults do.

## What 11.9 and 11.10 inherit

- **The paren collision is now `--filter`'s too.** 11.8's notes left "a value
  holding a paren must be quoted" as a `--where` property and said `--filter`
  was unaffected; it is affected now, by construction. A composite literal is
  `--filter "v='(1,a)'"` and a range literal `--filter "span='[1,10)'"` — the
  range case being the awkward one, since `[1,10)` ends in a `)` that closes
  nothing and so is refused for a paren the user never thought of as one.
  **11.9 should re-read the rejected "recognise `(` as grouping only where an
  expression is expected" call with that in hand**: the case against it was
  context-sensitive tokenization bought for a case quoting already covers, and
  the population that now needs the quoting is larger than it was.
- **Nothing about the literal grammar itself moved.** `refuse_where_structure`
  never looks inside a leaf, so a literal parser landing under `parse_filter`
  changes neither flag's structural reading.
- The refusal set is the tokenizer's, so **a change to `keyword_at`,
  `is_word_byte` or the quote scan moves both flags at once** — which is the
  property the check was written for and the reason a second scan was rejected.

## Where the tests are

`where_expr.rs`'s `mod tests` covers the refusal itself:
`a_term_that_reads_as_structure_is_refused`,
`the_refusal_names_what_it_found`, `an_empty_term_is_left_to_the_term_grammar`,
and — the one that pins the narrowness —
`a_term_that_is_one_leaf_is_untouched`, which walks the boundary spellings
(`v_text=not a`, `tag=and`, `nota=1`, both `IS` forms, a quoted keyword, an
unclosed quote) and asserts every one still passes.

`main.rs`'s `mod tests` has `the_flag_refuses_structure_before_it_parses_a_term`
for the ordering.

`tests/query_where.rs` carries what only the binary can say. The load-bearing
one is now `a_string_that_reads_as_structure_is_refused_under_both_flags` —
the inverse of 11.8's `the_same_string_means_different_things_under_the_two_flags`,
which asserted the defect and is gone.
`a_value_that_holds_structure_is_asked_for_quoted` uses row 4 of
`16/edge_cases/default.sql`, whose `description` genuinely carries both a paren
and a bare `not`, so the remedy is exercised against real dump bytes rather
than a constructed value.
