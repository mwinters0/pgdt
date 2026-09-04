# P7.7 — the predicated reading

What `7.7.1` inherits: **a registered figure that passes a filter**, and the
number that bounds the lever it was taken for. The figure itself is filed by
subject — [`measurements.md`](measurements.md), "What a filter term costs" —
and the mechanism's own cost property is beside the mechanism,
[`architecture.md`](architecture.md), "Predicates". This doc holds the
apparatus, the two instruments, and what the reading says about landing the
lever.

No library code: `pgdump_query/src/` and `pgdump_query-cli/src/` are
byte-unchanged. The one Rust file this slice touches is a test, and it is there
for the reason `query_projection.rs`'s sibling is — a command shape the CLI
refuses is otherwise discovered minutes into a sweep, with the figure lost.

## Module map

| File | What it is |
|---|---|
| `scripts/measure.py`, `PREDICATE_SHAPES` / `predicate_expr` | the six shapes, as (column, term count), and the `--where` expression each asks for |
| `scripts/measure.py`, `_script`'s `query-where-<shape>` arm | the in-container invocation: `--schema-mode strings`, full projection, `--dqcache none` |
| `scripts/measure.py`, `PREDICATE` | the mechanism tuple — `predicate.rs`, which no figure had declared |
| `scripts/measure.py`, `_PREDICATE_ROWS` / `run_predicate_terms` | the table, six rows, paired per-rep differences against the row above |
| `scripts/test_measure.py`, `PredicateShapes` | nine: the columns exist, the term counts match, the terms are distinct and disjoined, an unregistered shape is refused, every run is `strings` and carries its filter, the two depths differ only in the column, the deep one is deeper, the rows are the registered shapes, and the figure is taken on the control |
| `pgdump_query-cli/tests/query_where.rs`, `the_registered_predicate_shapes_are_executable` | every registered shape runs, and **none of them keeps a row** — the property the whole subtraction rests on, and the one nothing else would notice going wrong |
| `runs/measure-7.7.sh` / `.tsv` | the deterministic corroboration: the same six shapes in retired user instructions, on the host |

## The shape had to be built, not chosen, and three choices decide it

**All terms false, and OR'd.** `Or` evaluates its children until one is `True`,
so an all-false disjunction evaluates every term on every row — and no row
survives, so `push_row`, the decode, the Arrow build and `print_batch` are
identically absent from all six rows of the table. That is what makes the
difference between two rows the predicate and nothing else.

*Rejected: a conjunction of terms every row satisfies.* It would have given an
absolute a reader recognises, and it cannot be built on this file: every column
but `id` carries 2% NULLs, so an N-term conjunction is `Unknown` on
1 − 0.98^N of the rows and drops them, moving the very emit cost the
subtraction needs held constant. Confining the terms to `id` fixes that and
leaves one depth, which is the axis the figure exists for.

**Two depths, and the deep column is `v_bool` rather than the last one.**
`ResolvedTerm::eval` takes its operand with `field_ranges(..).nth(i)`, from the
front of the row, so depth is the walk. The two depths must therefore differ by
the walk and by nothing else — which rules out `v_escaped` (index 15), whose
decode takes the unescaping path and allocates, and `v_long_text` (14), whose
fields are long enough to move the decode's own scan. `v_bool` is index 12 and
its values are one byte.

**`--schema-mode strings`, for two reasons that agree.** The typed `=` decodes
the literal against the column's own type once per block, so `zzz1` on an
`integer` column is `Error::PredicateValueDecode` before the first row; and the
zero-copy path is the one this lever is read against, since 7.6 and 7.7.1 are
the phase's only levers on it.

## What it says

**The harness figure, wall clock, 512 MB container, warm on tmpfs**
([`measurements.md`](measurements.md), "What a filter term costs"): a term
thirteen fields in costs 0.09–0.12 µs a row, one field in 0.033 µs, and the
same five terms at the two depths differ by **0.27 µs a row** — 18% of what the
deep one costs on a query that decodes nothing.

**The deterministic instrument, retired user instructions, on the host**
(`runs/measure-7.7.sh` → `.tsv`, five reps, spread under 0.0012% on every
shape). It is a `runs/` artifact and therefore gitignored, so the shape is
written out here rather than left in it: the same 3.00 GiB control on tmpfs,
the same six `--where` expressions, `perf stat -e instructions:u` around
`pgdq query --table public.perf --dqcache none --schema-mode strings`, on the
host and outside the container.

| Shape | Median | |
|---|---|---|
| 1 term, 13th column | 3.4233 G | |
| 2 terms, 13th column | 4.3580 G | |
| 3 terms, 13th column | 5.2415 G | |
| 5 terms, 13th column | 7.0084 G | |
| 5 terms, 1st column | 3.5587 G | **−3.4497 G, which is 49.2% of the row above** |
| 1 term, 1st column | 2.7333 G | |

A deep term is 0.896 G, a shallow one 0.206 G, so **77% of a deep term is the
walk**. Dividing the 3.4497 G by the 5 × 12 extra field boundaries it buys puts
one boundary at **57.5 M instructions** over the file's 814,362 rows, or about
71 instructions a row per field crossed.

**Both instruments were needed and the reason is the machine.** A neighbouring
session built and tested throughout this sitting; the harness's gate held every
reading to its 15%-busy limit rather than excluding it, but the last two reps
drift upward across every row and the paired difference behind the 0.27 µs
ranges 0.08 to 0.39 s over the six. Instruction counts do not care, which is
what made them worth taking beside the figure — the same argument 7.6 made, and
the reason its own before/after was a `runs/` sitting.

## What this bounds for `7.7.1`, which is the point of the slice

Sharing one split replaces N walks of *i* fields with one walk of the whole row.
On this file that is 16 boundaries at 57.5 M each, **0.92 G**, against what the
predicate pays today:

| Predicate | Walk today | One shared split | |
|---|---|---|---|
| 5 terms, 13th column | 3.74 G | 0.92 G | **−2.82 G, 40% of the query** |
| 1 term, 13th column | 0.75 G | 0.92 G | +0.17 G, **5% worse** |
| 1 term, 1st column | 0.06 G | 0.92 G | +0.86 G, **31% worse** |

Two things follow, and the second is the one a session landing the lever must
not lose:

- **The prize is real and large on a many-term predicate**, and it is the
  largest single term in such a query.
- **The lever is not monotone.** On the shape most users actually run — one
  term, and often a shallow one — a shared full-row split costs more than the
  partial walks it replaces, *on a query that emits nothing*. It stops costing
  anything the moment rows survive, because `push_row` then splits the whole
  row anyway and the shared split is free; so the losing case is exactly a
  highly selective single-term filter. Whether `7.7.1` lands, and whether it
  lands unconditionally or behind a term-count test, is that trade and this
  table is what it is decided against.

The rows above are arithmetic over one measured slope, not six more readings —
57.5 M per boundary times a count. Stated so that nobody re-quotes them as
measurements.

## Deliberately not done

- **No library change.** The lever is `7.7.1` and it is a rework of the replay
  loop across `stream.rs`, `predicate.rs` and `batch.rs`; the two halves ask
  different review questions, and the evidence has to land first so the
  mechanism is checked against a figure it did not produce.
- **No sweep, and no re-take of anything.** Every figure that times a `pgdq`
  run was already stale before this slice and none of them moved: no library
  byte changed. 7.12's pair is what re-takes them, and it will take
  `predicate-terms` with them.
- **No profile.** The figure and the instruction counts answer the question
  this slice was set; a profile would attribute *within* a term, which is
  7.10's axis rather than this one's.
- **No second reading with rows surviving.** It is the obvious next table —
  what a filter costs when it keeps everything — and it is a different figure
  with a different subject, not a row that can be added to this one.
