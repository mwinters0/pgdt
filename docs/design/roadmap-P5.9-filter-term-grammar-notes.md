# P5.9 — the filter term is parsed for two audiences

What landed, and what the wrap and the next slice inherit. The spec is
[`roadmap-P5-pushdown.md`](roadmap-P5-pushdown.md), "A filter term is parsed
for two audiences"; the mechanism's own account is
[`architecture.md`](architecture.md), under the same heading.

## Where it lives

All of it is `pgdump_query-cli/src/main.rs`, and deliberately: `Predicate` is a
plain public struct an embedder fills in field by field, so nothing below L4
parses `column=value` and none of this reaches an embedder's values. Four
functions, one enum:

| | |
|---|---|
| `split_filter_op` | earliest operator position outside quotes, longest spelling; returns `FilterSplit` |
| `FilterSplit` | `Op` / `NoOperator` / `UnbalancedQuote(char)` — the third is what makes an unterminated quote a refusal rather than a term with no operator |
| `dequote` | the outer pair stripped, doubled interior quotes collapsed; `None` for an unquoted part |
| `filter_part` | one side of a term: `str::trim`, then `dequote` |
| `quoted_name_note` | the sentence `--column`/`--table` add to a name that was not found and looks quoted |

**No new module.** `measure.py`'s `QUERY_CLI` names `pgdump_query-cli/src/main.rs`
by path, and a figure's `depends` is matched by prefix — a second file in that
crate would sit outside the declaration and would be a staleness edge nobody
declared. Splitting the CLI into modules is fine, but it has to move `QUERY_CLI`
to the directory in the same change.

## The two calls that are not obvious

**`split_filter_op` walks bytes and compares bytes.** Matching an operator
through `str` — `spec[i..].starts_with(symbol)` — panics on the interior byte of
a multi-byte character, and that is reachable rather than theoretical: trimming
is Unicode's, so a non-breaking space is exactly the character a user pastes
into a term. The pre-`P5.9` parser iterated `char_indices` and so was safe by
accident; the byte loop is safe on purpose, and the reason is written at the
loop. `tests::trimming_is_unicode_whitespace` is what caught it.

**An unterminated quote is reported as a fault in the *column name*.** The scan
returns at the first operator it reaches outside a quote, so a quote that is
still open when the scan ends must have opened left of any operator — there is
no case where it is the value's. That is why `FilterSplit::UnbalancedQuote`
needs no position.

## What a reader will want to re-derive and should not

- **The `IS` forms are the fallback, and the order is load-bearing.** An
  operator outside quotes is looked for first. Restoring the more natural
  reading — strip the suffix, then look for an operator — reintroduces
  `--filter 'note=this is null'` parsing as an `IS NULL` on a column called
  `note=this`. There is a test named for it.
- **A bare quote character left of the operator now opens a quoted region**, so
  a column named `it's` must be written `"it's"`. That is the price of quotes
  carrying boundary information, and it is loud: the term is refused, naming
  the unbalanced quote.
- **Unterminated, trailing-text-after-close, and never-closed are one message.**
  Deliberate — falling back to the unquoted reading is the silent-wrong-answer
  shape the grammar exists to remove, and three messages would invite three
  behaviours.
- **`--column` and `--table` strip nothing.** `quoted_name_note` only fires on a
  name that was *not found* and that opens and closes with a matching quote. It
  is attached to `Error::UnknownProjectionColumn` through
  `name_taken_verbatim`, and printed after `no rows found` for `--table`. There
  is no equivalent for a predicate column, because a `--filter` term's quotes
  are already gone by then.

## Two earlier slices' notes are now wrong in one paragraph each

Corrected in place rather than left as traps, each pointing here:
`roadmap-P5.6-ordering-operators-notes.md` (the value kept its leading
whitespace) and `roadmap-P5.7-projection-figure-fold-in-notes.md` (the spaced
spelling was refused, and the question was open). The **spec** is untouched: it
already carries this grammar, written when the decision was made.

## For the wrap

- The phase's CLI grammar is one subject now, in `architecture.md`, "A filter
  term is parsed for two audiences". Nothing in the ordering section describes
  the split any more; it points there.
- The manual's account is `type-handling.md`, "Writing a filter term", and every
  shell line in it was executed against `fixtures/16/edge_cases/default.sql`
  rather than reasoned about — one of them had to be rewritten because `it's`
  cannot be written inside shell single quotes at all.
- No figure was re-taken, and `--stale` goes from ten to **eleven**: five
  figures declare `QUERY_CLI`, four of which were already stale on
  `scripts/generate_perf_data.py`, and `map-only` is the one this slice adds.
  It is not acknowledgeable — `--verify-additive` settles generator changes
  only, and a library or CLI change has no cheap oracle — so it stays red until
  the wrap sweep, alongside the ten that were already there. The parse is per
  *term* and happens before the file is opened; nothing on the row path
  changed.
