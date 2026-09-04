# P7.7.1 — one field split per row

The lever `7.7` sized: a row's field boundaries are found once and read by
everything that wants them, instead of by each predicate term and then again by
the batcher. How the mechanism works is beside the mechanism —
[`architecture.md`](architecture.md), "Predicates", and `copy::RowSplit`'s own
doc comment. This doc holds the two shape decisions the reading forced, the
measurement, and the one finding a later slice must not re-derive.

## Module map

| File | What it is |
|---|---|
| `copy.rs`, `RowSplit` | the split: `restart`, `field(row, i)` for random access, `complete(row)` for the rest. L1, beside `field_ranges`, which it does not replace |
| `predicate.rs`, `ResolvedTerm::eval` / `ResolvedExpr::eval` / `matches` | take `&mut RowSplit`; `split.field(bytes, i)` in place of `field_ranges(bytes).nth(i)` |
| `batch.rs`, `RowBatcher::push_row` | takes `&mut RowSplit` and reads `complete`'s ends. **One entry point, not two** — see below |
| `stream.rs`, the replay's `Event::Row` arm | owns the buffer for the whole replay, `restart`s it per row, hands it to the filter and then to the batcher |
| `runs/measure-7.7.1.sh` / `.tsv` | the before/after instrument: 7.7's six rejecting shapes plus five the lever also has to be judged on |

## The lever is lazy, which is why there is no term-count gate

7.7 left the choice as "unconditionally or behind a term-count test", sized
against an **eager** whole-row split — 16 boundaries whatever the filter reads,
which is why its arithmetic showed a single shallow term 31% worse.

`RowSplit` extends only as far as it is asked to, so that shape never arises. A
term reading field 3 finds four boundaries and stops; a row the filter rejects
is never walked past the deepest term. Each boundary is found by exactly one
`memchr` and every later ask is an index. The gate is unnecessary because the
losing case it would have gated is gone, and a gate would have been a heuristic
needing its own reading.

What remains, and it is not zero: the memoization itself. A boundary now costs
a `Vec` push and a read back where it used to cost only the `memchr`, so a walk
nothing shares is **~9 instructions a field** worse. That is the whole of the
cost side, and it is why the losing rows below are the ones with one shallow
term and the ones with no filter at all.

## Two entry points cost more than the bookkeeping they save

`push_row` was left walking the row directly for the unfiltered case, with
`push_row_split` beside it for the shared one. Both were correct and each had
the cheapest possible loop for its case. **It cost the unfiltered path 2.4% of a
query's user instructions** — 24.65 G → 25.29 G on the 3.00 GiB control, warm,
`--schema-mode strings`, full projection.

Nothing about that is the split. The second `push_field` call site in `batch.rs`
is enough on its own: with `push_row_split` present but never *executed*
(`black_box(false)` on the branch), the reading was still 25.29 G; with the call
site deleted, 24.696 G. A profile says what happened — before, `push_field` is
inlined into `push_row` and does not appear as a symbol; after, it is its own
symbol at 3.66% and `GenericByteViewArray::value` goes 2.55% → 4.60%, having
lost its inline into `render_field` in the same module. `#[inline]` on
`push_field` did not recover it, and neither did `#[inline(always)]`.

A closure shared between the two loops (`push_fields(…, impl FnMut(usize) ->
Option<Range>)`) was tried first and cost the same 2.6%, for the same reason:
two monomorphizations, two call sites.

So there is **one** `push_row`, and it always reads the split. The unfiltered
path pays the bookkeeping — 0.09 G on a full-projection query, 0.12 G on
`--no-columns` — which is a seventh of what the second entry point cost on the
first of those. **Do not reintroduce a direct-walk fast path without re-measuring the
unfiltered shape**; the intuition that it is free is exactly the one this slice
falsified.

## What it moves

`runs/measure-7.7.1.sh`, five shapes beyond 7.7's six, three reps each, retired
user instructions on the host, the same 3.00 GiB control on tmpfs, `pgdq query
--table public.perf --dqcache none --schema-mode strings`. A `runs/` artifact
and therefore gitignored, so the apparatus is written out here.

| Shape | What it asks | Before (G insn) | After | Δ |
|---|---|---|---|---|
| `reject-deep-5` | 5 terms, 13th column, OR'd, all false | 7.0084 | 4.2250 | **-39.72%** |
| `reject-deep-3` | 3 terms, same | 5.2415 | 3.9280 | **-25.06%** |
| `reject-deep-2` | 2 terms, same | 4.3580 | 3.7794 | **-13.28%** |
| `reject-deep-1` | 1 term, same | 3.4233 | 3.5780 | **+4.52%** |
| `reject-shallow-5` | 5 terms, 1st column, OR'd, all false | 3.5587 | 3.4814 | **-2.17%** |
| `reject-shallow-1` | 1 term, same | 2.7333 | 2.7651 | +1.16% |
| `keep-deep-5` | 5 terms, 13th column, AND'd, all true | 28.5686 | 25.2190 | **-11.72%** |
| `keep-shallow-1` | 1 term, 1st column, true on every row | 24.8252 | 24.9180 | +0.37% |
| `keep-shallow-1-nocols` | the same, `--no-columns` | 4.8388 | 4.9390 | **+2.07%** |
| `nofilter` | no `--where` at all | 24.6823 | 24.7724 | +0.37% |
| `nofilter-nocols` | the same, `--no-columns` | 4.2076 | 4.3274 | **+2.85%** |

Medians of three; per-rep spread is under 0.001% on every shape but the
four that emit 814,362 rows, where it is under 0.13%.

**The winning shapes are the ones the lever was proposed for**, and they scale
with the redundant walking removed: a five-term disjunction thirteen fields in
loses two fifths of the whole query, and the same five terms over rows that
*survive* — an all-true conjunction, which does not short-circuit — loses an
eighth of a query whose decode and render dominate it.

**`reject-shallow-5` is the row that shows the mechanism rather than the
prize.** Five terms one field in: before, five `memchr`s on the same first
boundary; after, one. −2.2% of a query that reads one byte per row.

**The losing shapes are all one term or none**, and they cost the memoization.
`reject-deep-1` is the largest at +4.5%, and it is the pessimal shape by
construction: thirteen boundaries memoized for one term, on a row that is then
thrown away, in a query that does nothing else. `nofilter-nocols` is +2.9% of a
query that walks and counts and nothing more; the same query with all sixteen
columns built is +0.4%, because the same 0.09–0.12 G is a much smaller share of
a query that also decodes, builds and renders every field.

**Nothing here is a `measurements.md` figure.** `predicate-terms` is the
registered table over this shape and 7.12's sweep re-takes it; these are
before/after readings of one change, on the deterministic instrument 7.6 and 7.7
both used, taken because the harness's wall figure cannot resolve a 2% move on a
machine this session shared.

## What the sharing does not reach

**The mapping pass, still.** The census's own field split runs in `map_forward`,
across the hard boundary [`architecture.md`](architecture.md), "Query: mapping
and streaming are separate passes" describes, and no arrangement of this lever
crosses it. 7.6 made that split cheap instead. This is unchanged from what 7.7
recorded and is restated because it is the obvious next thought.

**A row the filter rejects still walks to the deepest term.** Nothing skips
ahead: a term at field 13 costs thirteen boundaries whether or not the row
survives. The reading that would justify anything cleverer — a per-column offset
index, or evaluating the shallowest term first — does not exist, and reordering
terms changes which decode failures surface, which is a semantic change and not
this row's.

## Deliberately not done

- **No `unsafe`.** `RowSplit` writes through `Vec::push` and reads through
  indexing, both bounds-checked, on the same argument "A row's bytes are
  validated once, in bulk" makes: the bookkeeping is O(1) a field and a wrong
  index costs an answer, never the process.
- **No `Vec<u32>`.** It would halve the split's memory traffic and cost a
  width check on a row longer than 4 GiB. The 9 instructions a field are not
  mostly the store.
- **No re-take of `predicate-terms`, and no sweep.** Every figure that times a
  `pgdq` run was already stale; this slice moves the ones that pass a filter and
  the ones that do not, and 7.12 is what re-takes them.
- **No profile published.** One was taken to find the inlining regression above
  and is a diagnostic, not a proportion anything cites; it lives in
  `runs/p771-{before,after}.{data,txt}`.
