# P31.16 — `json_in`'s grammar: notes

What the slices after this one inherit. The check it extends is 31.14's
([`roadmap-P31.14-strict-notes.md`](roadmap-P31.14-strict-notes.md)); why
`json` is the phase's is
[`../status/history/2026-10-03.md`](../status/history/2026-10-03.md), "A
clean `strict` parse promises what the field and its declaration decide".

## What exists

- **`decode::json_in`**, a whole port of `json_in` (I72, one `pg-refuses`
  marker): RFC 8259 with the server's four blanks, iterative over a stack of
  open containers. It answers read or refused, nothing between: every text
  is one of the two, so no `Unread::Unparsed` arises for `json`.
- **`predicate::field_refused` checks a `json` field by it**, at the top
  level and, in `checked_key`, beneath any container. `json` is recognised by
  its plan, `ComparisonPlan::AS_TEXT` and a nested position's
  `Uncomparable { divergence: Some(AsText) }`, whose one member is `json`
  (`pgtype.rs`, `AS_TEXT`); a domain over `json` resolves to the same plan.
  Only a strict parse checks it: a `default` parse and a query read `json` as
  text, as before.
- **`CACHE_FORMAT_VERSION` is 53**: a block's `checked_in_full` now covers its
  `json` fields, so a strict parse's mark from before means less than it
  says. No persisted byte moved and neither pinned digest did.
- **Evidence**: `decode.rs`'s `a_json_is_refused_only_where_json_in_refuses_it`
  (every case put to `pg_input_is_valid(…, 'json')` on the koji replica,
  PG16); `predicate.rs`'s `a_strict_check_finds_a_refusal_anywhere_in_a_field`
  (`json`, `json[]`, a composite holding one, an array of those) and
  `oracle::a_json_field_is_refused_as_the_server_refuses_it` (every `json` row
  of `literals.tsv`, every major); `tests/statistics.rs`'s
  `a_strict_parse_refuses_the_fields_a_default_one_leaves_to_a_query`, whose
  dump gained a `json` column.

## Negative results and limits

- **No depth is refused.** The server's parser recurses under
  `check_stack_depth()`, so its limit is the restoring server's
  `max_stack_depth` (I72's scope limit), a setting outside the promise; the
  manual names it. `jsonb`'s reader keeps its fixed `JSONB_MAX_DEPTH`, a
  shortfall.
- **`\u0000`, a lone surrogate half and a number past `numeric`'s range are
  read**, as `json_in` reads them; `jsonb_in` refuses each (I41, I63).
- **A `json` filter literal is not checked**: the column compares as text
  where the server has no comparison at all (`ComparisonDivergence::AsText`).
- **The comparison oracle was not extended or regenerated**: its one refused
  `json` input, `{a:1}`, is asserted at every major; the source of all six
  was read for the rest.

## What the slices after this inherit

- **31.22** owes `jsonb` a refusal for text `JsonCursor` cannot read
  (`KD90`). Every text `json_in` refuses `jsonb_in` refuses too, the
  latter's grammar being the former's plus its own refusals, so
  `decode::json_in` answering false is a marked refusal it can use.
