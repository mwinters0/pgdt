# P6.2.2 — Every text-emitted kind compares as text: notes

An earned follow-up to 6.2 in
[`roadmap-P6-datafusion.md`](roadmap-P6-datafusion.md), admitted by the review
in [`../status/history/2026-09-22.md`](../status/history/2026-09-22.md), "A
`macaddr` literal in Arrow semantics". `CompareKind::arrow_order` now maps
`MacAddr` to `Text`, and `arrow_divergence` gives it `ValueAsText`. The why is
[`decisions.md`](decisions.md), "D40".

## What 6.9 and 6.7 inherit

- **Every kind emitted `Utf8View` or `Dictionary<Utf8>` is `Text` under
  `arrow_order`**, asserted of every declared type the oracle holds, on six
  majors, by `arrow_semantics_answers_as_datafusion_does` (`predicate.rs`,
  `oracle` module). So the provider's `compared_as_text` is true of every
  text-emitted column, and `literal_text` names no kind apart.
- **A `macaddr` string literal is pushed `Exact`, in either case of hex.**
  `a_pushed_filter_keeps_the_rows_datafusion_keeps` already tried each text
  value's uppercase spelling, and now pushes it and checks the rows against
  DataFusion's. `sql_pushes_down_what_the_library_answers_and_answers_alike`
  pins both spellings and a `macaddr8` ordering term.
- **`macaddr` has no Arrow-mode bounds now**: `bounds_ordered_in(Arrow)` is
  false, the kind having moved. The dictionary still answers `=` there. 6.9's
  bytewise gathering is what would give it bounds back, with a bare
  `numeric`, `timetz`, `inet`/`cidr` and `jsonb`.
- **`ARROW_AGREEMENT` still records `MacAddr` as agreeing.** That table is
  6.1's evidence over the server's own spellings, which stays true. Where
  `arrow_order` moves a kind, the walk no longer requires the table to agree.

## Negative results

- **No cache format change.** Gathering is unchanged; only which stored
  bounds Arrow semantics reads moved.
- **`pgdt` is unchanged.** It compares in PostgreSQL's semantics, where
  `macaddr` keeps its octet order and reads either case (D55).
