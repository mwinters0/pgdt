# Out-of-band work

The ledger of work that belongs to no phase. It sits beside
[`roadmap.md`](roadmap.md)'s phase index rather than inside it because it grows
monotonically between keystones: a register read only as far as somebody's
window reaches is one that reissues a number already spoken for, and the rows
here are a work queue as well as a record. The rule it implements is
[`../process.md`](../process.md), "Out-of-band work".

An item is the kind of change that fits one session and answers to no phase's
intent: a CLI ergonomics change, a defect fix that changes no decision. It gets
a number `M<k>` and **one terse ledger line** — date, what changed, whether it
blocks the open phase, and the history entry that says why — in the table
below the watermark, started again by the first item admitted after a
keystone. Nothing else: no spec (there was no intent doc to write), and no notes doc,
because the history entry *is* the notes. If out-of-band work turns up a fact
an unspecified phase needs, that fact goes in that phase's inbox, as always.

**A number is allocated on admission, not on landing.** An item queued for
later takes its `M<k>` and its row when it is admitted, with the Date column
empty until it lands — so a queued item can be cited by number, and so this
table stays the authority on which numbers are spent. Row order is allocation
order, which is why a queued row may sit above one that landed before it.

**The admitting session sets the Blocks column, because only it knows.** An
item admitted while a phase is open is either in the way of that phase's
remaining slices or it is not, and the session that just finished grilling the
decision can say which; a session picking the row up weeks later cannot, and
guesses. `Blocks` names the open phase when that phase's remaining slices
should not be landed around it, and is empty otherwise. It is read by the
unattended loop — [`.claude/skills/go/SKILL.md`](../../.claude/skills/go/SKILL.md)
takes a blocking row ahead of the next unticked slice — and cleared when the
item lands, at the same time the Date is filled in.

**Admission rule.** An item is out-of-band only if it changes no decision any
spec records **and** fits one session. Anything that changes a decision goes
back through grilling → spec amendment → a numbered slice; that rule is what
keeps this ledger from becoming where design work goes to avoid review.

**Ledger lines stay one line each.** The file is read as an index, never as an
account — the detail lives in the dated history entry it points at. It grows
until a keystone, which strikes it along with the phase docs and leaves a
watermark saying which numbers are spent (`../process.md`, "The out-of-band
ledger is struck too").

**M1–M75 are struck**, and nothing at or below `M75` is reused. That is a
high-water mark rather than a claim that every one of them landed: some were
absorbed into a neighbour or folded into a phase slice, one is admitted and
still queued, and their numbers are spent all the same. What each struck item did is filed by subject —
[`architecture.md`](architecture.md) for a mechanism,
[`measurements.md`](measurements.md) for an apparatus change,
[`layering.md`](layering.md), [`../process.md`](../process.md) and
[`.claude/skills/`](../../.claude/skills/) for a rule — and why it was done is
in the dated history entry it was filed under.

## The ledger

**The table below carries what is still outstanding**, and is written on by
the next item admitted. A landed row is provenance and went with the rest of
the centering; a row whose Date is still empty is a live obligation and stays,
since a number is allocated on admission and the unattended loop reads this
table as a work queue.

| Item | Date | What changed | Blocks | Why |
|---|---|---|---|---|
| `M74` | | The sweep publishes `rss-attribution`: the `Shared` edge onto `peak-rss` is declared, the section's `outside-register` marker and `measure.NOT_OURS` row are deleted, the figure marker goes on and the stamp's accounting sentence is re-generated. It cannot be done before then — the doc's two tables carry two disagreeing readings of one command shape, which is what the borrow exists to collapse, and declaring the edge early only fails `--check` without letting the marker on | | [2026-09-08](../status/history/2026-09-08.md), "`M65` lands its instrument; `M74` owns the sitting" |
| `M76` | 2026-09-09 | The controlled arena reading: `control_xz` at `--jobs 4 --parallel-memory 268435456` — the koji probe's own arrangement — capped and uncapped, on the current binary and on a pre-`19.3` build from a temporary worktree. The manual's 200 MiB was **not** idle arenas the `current_thread` runtime removed: 6 threads against 29 read alike, and the cap reclaims ~50–60 MiB on both. A `runs/` probe, not a figure | | [2026-09-09](../status/history/2026-09-09.md), "`M76`: the arena cap is not the runtime's" |
| `M77` | 2026-09-10 | The re-vendor: `vendor/xz-seek` moves `5b549d72` → `87d55715`, taking the decode footprint, borrowed input and the decoder charge without the chunk. **Not a pin** — that crate's sweep and keystone are in flight and the copy takes its `HEAD`, so another sync follows. `StreamEntry` gains `first_block_dict_size`, so the cache envelope is reshaped and `FORMAT_VERSION` goes 16 → 17 — every existing cache is cold, koji's included. Unblocks `19.14`, which now has `Reader::decode_footprint()` to divide by; corrects the two doc comments that named `RangePlan::footprint` as excluding the dictionary | | [2026-09-10](../status/history/2026-09-10.md), "`M77`: the re-vendor, and the window route priced and refused" |
| `M78` | 2026-09-10 | The second sync `M77` owed: `vendor/xz-seek` moves `87d55715` → `f44d921f`, that crate's keystone, so the copy names a struck commit rather than a mid-flight `HEAD`. **No code moved** — the two commits between are that crate's `P8` wrap and the keystone strike, which touch docs, `tests/` and `harness/`, none of which the sync keeps, so `VENDORED_FROM` is the whole diff. No figure is affected: `vendor/xz-seek/src/` is the only declared path and its bytes are identical | | [2026-09-10](../status/history/2026-09-10.md), "`M78`: the sync onto the keystone" |
| `M79` | | The resolution is announced in two lines rather than one: what was **stated or discovered** before the source is touched — the flags as typed, and the limit with the file that stated it — and then what the source's recommendation and the allowance **fit**, which is the only line that can name a count the allowance lowered. `19.9`'s single line waits on `open_for_scan`, so on koji's multistream download a typo'd `--parallel-memory` goes unconfirmed through 85 s of footer walk. No decision moves: the distinctions, their spellings and their layer are `19.9`'s | | [2026-09-10](../status/history/2026-09-10.md), "`M79`: the resolution is announced twice, because half of it is knowable before the file is opened" |
| `M80` | 2026-09-11 | `clap`'s `wrap_help` feature, and a snapshot of every help page. Without the feature `clap` wraps at no width at all — not the terminal's, not `COLUMNS` — so the pages printed single lines up to **748** columns, which a terminal hard-wraps mid-word with none of the hanging indent that makes an option list scannable. Nothing rendered a help page, which is how it survived every session that edited those comments; `tests/help_text.rs` holds all eight pages plus the two properties a snapshot cannot carry — no line over the width, and no flag with nothing beside it. The second caught `info --detail`, which had no doc comment at all and printed blank | | [2026-09-11](../status/history/2026-09-11.md), "`M80`: help text is not wrapped, and nothing looked" |

| `M81` | | **Withdrawn — the row's mechanism is false, and the mirror it would have changed is correct.** The number stays spent | | [2026-09-11](../status/history/2026-09-11.md), "`M81` and `M82` are both withdrawn, and the account they were guarding is wrong" |

| `M82` | | **Withdrawn — not out-of-band, and unbuildable as written.** It reverses a decision `19.17` registered, so by the admission rule it needs a slice; re-filed as **`19.17.1`**. The number stays spent | | [2026-09-11](../status/history/2026-09-11.md), "`M81` and `M82` are both withdrawn, and the account they were guarding is wrong" |
| `M83` | 2026-09-11 | `PartitionRead::Whole` means **one of the source's units**, not one piece — `want = min(piece, unit)`, the xz arm stating `SeekTable::max_block_uncompressed()`. Behaviourally identical at the shipped width, where a piece *is* one unit, so it needs no re-measurement; above it the buffer is capped at one unit instead of growing with `k`, which removes the unbounded case and **not** the copy — a one-unit read is un-poolable either way, `BufferPool::keeps` admitting only the announced chunk, and a file-wide maximum can still cross into a smaller successor block. It is not what makes a wider cut safe; the boundary clip is. `19.20` decoupled the cut width from the read shape and widened `a_block_decoding_partition_spans_at_most_the_cut_width` from `== 1` to `<= k`, so raising `BOUNDARIED_PARTITION_UNITS` would have re-created the un-poolable partition-length buffer `19.19` removed with every test still green. Bounded by `no_read_a_worker_makes_exceeds_the_stated_unit`, which states `Whole` over a *plain* source because the one source that says it cuts single units and makes the bound vacuous | | [2026-09-11](../status/history/2026-09-11.md), "`M83`: `Whole` carries the unit, and the chunked bound's prose is corrected" |
| `M84` | 2026-09-11 | A censored `reserve` leg states what it still proves, and every fit states the window it covers. `Session.censored_bounds` reads the `maxrss_bound_kib`/`seconds_to_kill` pair `time_run`'s kill branch had been writing to `raw.json` with nothing reading it, and `_censored_constraint` prints the worst killed rep as a constraint line under the fits — never fitted, interval censoring being the right treatment at the wrong size for four legs. The claim printed is **"did not fit `-m <token>`"**, not `peak > limit`: `RT9` establishes that the kernel reaped a process in that cgroup, which says the arrangement did not run inside its allocation and needs no reclaim-ordering step; the wrapper's reading is printed beside it as a floor on a peak never reached. Both fit branches now name the legs that are **in** — `` `512m` at 2r `` — where each named only what left, because a leg is censored exactly when its resident ran closest to its ceiling and a line through the survivors is a line through the legs that had room | | [2026-09-11](../status/history/2026-09-11.md), "`M84`: a killed leg's constraint, and the window a fit covers" |
| `M85` | 2026-09-11 | The instrument's report is a **file named by `PGDQ_INTROSPECT_OUT`**, not a marker-bracketed block on stderr — unset meaning no report at all, so the instrument never writes to a stream: the bracket, the two shared marker constants and `measure.parse_instrument` go, `parse_reported` stays the reader with a file's text as its input, and an env var leaves the command shape identical where a flag would not. A file has one writer by construction, where a shared stream is a framing protocol paid once per writer — and this is the first of several self-reports a build is expected to make. With it, **a missing report becomes an error where one was expected**: today an absent block parses as `{}`, which is right for every default build and indistinguishable from a leg built without the feature, a leg pointed at the default binary, or a report that never arrived — a killed leg staying censored as `19.17.1` left it. Also makes the report **state each quantity's scope**: `live_*` counts Rust's `GlobalAlloc` and `mallinfo_*`/`malloc_*` are glibc's view of the whole process, so the two are not commensurable and their difference is not retention — `liblzma` is the active `.xz` backend in the shipped build and allocates through C `malloc`, ~9.47 MB a reader that the counter cannot see ([`architecture.md`](architecture.md), "What the binary can report about itself"). And re-files the instrument's numbers as the **per-rep readings** they are: `Session.reported` is keyed per spec and documents itself as facts identical across reps, which `live_peak_bytes` and the `mallinfo_*` fields are not — they are filed per rep in `raw.json`'s `instrument` dict, and `RunSpec.instrument` is the declaration that makes an absent report an error |  | [2026-09-11](../status/history/2026-09-11.md), "`M85`: the instrument's report is a file, not a framed stream" |
| `M86` | 2026-09-11 | **heaptrack as the project's libc-level instrument**, recipe printed by `measure.py --heaptrack-recipe` and never run by it, as `--profile-recipe` already is — the third invocation the harness owns without executing. It hooks `malloc`, so it sees C and Rust alike where the counting `#[global_allocator]` sees only Rust, which is the layer that stays correct as more C is vendored. What it records is `reserve`'s **path step** — the two stated budgets either side of `reader_bytes`, read as a `heaptrack_print --diff` — so the difference between them names the whole block path's allocation, decoder included, with no subtraction between sittings. **Five** details are load-bearing and each fails by returning a plausible report of something else, so `test_measure.py` asserts them: the **`profiling` profile** (`debug = "line-tables-only"`), without which no report carries a `.rs:` reference at all (3,627 against 0); `-C force-frame-pointers=yes` is **not** needed (heaptrack unwinds `.eh_frame`, unlike `perf`); **`--record-only`**, so the finished file is not handed to a `heaptrack_gui` that need not be installed and, where it is — Arch ships it in the CLI package — may not start; **`--merge-backtraces=0`**, because a merged frame prints the summed peak of the backtraces under it beside the merge's call count and `heaptrack_print --help` says that peak is not correct, which is what made one decoder's 8.39 MB read as "67.11 MB over two calls"; and **`c++filt`**, this workspace's symbols being Rust v0 while heaptrack's own Rust demangling is post-1.5.0 and absent from the installed build. Its row is in [`measurements.md`](measurements.md), "What an instrument can see". **Not a figure** — a `runs/` artifact read for attribution, never a median or an apparatus line |  | [2026-09-11](../status/history/2026-09-11.md), "`M86`: heaptrack, and the decoder term it reads straight off" |
| `M87` | 2026-09-12 | A recommended worker count answers to the allowance however the budget arrived: `ParallelArgs::resolve_in`'s stated-`--parallel-memory` arm bypassed `Parallelism::discover_in` entirely, so `--parallel-memory` with no `--jobs` reported a count it would not deliver — `24 (recommended by the source)` beside one serial reader. The standing rule scopes the lowering to the absence of `--jobs`, and `--jobs` is absent here, so this is an oversight rather than a decision; `Resolved::jobs_display`'s doc comment becomes true as written, and the clause it prints names which budget did the lowering — `by the allocation` against `by the stated budget` | | [2026-09-12](../status/history/2026-09-12.md), "A `parse` reports what was asked for and never what ran" |
| `M88` | 2026-09-12 | Every `reserve` cell states **which path it ran**, and the harness asks that question one way: `block_path_afforded` mirrors `BlockCache::affordable` as `charge_bytes(unit, 1)` — the per-reader term **plus that one reader's pool floor**, which `19.22` put inside it — where three of the renderer's four sites still compared against `reader_bytes` alone and the fourth did not. The line moved 58.03 MiB → **130.0** at 24 MiB blocks and 266 → **650** at 128, so the stale mirror mislabelled whole cells rather than an edge: `-m 512m` reads the streaming fallback on both inputs, and the path step's pair straddled a comparison neither leg made, pricing a path both had declined. A cell now names `*block path*` or `*streaming*`, and the two apparatus claims that said 512 MiB reaches the block path are corrected. **The row's own mechanism was wrong**: `parse` emits no decline note, that being a `PlanNote` on a query's `TableStream`, so the path is read off the budget the run *does* report | | [2026-09-12](../status/history/2026-09-12.md), "`M88`: the path a leg ran is read off the budget it reported" |
| `M89` | 2026-09-12 | The reserve axis reaches the pool floor, and a fit that cannot be checked is not published. Two limits join `RESERVE_LIMITS` — **`544m`** (budget 160 MiB, one reader at 24 MiB blocks, a 72 MiB floor) and **`1088m`** (704 MiB, one reader at 128 MiB blocks, a 384 MiB floor) — because the term is clamped off at `POOL_DEPTH` readers and every block-path leg on the four-limit axis resolves 4 to 24, so no published cell bills the term `19.22` added, `19.24` gave a column and `M88` corrected the mirror of; the two windows are disjoint (24 MiB blocks want a limit in [514.03, 616.13), 128 MiB blocks [1034.03, 1448.13)), so one limit cannot cover both. **Additive only** — the four existing limits and their readings are untouched, `512m` keeps the axis's worst headroom, and the figure has never been published, so nothing goes stale; +6 legs on 32. The fit guard rises from 2 distinct reader counts to **3**, a two-term model having no residual at two points and the renderer printing `±0 MiB` as though it were one; the 128 MiB family is `19.11`'s acceptance gate and not a source of numbers, so that line was never owed a fit. `--check` asserts the floor is reached on each block size | | [2026-09-12](../status/history/2026-09-12.md), "The gate never reaches the pool floor, and the 128 MiB family was never a fit" |

**One obligation outlived them and is most of the way discharged.** An
`INSERT`-run scan cost **mid-teens times** a `COPY` scan per byte, CPU-bound,
which argued for an `INSERT` fast path — and *that* changes a decision, so it
went through grilling → spec amendment → a numbered slice rather than through
this section. It is `KD9`, and the `INSERT` statement scan took it to
**4.9× warm**. The
entry stays live at that residual: part of it is a property of the two
algorithms and cannot go, and part of it is two named, untaken cuts
([`architecture.md`](architecture.md), "Bulk regions: one span kind, three
payloads").
