# Status

What is built, right now. Rewritten in place as state changes. How the system
*works* is [`../design/architecture.md`](../design/architecture.md), filed by
subject; what is still ahead is [`../design/roadmap.md`](../design/roadmap.md),
whose index table is the schedule; dated pickup notes and plan-changing
discoveries are in `history/`.

## What exists

P1–P5, P7, P9, P11–P13, P16 and P17 are complete and were struck at keystone
reviews; how each mechanism works is
[`../design/architecture.md`](../design/architecture.md), filed by subject,
which is where a session touching one meets its rejected alternatives and its
limitations. The capability table below says what state each is in.

[`../design/measurements.md`](../design/measurements.md) carries the `af15eac`
stamp of 2026-09-05, and **seventeen of its twenty-one tables come from one
sitting**: sixteen from the scan-performance wrap sweep and `session-drift`,
which no sweep can take because it is derived *across* two, from that sweep and
a second begun the minute it finished. The other four are `peak-rss`, taken alone
at `41c96bb`; `xz-decode-scaling`, taken alone at `7d21c6e`; `parallel-peak-rss`,
taken alone at `e29939c` and unmoved since (the sub-stream divisor never
reaches its command shape); and `parallel-scan-throughput`, re-taken alone at
`20fd77c` once that divisor change landed — the two
no longer share a commit, which was never a `shares` edge between them, each **saying
so inside its own figure marker**, which is where
`--stale`, acknowledgement spentness and `--verify-additive` each read the
commit they argue that figure from; none shares a reading with any other,
which is the condition under which a figure may be published outside a sweep at
all. So no table carries a partial-sitting note and no absolute in the
document is a cross-sitting reading. The fresh
stamp spent all six of the previous acknowledgements, which were deleted rather
than kept as sediment; the register carries three now — P7's wrap and
keystone, both comment-only, plus `a30cc43`, which is not: it
excuses `parallel-peak-rss` on reachability, an executable diff that never
reaches that figure's command shape. Under the previous `ba2fc12`
stamp ten of the seventeen stood outside the sweep.

**The largest correction the register has carried is `census-arrays`.** Its warm
census cost read 1.045 s and 1.49 µs a row under the old stamp and now reads
**0.201 s and 287 ns** — a factor of five — because the census's field split
went behind `memchr` and the split was the byte loop, not the census. It was
red on `pgdump_query/src/copy.rs` throughout, so nothing about the register
missed it; what a red figure never says is *how far* a number has moved.

**Four other findings changed shape rather than magnitude.** The census's two
tiers are now within a factor of four of each other — the pre-filter is 25% of
what an inspected row costs, against 4% before — so "one tier, not two" no
longer holds. `census-attribution`'s baseline gap fell from +0.874 s to
+0.033 s, which is inside the drift figure, so the untyped baseline's
file-dependence is now invisible without the census-off binary rather than
obvious with it. `predicate-terms` measures the shared field split, and its
term-count axis has gone flat: five terms at one depth cost +0.05 µs a row over
one, against +0.37 before. And the I/O-defaults ceiling fell from 8.9% to
**5.8%**, because the read-path work took the parse below the device by more
than it took the device — every I/O lever declined at 8.9% is declined harder
now.

**The query path roughly halved.** A typed control query went 10.36 s → 4.75 s
and the `--arrays --composite` file 19.93 s → 9.12 s, against `strings` legs of
4.42 s → 3.44 s and 5.36 s → 3.45 s; the nested-column increment went
**13.5 µs → 6.5 µs a row** and every row of `projection-widths` about halved
with the same ordering intact. The scalar decoders, the read path and the four
render-path changes are what moved them.

**One reading weakened a settled claim without reopening it.** `mimalloc` now
reads 0.97×, 0.97× and 0.99× on the three headline shapes — marginally ahead of
the platform allocator on all three, where the previous sitting read
1.00×/0.99×/1.01×. Every cell's spread overlaps the reference's and the largest
gap is 3%, so the decision stands and the platform allocator is kept, but the
statement behind it has weakened from "nothing beats it" to "nothing beats it by
more than the instrument's own noise". Reviewed and affirmed at P7's wrap; what
would reopen it is an instrument that resolves a 1% wall difference, not another
sitting of this one ([`../design/architecture.md`](../design/architecture.md),
"The allocator is the binary's choice").

**The pair that produced this stamp cleared the contention gate on the number
alone**, which the previous stamp did not: no warm floor is near the ~15%
threshold, and the two files' floors moved in *opposite* directions — `control`
−2.5% to −5.3%, `arrays` +3.4% — which is the opposite of the shared slow move
that disqualifies a sweep
([`../design/measurements.md`](../design/measurements.md), "The floor is read
directionally"). Session drift over 92 shared readings is a median absolute
**1.6%** and a largest 14.3%.

**Twenty of the twenty-one figures are stale, and no acknowledgement can
excuse them.** Six rounds of library work did it. The read-path work made the
buffer pool keep the chunk size a read loop announces, touching `io.rs`,
`scan.rs`, `stream.rs` and the CLI; the compressed-input work reshaped every
`ByteRangeSource` signature to a boxed future and added a second
implementation, touching `io.rs`, `cache.rs`, `index.rs`, `diagnostic.rs` and
the CLI again; the cache-replacement work put a refusal in front of all three
scan entry points, touching `cache.rs`, `index.rs`, `stream.rs` and the CLI
once more; the parallel-scan work moved `io.rs`, `scan.rs`, `stream.rs`,
`batch.rs`, `map.rs` and the CLI again on top of that; and the per-file-term
repair separated a partition's cut size from its memory charge, moving `io.rs`
and `leader.rs` once more; and the margin on the resolved count moved `io.rs`
again, twice — `19.23` adding it and `19.26` giving it its own constant —
though **no registered command shape's arrangement moves with either** —
the margin can only lower a count above one, every shape but the jobs-axis
families states `--jobs 1`, and those three state a budget as well, which is
the arm that never reaches a discovered limit; and the delivered-count report
moved `leader.rs` and `stream.rs`, adding a few comparisons per `COPY` block
and a status line no registered shape can print — `--jobs 1` returns before the
source's advice is read, and the jobs-axis families state a budget that affords
the count they ask for. All of them add
executable lines, so neither mechanical oracle applies: reachability excuses
only a diff no command shape executes, and byte-identity settles generator
changes alone.

**Three of the parallel-scan changes are red on their own terms rather than
inheriting another change's red**, because each moved what a *registered*
command shape executes. `pgdq query` runs the partitioned entry point at
whatever `--jobs` allows. `pgdq parse`'s mapping pass offers every `COPY`
region large enough to cut to the leader's scheduler, and every query's mapping
pass with it. And `main()` installs a `tracing_subscriber` on every invocation,
with one event around the seek-table build and one at each scan's start and
completion — every registered shape reaches at least the completion site. What
can be said without an oracle is that those lines are a fixed few per
invocation rather than one per row or per block, and that they read and write
nothing belonging to the dump. **The worker-count apparatus rule bounds the exposure**: every
command shape states `--jobs 1` rather than inheriting the CLI default, so a
figure re-taken today measures the serial arrangement the published tables were
taken under — and the CLI default's own fall to 1 therefore moved what a person
who states no flag gets and nothing the register measures.

**The rest are red on paths they share with those changes, and three carry a
reason of their own.** `xz-decode-scaling` is red on
`scripts/generate_xz_input.py`, which gained a `--block-size` flag: a generator
change, the one case a mechanical oracle settles, so `--verify-additive` is
available to whichever change spends it — checked by hand meanwhile on a 77 MiB
input, where the pre- and post-change generators write byte-identical `.xz`
files at the default. `session-drift` has been red on `scripts/measure.py`
since the harness took the derived direction of the borrow graph.
`parallel-peak-rss` is red on the read path since its own `e29939c` sitting,
with one change excused: `a30cc43`'s sub-stream divisor is inside
`stream::plan_partitions`, which its `pgdq parse` command shape never calls.
`nested-decode-micro` is the one figure of any kind still green, timing
decoders none of this touched.

**One published number actually moves**, the
`chunk-size` table's 16 MiB row, which was taken when a chunk that large missed
the buffer pool; that row is called out where it stands. A stale figure obliges
no sweep ([`../design/measurements.md`](../design/measurements.md), "A stale
figure does not oblige a sweep"), and a sweep is what re-takes these: seventeen
of the twenty-one tables from one sitting is the property the `af15eac` stamp has
and a partial sitting would spend.

**`peak-rss` is the figure whose red says least about its numbers.** `io.rs`,
`cache.rs`, `map.rs`, `scan.rs` and `stream.rs` all moved between `7ee5db5` and
`41c96bb`, and those are executable changes — but the re-take at `41c96bb`
reads 5.90 / 5.85 / 9.73 / 43.78 MiB against 5.88 / 5.64 / 9.53 / 43.59, every
one inside the other's spread. Three rounds of read-path, compressed-source and
cache work changed what a scan holds resident by nothing a three-rep instrument
can see, and the red it carries now is a struct that changed modules.

The two acknowledgements the register carries still stand and still hold for
what they name. `measure.ACKNOWLEDGED` records P7's wrap and keystone, whose
every hunk is a comment, a docstring, or a `quoted_by` edge into the phase docs
they deleted; each entry names the diff that re-checks it.
`scripts/acknowledged.py` is its own module precisely so that an acknowledgement
edit does not re-stale the stamp it was just given.

| Capability | State |
|---|---|
| Streaming row extraction from plain-format dumps, push and pull mode, resumable | working; a batch flushes on whichever of `max_rows`, `max_bytes` or `max_source_span` comes first, the last of which is what bounds the read chunks an in-flight batch pins ([`../design/architecture.md`](../design/architecture.md), "Three flush triggers") |
| Typed Arrow columns from `CREATE TABLE` DDL, with per-column resolution diagnostics; `SchemaMode::Strings` for the untyped path | working. `interval` is `Interval(MonthDayNano)` — PostgreSQL's own three fields — with v17's infinities and a time part past `2562047:47:16.854775807` an `Error::FieldDecode` and `--schema-mode strings` the recourse. `oid` is `UInt32` — PostgreSQL's one unsigned integer type, mapped where the ADBC driver's `Int32` turns an OID at or above 2^31 negative. `int2vector` is `List<Int16>` — the one built-in whose Arrow type is a container and whose name is not spelled like one, written as space-separated `int16`s with no quoting and no NULL element (I47), so it travels with a `NestedPlan` of its own exactly as a multirange does. A `uuid` column's field carries the canonical `arrow.uuid` extension name and a `json`/`jsonb` column's `arrow.json`, top level only and written through arrow-rs's own extension types, so neither changes a byte. Render-back has two entry points — `render_field`, returning `Result<Option<String>, Error>`, and the sink `render_field_into(.., &mut String)` it wraps, which appends and answers `Result<bool, Error>` so a caller printing a row builds it in one buffer — and a third outcome besides a value and SQL NULL: `Error::FieldRender` is an Arrow value with no PostgreSQL text form — reachable only from an array a caller assembled, since every column this crate fills comes from a decoder whose range its renderer writes back, and carried by `interval` alone, whose nanoseconds are finer than PostgreSQL's microseconds ([`../design/architecture.md`](../design/architecture.md), "Type resolution" and "Decoders and render-back") |
| Full byte-exact file map — every byte in exactly one span, verified over every fixture | working |
| DDL object inventory: TOC enrichment, referenced roles and tablespaces, object census | working |
| Best-effort structural cache with source-identity checking and cache-only inspection | working. `CacheMode::load` answers a `CacheLoad` — an index, or which of five reasons there is none: the four unusable `CacheStatus`es carried across plus the caller's own `Disabled`, which no status can express — so a caller that is about to scan holds the reason before it does any work rather than after. **The library never replaces cache data automatically**: the three scan entry points refuse a `SourceChanged` cache with `Error::CacheSourceMismatch`, naming the path, what the cache was written for and what this source is, before a byte of the dump is read; the other three unusable statuses still start cold, there being nothing at that path worth keeping. There is no override — removing the file or naming another path is the way out, and **every refusal says both in the same words**: the library's size-mismatch error, the CLI sentence a contradicted compression claim gets, and `info`'s `SourceChanged` arm, which names the two ways out *before* `pgdq parse` because `parse` refuses that same condition. The other three `info` sentences still reach `pgdq parse` on its own. **The CLI reaches the size-mismatch verdict before it opens the file** — `cache::claim` compares the recorded stored size against a `stat`, `open_for_scan` raises the library's own error for `parse`/`query` and `info` renders its own sentence — so all three keep the wording they had and none of them spends an `.xz` file's footer walk to reach it; the three scan entry points still refuse on their own, which is the guarantee an embedder holds ([`../design/architecture.md`](../design/architecture.md), "The cache" and "The CLI's two refusals are worded as one") |
| Arrays, composites, ranges, multiranges, `int2vector` | typed and decoded end to end: `List<T>`, `Struct<…>`, the five-field range struct, `List<`range struct`>`, `List<List<T>>`, and `int2vector`'s `List<Int16>` — a fifth literal form with no wrapper, no quoting and no NULL element (I47), compared through `anyarray` polymorphism's `array_lt`/`array_eq` so `'2' < '10'` is true where a byte comparison says false. Three shapes stay strings, each with its own resolution outcome — an opaque element type, an element type that is itself an array (I26), and values that disagree on shape. **Every container kind now compares structurally** — element-wise, field-wise and bound-wise through a `ComparisonPlan::Nested` tree, `array_cmp`'s shape tie-break, and one NULL rule at every level (I45) — with the literal read through the `*_in` supersets (I44) and each leaf in its own type's output form. Comparability and divergence are both inherited: a `json` position refuses the column's *ordering* and names itself, a `text[]` announces its element's collation. A column whose order one position refuses still answers `=` — over the container's whole text — and **says what that costs at the position that did it**, since `array_cmp` raises for a `json` element rather than comparing, so bytewise is an answer the server does not have; a position the *resolver* declined instead (I22, I26) resolves the column to text before any tree is read and is silent. **A range is put into the form the server stores it in before it is compared** (I46): `range_serialize`'s out-of-order refusal and empty-collapse, then the canonical function the three discrete built-ins have, so `int4range '[1,10]'`, `'(0,10)'` and `'[1,11)'` are one value and `'(1,2)'` is `empty`; a multirange's members are sorted, coalesced and emptied out before the sequence is walked. A user-defined range declaring a `canonical` function is refused under **every** operator, `=` included, since the server rewrites both operands through arbitrary server-side code before comparing them |
| Array shape census | recorded by every mapping pass and consumed: a query retypes its top-level array columns from the union over the blocks it will replay, before the first batch |
| CLI `pgdq parse` / `info` / `query`, including `--map`, `--json`, cache-only `info` | working; **`parse` is the only scanner** — it resumes from a matching cache, banks at `COPY` block boundaries under a self-tuning throttle, and saves unconditionally on Ctrl-C (exit 130/143). **It splits a `COPY` block's interior at `--jobs`** — the mapping pass offers each open region to the leader's scheduler, which declines a region smaller than one of the source's partitions, so a dump of small tables mostly reads serially and a dump of huge ones mostly does not; a cancelled region banks nothing and resumes like any other stop. **An omitted `--jobs` is the source's own recommendation** (`ByteRangeSource::default_workers`), not a constant: a plain file inherits the defaulted **one**, an `.xz` file answers `available_parallelism()` — already the minimum of the affinity mask and every ancestor cgroup's CPU quota — and a stated flag wins outright in either direction, there being no third spelling since `--jobs 0` is refused. So a flagless `parse` of a compressed dump reports the machine's cores on the `scan started` line and a plain one still reports `jobs=1`. **What that line announces is corrected where it is not delivered**: `scan arrangement` states the delivered reader count beside the announced one, once per scan, with which rule cut it and what the refused arrangement would have held — the number that buys it back, read off the source rather than re-derived. Silence means the announced count ran; the one refusal it does not report is a `COPY` region too small to cut, which is a property of that block rather than of the run ([`../design/architecture.md`](../design/architecture.md), "Status output"); nothing in the library reads the recommendation, so an embedder's silence is still `Parallelism::default()`. What the stated budget affords binds afterwards, through the divisor every count passes alike, and **a recommended count is lowered by whichever budget is in force** before that — discovered or stated, the rule being scoped to the absence of `--jobs` rather than to the absence of a budget, so only a count somebody typed is announced unlowered. **A discovered limit lowers it twice over**: once to what `limit − MEMORY_RESERVE` affords, and again to the count whose predicted resident leaves `MEMORY_MARGIN_PERCENT` of that limit unused — which is what makes the resolved arrangement a property of the allocation rather than of the host's core count, and which can never take the last reader or the budget it spends. `info` reports from the cache and never scans. **`query` reads its rows partitioned**, through `table_stream_partitions` at whatever `--jobs` allows, and merges them back into file order one batch per sub-stream, so the rows and their order are the same at every setting and only the reading changes ([`../design/architecture.md`](../design/architecture.md), "`pgdq query` merges the sub-streams back into file order"). Both scanning commands take `--chunk-size <bytes>`, whose 1 MiB default is the fastest of six sizes measured on the one device class where the size makes a difference ([`../design/measurements.md`](../design/measurements.md), "What the read chunk size is worth"); a raised value keeps its pooling, because each read loop announces the length it repeats, and costs four buffers of that size in RSS instead, or `--parallel-memory`'s worth, whichever is fewer — the pool's slot count falls out of that byte budget, which is what lets a slot be a decoded xz block ([`../design/architecture.md`](../design/architecture.md), "Execution model and API surface"). `--detail` adds each block's byte offsets, a per-column resolution line, an enum column's declared labels beneath it, a compressed dump's container shape (blocks, streams, largest block — `--json` carries the same object under `compression`), and — under the `user-defined types` count that heads it — one line per user-defined type, every `TypeKind` arm rendered with its payload. **Every help page is wrapped** — `clap`'s `wrap_help`, at the terminal's width, `COLUMNS`, or 100 columns in a redirect — and all eight of them (both depths, four commands) are snapshotted in `pgdump_query-cli/tests/help_text.rs`, which also asserts outright that no line overruns the width and that no flag renders with nothing beside it ([`../design/architecture.md`](../design/architecture.md), "A flag's help is its doc comment, citations included"). Text output shape is provisional; `--json` carries no shape promise at all, and states the labels once per type in `metadata.databases[].types[]` rather than per column |
| Partial reporting | `info` reports an unfinished scan's cache for as far as it got, with `Scan completion: N%` stated once at the top and nothing below it qualified. An interrupted cache is **typed** for every database segment the scan finished (I1) |
| Measurement harness | `scripts/measure.py` takes every figure in [`../design/measurements.md`](../design/measurements.md) and emits that doc's tables — twenty-one figures the doc carries, sixteen taken by a sweep, one derived across two and four taken alone, each declaring what invalidates it, which documents repeat it, and which readings it borrows from another figure — that third edge is what lets `--figure` pull in what a figure borrows and name the rest of the set that must be re-taken with it, and `--alone` asks for a diagnostic sitting: it borrows nothing and marks its whole run unpublishable, joining `--reps` and the size override, so the publication refusal below stops firing for it by construction rather than by an exemption. **Nothing it runs inherits a `--jobs` default**: `measure.SWEEP_JOBS` is stated by every command shape, by the profile recipe's argv and by the three untimed invocations that publish numbers all the same — the host `parse` behind every per-row divisor, the `strace`d save count, and the RSS attribution's nine legs — while koji takes the count as a parameter (`--koji-jobs`), its leg being a leg at some count against a serial one. `--check` refuses a shape carrying neither `--jobs N` nor, for the decode instrument, `--workers N`, with two exemptions and both named as such — `dd`, which is not a run of ours, and the reserve instrument's **flagless** legs, whose reading *is* the count a run stating nothing resolves (`measure._NO_FLAGS`), and which `flagless_flag_problems` holds from the other side by failing one that states either flag; a test parses `_script`'s own branches back out of its source — resolving a branch that dispatches on a *named* tuple of prefixes, not literals only — so a shape added there cannot be exempted by not being enumerated, and a second test holds the doc's apparatus line to naming the count the harness pins. The value is **1** because that reproduces the arrangement every published table was taken under, so raising it is an apparatus change owing a re-sweep exactly as the allocator would ([`../design/measurements.md`](../design/measurements.md), "The apparatus"). **A figure whose axis *is* the worker count is the one exemption, and it is declared at both ends**: three command-shape families are named in `measure.JOBS_AXIS` and take their counts from `measure.PARALLEL_JOBS`, and `pinned_count_problems` — a second `--check` refusal beside the first — fails a shape pinning some third number without declaring itself an axis. So the exemption is from the constant, never from stating a count. **The boundary of that register is declared rather than inferred**: a section outside it carries an `<!-- outside-register: <id> -->` marker, `--check` resolves those against `measure.NOT_OURS` in both directions and fails on a declared section that also carries a figure marker, and the doc's session stamp is scoped to the markers rather than to everything printed below it. **A declared section still carries an invalidation edge**, and its marker names the commit its readings were taken at, so `--stale` reports one whose inputs have moved beside the figures and an acknowledgement may name it — being outside the register means the harness cannot re-take the readings, not that nothing is told when they go wrong ([`../design/measurements.md`](../design/measurements.md), "The apparatus"). **A figure taken outside the sweep declares its sitting commit inside that same marker**, and `--stale`, acknowledgement spentness and `--verify-additive` each argue from that figure's own commit rather than from the stamp — permitted only where the figure stands in no borrow edge in either direction, which `--check` fails on, along with a sitting that does not descend from the stamp and a stamp whose accounting sentence is not the one the harness generates; `--figure` refuses such a sitting before the measurement is spent ([`../design/measurements.md`](../design/measurements.md), "A figure may be published outside the sweep"). Its reverse direction is **named rather than taken**: a table whose row is a difference over another figure's reps — `cross-file-floor` over `nested-end-to-end`, the register's one such edge — is not a closure edge, so `--figure` names it in the run log and in the emitted header when a sitting re-takes the reps it is derived from, and `--check` reports the relationship beside the partial sittings ([`../design/measurements.md`](../design/measurements.md), "The apparatus"). `measure.UNTAKEN` carries two entries. `rss-attribution`: the standalone attribution script was folded in, so the figure has an id and both declared edges, but the readings `measurements.md` prints under that heading were the script's and the section keeps its `outside-register` declaration until a sweep takes the figure — `M74`, the only sitting that may, its reference row running `peak-rss`'s two block-count shapes, which the two must share as one reading rather than measure separately as they do today. And `reserve`, which has no table in the doc at all and no marker with it: it is the compressed path's account as well as the budget rule's one number, four families under one id — the stated budget axis, twelve flagless legs whose axis is the **container limit** and which carry it on the `RunSpec` rather than in the command shape, three mechanism legs at one block size, and the path step one byte either side of what **one** block-decoding reader costs the budget rule — the per-reader term plus that reader's share of the pool's retention list, which is `BlockCache::affordable` exactly, asked in the one place (`block_path_afforded`) that also labels every flagless cell `block path` or `streaming` — and the edges it will owe when a sweep publishes it are one (`peak-rss`'s `control` row) plus a stated non-edge (`rss-attribution` shares no run with any leg of it), both computed by a test rather than asserted. **That figure also checks a model rather than reporting readings alone**: `charge_model` predicts what each flagless cell held from what the budget rule billed it, names the block pool's retention list — the `(POOL_DEPTH.max(jobs) − 1) × unit` the per-reader charge does not carry — as a column of its own, and answers a criterion registered before the sitting, now in **three** lines: the unnamed remainder is non-negative, a negative one being an over-bill a headroom sweep reads as headroom; no larger than `MEMORY_UNPOOLED_BOUND`, the number the margin predicts a count's resident with; and no larger than `MEMORY_RESERVE`, which is what covers it by construction. A refuted cell is **named in the emitted verdict and does not raise**, a remainder outside the band being a finding about the library rather than an apparatus fault. **The verdict names which of the three lines each faulting cell crossed, and `19.11`'s gate is an enumeration of bands rather than a threshold**: `bound` alone is released — the allocation is intact *and* the sitting re-derives that constant from its own cells — while a `rule` breach and an over-bill both bar, as does any band nobody has recorded a stance for, which `--check` reports until somebody does. **The pool column is filled in at every cell**: the term was `max(0, POOL_DEPTH − jobs) × unit` and so clamped off at four readers, which is why `544m` and `1088m` were registered under the clamp, one per block size; `M93` restated it as what the pool holds, so it is billed at every count and is the larger half of the charge above four readers. The two limits stay as additive legs whose reader counts no other limit resolves, and the four earlier limits and their readings are untouched. **A published line needs three distinct reader counts, and below that it is a secant**: `measure.RESERVE_FIT_MIN_COUNTS` is a property of the two-term model rather than of one family, so it sits at `_fit_or_secant`, the boundary all three fitted lines cross — the flagless fit, its band and the instrument account's own line. **That boundary is also where the pool's retention list comes off**: the charge is piecewise linear with a kink at `POOL_DEPTH`, the bend is in the measured resident and not only in the bill, and both registered families straddle it — so a single straight line across it reads the intercept ≈49 MiB high at 24 MiB blocks and ≈421 MiB high at 128, and the slope 31% low. `_depooled` subtracts `(POOL_DEPTH.max(jobs) − 1) × unit` — arithmetic over quantities all known before the sitting and already mirror-checked against the library's constants — and the line is fitted to the remainder, so both published terms are what a leg held *outside* the pool and the `98% of reader_bytes` comparison has a `reader_bytes` on the other side that excludes the same list. A test holds `_least_squares` to exactly one call site, inside `_fit_or_secant`, so a fourth line cannot be fitted around it, and a leg that declined the block path is out of every line by name, there being no retention list to take off a streaming leg. A two-term model passes exactly through two points, so its residual there is zero by construction; the *slope* is not, being the measured difference between two named legs, so below three counts the slope is published with no fixed term and no residual beside it, which is what keeps `19.18`'s `98% of reader_bytes` comparison alive in a sitting a kill has censored. `_least_squares` keeps its own floor of two, which is where the arithmetic rather than the publication stops, and `reserve_axis_problems` carries the static half — a registered axis whose limits afford fewer than three distinct counts is refused before a sitting is spent, a necessary condition and not a sufficient one, since a host with fewer cores collapses two fits onto one count. **It builds two instruments and refuses to build a third**: the `allocator` figure's legs, and the `xz_decode` example the decode-scaling figure runs, which is an *example* target so that `target/release/pgdq` — the binary every other figure is timed against — is never replaced by a build the harness made for one figure. A figure may also declare its own container memory, which is an apparatus departure its own table states: the register's line is a 512 MB container, and twenty-four decoded 24 MiB blocks are not one. Its inputs are no longer all plain dumps either — an input names its own suffix, and one may be *derived* from another, folding that one's stamp into its own so a change to the perf generator regenerates the compression of it as well. **The one binary it refuses to build, it now refuses to trust unstamped**: `runs/pgdq-nocensus` carries a `.stamp` naming the commit it was built from, the way a generated input does, and a `census-*` figure whose stamp is missing, unreadable as a commit, or not an **ancestor** of the commit being measured is refused in the first second — a census figure being a subtraction that charges everything differing between the two trees to the census. An ancestor is tolerated only where no path the census figures *being taken* declare changed in between, read through the same prefix predicate `--stale` argues staleness from and off the selected figures' own `depends`, so the refusal names the declared path that moved ([`../design/measurements.md`](../design/measurements.md), "The census-off binary is a source patch"). The binary in this tree is `f5768e7`, and read-path work has moved since, so the next census sitting rebuilds and re-stamps it. It also builds and interrogates the `allocator` figure's three legs, reading each binary's allocator out of `pgdq --version` rather than trusting the flags it passed, and names the shipped one in the session stamp. A leg is rebuilt **once per harness process** rather than reused from `runs/`, which is what stops a fresh reference being timed against last session's legs, and all of them are built before the first reading rather than at the rep that wants one |
| Column projection | working, library and CLI: `QueryOptions::projection` names columns, cuts the reported `ResolvedSchema` with the batches, may reorder, and may be empty (`COUNT(*)`); `pgdq query` spells it `--column <name>` repeated, or `--no-columns`, which prints no header so `\| wc -l` is a row count. A filter may name a column the projection does not, and an unprojected column is never decoded, so projecting a column away escapes its `Error::FieldDecode` — including `KD2`'s, which the error message does not name ([`../design/architecture.md`](../design/architecture.md), "Projection"; [`../manual/type-handling.md`](../manual/type-handling.md)). Measured on one 3.00 GiB file at five widths: `--no-columns` is 1.61 µs a row against 12.97 for all 19, the two array columns alone are +6.03 and the composite +0.75 ([`../design/measurements.md`](../design/measurements.md), "What a column costs") |
| The filter expression, evaluated three-valued | working: `QueryOptions::filter` is one `Expr` — `Term`/`And`/`Or`/`Not`, `And` and `Or` n-ary — evaluated in SQL's `True`/`False`/`Unknown` domain, a row surviving only where the root is `True`. A NULL field is `Unknown` under every comparing operator, which is the row set the old collapse gave for every conjunction and is what makes `Not` expressible at all. `IS DISTINCT FROM`/`IS NOT DISTINCT FROM` come with it, being the one thing `Not` cannot spell. Short-circuiting is defined against the *root*: `And` stops at the first non-`True` unless a `Not` is above it, which is where a decode failure surfaces or does not. Nothing folds two terms, so a contradictory pair is a query with no rows. Reachable from the CLI as well as the library: `pgdq query --where <expr>` builds the tree and a repeated `--filter` still builds the conjunction ([`../design/architecture.md`](../design/architecture.md), "Predicates") |
| The `--where` expression grammar | working, CLI only — `Expr` is an enum an embedder fills in, so nothing below L4 parses an expression. Parens group, `NOT` binds tighter than `AND` and `AND` tighter than `OR`, the keywords are case-insensitive and are keywords only outside quotes, and everything that is not a paren or a keyword is a term handed to the `--filter` grammar unchanged. A keyword is recognised only against whitespace or a paren, so `tag=and` stays an equality; a `NOT` after the word `is` belongs to the term, so `IS NOT NULL` and `IS NOT DISTINCT FROM` survive whole; juxtaposition is not an implicit `AND`; and a value holding a paren must be quoted. Both flags together are one conjunction. **No `--filter` string changes meaning** — that is what the separate flag buys ([`../design/architecture.md`](../design/architecture.md), "`--where` builds an expression out of those terms"; [`../manual/type-handling.md`](../manual/type-handling.md), "Combining terms: `--where`") |
| The `--filter` term grammar | working, CLI only — `Predicate` is a struct an embedder fills in, so nothing below L4 parses a term. Whitespace outside quotes is trimmed on both sides of the operator; `'` and `"` both quote either side, matching pairs only, with an interior quote doubled; the operator split skips quoted regions, so a column named `a=b` is askable; and the `IS NULL` forms are the fallback, tried only on a term with no operator, which is what makes `note=this is null` the equality it reads as. `IS DISTINCT FROM`/`IS NOT DISTINCT FROM` are candidates at the same positions the punctuation spellings are, so the earliest operator still wins in both directions, and the phrase needs whitespace on both sides — which is what leaves a column named `is distinct from` askable as `is distinct from=x`. A malformed quote is refused, never reinterpreted. `--column` and `--table` take their names verbatim and say so when a quoted-looking name is not found ([`../design/architecture.md`](../design/architecture.md), "A filter term is parsed for two audiences"; [`../manual/type-handling.md`](../manual/type-handling.md), "Writing a filter term") |
| Typed ordering operators (`<`, `<=`, `>`, `>=`) | working, library and CLI: each side is decoded with the column's own decoder — the field per row, the literal once when the block's schema resolves — and the decoded values compared, so `9 > 10` is true on an `integer`. Available on a column that resolved `Mapped` and that the comparison register gives an order to — which now includes any container whose every position is comparable — and refused on any other, which is also why `--schema-mode strings` refuses every one of them. An undecodable literal is `Error::PredicateValueDecode` before any row; a field that is genuinely undecodable is `Error::FieldDecode`, worded as the build path words it ([`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed") |
| Typed `=` / `!=` | working, library and CLI: they route through the same per-column `ComparisonPlan`, so `--filter 'price=1.5'` matches a `numeric(10,2)` written `1.50` and `--filter 'code=ab'` matches a padded `char(10)`. **Three canonicalizations**: the literal rendered once into the file's `*_out` form (everything not below — `=` on a `text` column is the byte comparison it always was), the field narrowed per row (`character(n)`), and both sides decoded per row for the seven kinds where the file's spelling is not unique or where reproducing `*_out` would mean re-implementing an output function — bare `numeric`, `interval`, `jsonb`, `real`/`double precision`, `time with time zone`, `inet`/`cidr`. **A nested column takes a fourth shape**: both operator families go through one structural walk over a `ComparisonPlan::Nested` tree, so `--filter 'tags={a, b}'` matches an array written `{a,b}` and the same walk answers `<`. Equality is never *refused* on a column the register does not compare; it falls back to text, a guess for an unmodelled scalar (`KD10`) and, for a nested column one of whose positions has no order, an answer the server does not have — announced at that position rather than passed over in silence. A literal that is not a value of the column's type is `Error::PredicateValueDecode` before any row, on the same output-form-only grammar, and the refusal names the form that column's comparison reads rather than only the value it turned down — a `boolean` is written `t` or `f`, an enum's clause lists its declared labels (the first twelve, then a count) and a `numeric(p,s)`'s names the scale it refuses a finer literal against ([`../design/architecture.md`](../design/architecture.md), "Equality is typed too"; [`../manual/type-handling.md`](../manual/type-handling.md), "`=` and `!=` compare values, not spellings") |
| Six string-shaped types that order typed, and `interval`, which now has an Arrow type and still does | working: `interval` by `interval_cmp_value`'s 128-bit span, so `1 mon`, `30 days` and `720:00:00` are one bound; `time with time zone` by the UTC instant then the stored zone; `inet`/`cidr` by family, shorter prefix, netmask, address; `macaddr`/`macaddr8` by their octets (I40); and `jsonb` by `compareJsonbContainers`' walk down two documents — kind before value, a container's size before its members, and a top-level scalar inside the pseudo-array that makes it outrank `[]` (I41). Only `interval` has an Arrow type; for the other six the *ordering* is the only path that decodes them, and for `interval` the ordering's fused span and the decoder's triple are two consumers of one grammar walk. The first six read a literal in the type's own `*_out` spelling and no wider — `1 month` and `08-00-2b-01-02-03` are refused by name, which is a property rather than a deficiency since every value a dump holds is already in the accepted form. `jsonb` is the exception and takes the whole of `jsonb_in`, because `{"a":1}` is what a person types and `{"a": 1}` is what the file holds ([`../manual/type-handling.md`](../manual/type-handling.md), "Six string-shaped types still order the way PostgreSQL orders them" and "`interval` keeps its three fields") |
| PostgreSQL's special values under an ordering operator | answered exactly, not raised as a fault: `-infinity` below every finite value, `infinity` above, a `numeric`'s `NaN` above `infinity` and equal to itself (I34), each in the spelling its own type writes — `date` writes `infinity` and `numeric` writes `Infinity`, and neither answers to the other's. A **bare** `numeric` carries all three; one with a typmod carries only `NaN`, since any typmod rejects an infinity, so the two infinity spellings are refused there as a filter literal. Carried as a position in the order rather than as a number, since no Arrow type has one. **A filter is therefore exact where the batch still cannot hold the value** — the row `--filter 'v_date<2020-01-01'` selects for `-infinity` fails to build if `v_date` is projected, which is a property of two paths with different powers, not a defect (`KD8` is the materialization question). ([`../manual/type-handling.md`](../manual/type-handling.md)) |
| The comparison register | **L2**, in `pgtype.rs`: `comparison_for(declared, collation, types)` answers a `ComparisonPlan` per **column**, carried as `ResolvedSchema::comparisons` and rendered as a table in [`../design/architecture.md`](../design/architecture.md), "Ordering operators compare typed". `predicate.rs` reads the plan and names no `DataType`. Eighteen of its twenty-six rows agree with PostgreSQL (I33, I34, I37, I38, I40, I41, I45, I47); six diverge, and only one statement among them is a deficiency — a column that *states* a collation this build does not implement, whether the dump calls it deterministic or not (`KD7`, two rows). The other four are properties: a collation no plain dump records, reached through a column and through a `jsonb` string leaf, and `json`, which the server does not order at all. **A divergence is operator-conditional**: `ComparisonDivergence::affects_equality` answers `true` for `json`, for `KD10`'s unmodelled scalar, and for a collation the dump declares `deterministic = false` (I42) — the three collation variants either side of that one answer `false`, a deterministic collation making `texteq` a byte comparison whatever it orders, so a `text` column with no clause warns under `<` and is silent under `=`. **Two rows are nested and neither is a scalar answer**: each carries a `NestedCompare` tree whose verdict is inherited from its positions, so "does this column have an order" is `ComparisonPlan::orders()` and never a match on the variant. The range row's node also carries a `discrete` flag, read off the range *type* rather than off its subtype, which is what says whether the bounds are rewritten on the way in. The plan is `Clone` rather than `Copy`, because three of its comparisons carry a fact about the column: an enum's labels, whether a `numeric`'s typmod excludes the infinities, and a nested column's whole tree. Exhaustiveness is `builtin_scalar` answering the Arrow type and the comparison in one arm, plus a wildcard-free `match` over `TypeKind`. A divergence is announced by `pgdq query` once on stderr, and read by an embedder from `TableStream::comparison_notes` — a third channel, since the signal is per-**term** *and* predicate-conditional (L4) |
| The declared collation, and the dump's own `CREATE COLLATION` list | read, and both move the verdict rather than the comparison: `ColumnDef::collation` keeps a column's `COLLATE` clause verbatim (`pg_catalog."C"`), `TypeKind::Domain` keeps a domain's own as the type default a column-level clause overrides, and the register answers **agrees** for an explicit `C`/`POSIX` and for a bare `name` column, **diverges** for any other stated collation and for a `text`/`varchar` column with no clause at all (I32, I37) — a user-defined collation the same dump declares `locale = 'C'` included, which is correct rows plus a note the user can ignore, and so a property rather than a `KD<k>`. The clause is found wherever `pg_dump` displaced it, past a `DEFAULT`, a `GENERATED … STORED` expression and a `NOT NULL`, which `fixtures/<13–18>/types/default.sql` now carries. `character(n)` is the fourth collatable arm and reaches the same three verdicts: `CompareKind::PaddedText` takes the dump's blank padding off both sides first, which is `bcTruelen` (I38), and the clause then decides exactly as it does for `text` — so a `char(n)` declaring `COLLATE "C"` agrees. **A fourth verdict is read off a statement rather than a name**: `CREATE COLLATION` reaches `DatabaseMetadata::collations` as a `CollationDef { name, deterministic }`, and a clause naming a collation the same dump declared `deterministic = false` answers `NonDeterministicCollation` — the one equality divergence a plain dump states outright (I42), since `pg_dump` writes that clause unconditionally and the server allows it for no provider but ICU. The two spellings are joined parsed rather than as text, and an unqualified reference matches on the name alone, which announces rather than stays silent. **Both determinism answers come off committed bytes**: `fixtures/<13–18>/types/` declares `public.c_collation` (libc) and `public.nd_collation` (`provider = icu, deterministic = false, locale = 'und'`), byte-identically at every major, with `t_collate.v_nd` a column of the second whose clause is spelled exactly as `v_user`'s and which answers differently. The ICU exclusion stays scoped to answers — no oracle case, `fixtures/*/oracle/` byte-unchanged — and the `collversion` reaches one flag set only, `types/binary-upgrade.sql`'s, where a test guards the shape claim and requires the six majors to agree on the version without naming it ([`../design/architecture.md`](../design/architecture.md), "Fixtures"; [`../manual/type-handling.md`](../manual/type-handling.md), "Text ordering is bytewise") |
| Comparison oracle | `fixtures/<13–18>/oracle/` holds what PostgreSQL itself answers for 2096 typed comparisons and 331 literals per major, each cell recording whether the server *accepted* the input, generated by `scripts/generate_fixtures.py --skip-dumps` and committed ([`../design/architecture.md`](../design/architecture.md), "The comparison oracle"). The answers are **glibc's** — every fixture container is the Debian (`-trixie`) image — and every text pair is asked twice, under `COLLATE "C"` and under the database's own collation, so both halves of the register's text row are in the file rather than argued. `character(10)` is asked under both too, with a tab-bearing value that puts I38's ordering corollary in the file, and a case's `collation` now means one thing only: `None` says the register does not branch on the clause for that type. A pair is asked through two *columns* of the declared type, so a bare case measures what a bare column in a dump does: `name`'s cells are its own `C` type default, and `public.text_c` — the domain whose own DDL carries the clause — is in the file beside it on the `user/Domain` arm. A literal the server refuses therefore never reaches an operator, and answers all six cells with the rejection `literals.tsv` records for it. `jsonb`'s sixteen values are one per branch of `compareJsonbContainers` — both booleans, a string, both empty containers, a one-pair object whose key sorts after a two-pair object's, and the pair separating storage order from alphabetical — so I41's facts are in the file rather than in a probe |
| Register against the oracle's answers | working: `the_register_answers_every_committed_oracle_cell`, a unit test in `predicate.rs`, puts every committed cell to the register through the same `resolve_term`/`ResolvedTerm::eval` path a `--filter` takes — **52,338 cells over six majors, all six operators**. Its one-column schema is built by `resolve_columns` from a synthetic `DumpMetadata`, so resolution, nested plan and comparison plan agree the way they do in a real query. It skips an `E`-cell (what the server refused), a NULL right operand (which the filter grammar cannot spell), and — *for the four ordering operators only* — a column the register refuses an ordering operator on, whose set is asserted exactly; `=`/`<>` are still asked of those columns, because equality is never refused. A NULL **left** operand is not skipped: the server's `u` cell is asserted against `Truth::Unknown` itself, rather than against the exclusion it collapses to at the root. **Forty cases are permitted to disagree and every one of them does, in every cell its divergence reaches**: a `jsonb` string leaf (2), `text` under glibc's `en_US.utf8` (30) and a `text[]` element under the same collation (6) — one statement at three depths — over 24 ordering cells each; and `box`'s area equality (2) over 6 `=` cells, PostgreSQL defining no `box <> box`. A disagreeing term must announce a `ComparisonNote` **under that operator** ([`../design/architecture.md`](../design/architecture.md), "The register against the oracle's answers") |
| Cross-major differ | working: `scripts/oracle_differences.py` walks the majors as a chain of adjacent pairs and files every cell that moved in `fixtures/oracle-differences.tsv` — **533 differences across 13–18, every one of them additive** (I35), so the union rule is checked rather than asserted. `test_oracle_differences.py` asserts the committed file against a fresh computation and, separately, that no difference is non-additive; an oracle pass of `generate_fixtures.py` ends by running the same check ([`../design/architecture.md`](../design/architecture.md), "The cross-major differ") |
| Register-to-oracle reconciliation | working: `scripts/oracle_register.py` reads the register's arms out of `pgtype.rs` — one per declared base name in `builtin_scalar`, one per `TypeKind` match arm in `comparison_user_type`, the three branches of the walk that are not match arms, and the four branches of `collated_text` — and joins them against the case table both ways, failing on either. **42 arms, 55 cases, nothing uncovered and nothing unplaced.** One arm carries an exemption instead of a case and is reported under its own heading: no oracle case can reach `collation/non-deterministic`, a non-deterministic collation being ICU-only (I42) and an ICU case carrying the `collversion` drift the oracle excludes ICU to avoid. **An exemption names where the arm's evidence is** — `(file, needle)` pointers the check resolves, three unit tests today — because the reason alone says why the oracle cannot cover the arm and nothing about what does; it goes stale from both sides, an exempt arm that acquires a case being a problem and evidence that stops resolving being one too. The pointers name sufficient evidence rather than exhaustive, so the fixture bytes that now carry the shape owe no edit there. Each collation branch is anchored on a string the parse must find, so deleting one is reported rather than shortening the list. The collation is a second dimension: a case's label picks the arm, `C` reaching the bytewise branch and `default` the other two, and the `datcollate` that makes that mapping sound is read out of `meta.tsv` rather than assumed. An oracle pass of `generate_fixtures.py` ends by running it beside the differ ([`../design/architecture.md`](../design/architecture.md), "The register-to-oracle reconciliation") |
| ADBC floor oracle | `fixtures/<13–18>/adbc/floor.tsv` holds what the Arrow ADBC PostgreSQL driver (`adbc_driver_postgresql` 1.12.0, pinned in `scripts/pyproject.toml`) returns for every declarable `pg_catalog` type — 74 rows at 13, 82 at 14–18, taken from the host over a published port by `scripts/generate_fixtures.py` and committed ([`../design/architecture.md`](../design/architecture.md), "The ADBC floor oracle") |
| The floor rule, reconciled | working: `scripts/floor_mapping.py` joins the oracle against `builtin_scalar` and fails both ways — every floor row the rule reaches is met or carries a stance, every arm resolves to a floor row, and every stance is about a row that still needs one. **58 of the 82 rows a major are placed by the file's own columns** (`arrow.opaque`, or a driver refusal), 21 of the remaining 24 are simply met, and three carry a stance: `money` below by decision (`KD13`), `regproc` unanswerable because the two encodings denote different values, and `oid` answering `UInt32` where the driver answers `Int32`, which the rule permits. The fourth stance the rule defines — `waiting`, for a row a slice of the open phase closes — is carried by no row now that `interval` and `int2vector` have both closed, and is exercised against a synthetic row in `test_floor_mapping.py`. D8's pin is asserted here — the driver version every row records must equal `scripts/pyproject.toml`'s ([`../design/architecture.md`](../design/architecture.md), "The floor: the ADBC driver's answer bounds ours") |
| Compressed input (`--source foo.dump.xz`) | **working, serially and at any `--jobs`** — the `--jobs` 2-or-more deadlock is closed by classing the block pool as the **retaining** holder it is, so no read loop's `WaitPolicy` reaches it and `BlockCache::slot`'s drain is a reuse rule rather than a progress guarantee; what the block pool's budget bounds is stated in two terms, `slots` retained blocks and `stream::worker_count` live ones ([`../design/architecture.md`](../design/architecture.md), "Execution model and API surface"). `.xz` reads end to end through `parse`/`info`/`query`, in all three container shapes, the span offsets it produces being uncompressed ones so nothing above L1 knows. `pgdump_query::open_local` recognises a source by **content**, not by name, and hands the CLI's three commands an `XzSource` or a `LocalFileSource`; `XzSource` answers `size()` from the stream index and `stored_size()` from a `stat`, and hands its seek table to the cache, which persists it in a `CompressionIndex` envelope field beside a `ContainerKind` that stays `Plain`. **A read decodes the blocks it lands in**, into a second `BufferPool` of the block unit, with the reader's mutex held only long enough to name an `xz_seek::BlockTask` — so concurrent `read_range` calls decode concurrently, and a read inside one block is a zero-copy slice of it rather than a copy out of a decoder. Decoded blocks are retained LRU-first under that pool's own budget, eviction running *before* a slot is taken so the budget bounds the retained and the free together: a serial `parse` of the 3.00 GiB `.xz` control holds 64.7 MiB against the streaming form's 16.2, and the two produce byte-identical caches. A block the caller's stated budget cannot hold keeps the streaming reader restarted on seek — in practice a single-block file, whose one block is the whole plaintext, which is what plain `xz` writes whenever it is not threading, so that fallback is the ordinary path for a large archive rather than a concession to an edge case. What the line declines is **memory, not seekability**: it is keyed on the largest block rather than the block count, so a multi-block file written with large blocks is declined by it and does have parallelism to lose — under the 64 MiB default that is `xz -9 -T0` and `xz --block-size=128MiB`, and `--parallel-memory` is the recourse. **The decline announces itself on both commands**: `pgdq parse` reports the serial arrangement it delivered on the `scan arrangement` status line, and `pgdq query` prints a `PlanNoteKind::CompressedBlockPathDeclined` once on stderr, naming the file's largest block beside the budget that declined it **and the whole of what one reader of it would have held**, read off the source (`ByteRangeSource::block_decode_bytes`) rather than re-derived by the note, so the flag that says *raise it* says what to raise it to. It is a plan note and not a `DiagnosticKind` because a decline is a property of the file *and this run's budget*, which is also why no cache can report it; what a cache can report is the container's **shape**, and `info --detail`/`--json` state it from the persisted seek table as `cache::CompressionShape` — blocks, streams and largest block, answered with no dump file present. **That table is read back**, so the stream-footer walk — one read for a single-stream file, **85 s** for the 31,150-stream koji download — is paid by the command that first parses a file and by nothing after it: `cache::claim` answers a three-state `KnownCompression` before any source exists, `open_local` takes it and builds the source through `XzSource::with_table` without walking, and a claim the file contradicts is `Recognized::Mismatch` — the whole cache unusable, since table and span index were one `save`, and refused by all three commands having read nothing, in a sentence of its own rather than `CacheStatus::Unreadable`'s. **The sibling refusal is free as well**: `claim`'s second outcome is a cache recorded against a file of another stored size, so that too is settled from the envelope and a `stat` and no source is ever opened — one outcome added to the read that was already happening, not a second read, and every other unusable outcome still collapses to `KnownCompression::Unknown`. **A source advises its own partitioning**, through a defaulted trait
method that reads off the read path it took: block boundaries where it
block-decodes, at **what one concurrent reader holds** each — the block unit
twice, the chunk buffer a straddling read is assembled into, and the decoder's
own retention (`xz_seek::Reader::decode_footprint`, 9,471,776 B on koji's
8 MiB-dictionary shape), which is 58.03 MiB against 24 MiB blocks where a
charge of one block and a chunk billed 25 for a sub-stream measured at 59.4 —
and **one
partition** on the streaming fallback however many boundaries its table has,
since reaching an offset inside a block there restarts that block and two
workers would each force the other's restart. `LocalFileSource` answers
anywhere, **eight read chunks each** — a partition of one chunk made a worker's
chunk-sized tail read double the file, which is what the plain path's 1→2 step
was — and a source that overrides nothing declines to advise. **A partition
also states what a retained batch pins** (`Partitioning::retained_unit`): the
read chunk, or the partition itself where a decoded block already holds it. **`BlockCache::affordable` is that same per-reader number asked of one reader**, so the budget that affords block decode and the budget that admits a reader are one statement: the chunk and the decoder come off the top, since both are paid on the streaming path too, and what is left must hold two blocks — which puts the decline at ~27 MiB of block under the 64 MiB default for an 8 MiB-dictionary file, koji's 24 MiB still decoding. Two things read it — the partitioned replay's own cut, and the leader's
scheduler, which the mapping pass offers every open `COPY` region to
([`../design/architecture.md`](../design/architecture.md), "Execution model and
API surface"). A file with no more than one block is **warned about, never refused** (`DiagnosticKind::NonSeekableCompressedSource`, naming `xz -T0` and `--block-size=<size>`). The decoder is a frozen read-only vendored copy of `xz-seek` at `vendor/xz-seek/` (`CLAUDE.local.md`), not a published dependency, since this project is its first consumer and that consumption is what vets the interface. gzip/zstd are not read — `pg_dump -Fp --compress=…` output is unreadable today and is P15's (gzip) and P18's (zstd, lz4) ([`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)) ([`../design/architecture.md`](../design/architecture.md), "The compressed source") |
| Partitioned replay | **working, and `pgdq query` is its consumer** — `table_stream_partitions` runs the mapping pass once and hands back N `TableStream`s over the blocks it settled, each internally in file order and the groups themselves in file order, so **concatenating them is the serial stream**. That is a property of the code rather than a claim about it: both entry points are `map_for_query` (pass 1 whole) then `replay` (pass 2 over a segment list), and `table_stream` is `replay` over one segment per block. A piece's first row is the one starting past the first LF at or after its `start` and its last is the one *ending* at or past its `limit`, so a row straddling a cut belongs to the piece before it and **no cut has to land on a row boundary** — which is what lets `ByteRangeSource::partitions` advise cuts that know nothing about rows, this being its first consumer. The data range is what is cut, the first piece extended back over the header it takes its schema from; every later piece takes the header off the map's own `CopyBlock`. The resync is a real forward read rather than a scanner started one byte early, because I7's guarantee is about a *line start* and a row's own tail can be `\.`. Both of the caller's numbers bind and the bytes bind on the **sub-stream** count — `jobs` capped by `memory_bytes` over the largest footprint any matched block advised, plus the held batch's `max_source_span` **where the source retains by the read chunk**; a block-decoding source is charged the footprint alone, since the block a batch pins is the one already counted, and the plan note then reads `max_source_span: None` rather than a number that was never charged — and `Parallelism::Serial` is one sub-stream. A sub-stream's resume token is stamped with its partition, so feeding one to `table_stream` is `Error::ResumeQueryMismatch` rather than a silent superset. Nothing spawns: the sub-streams are `Send` and the caller runs them. **`pgdq query` is that caller**, and it prints file order out of them — one batch held per sub-stream, the earliest-starting one printed, only the drained slot refilled, so the reorder buffer is N × batch rather than an open-ended one. The key is `TableStream::batch_source_offset`, a fourth value a stream publishes about itself: `RowBatcher`'s existing span lower bound, which is the batch's **first row** rather than the end the resume token already carried, so the ordering does not rest on a batch ending where its scanner stood. A held batch carries its block's `NestedPlan`s, taken when it was taken; the comparison notes are announced off sub-stream 0, whose first segment is a `COPY` header. **A failure is recorded rather than raised**: index order is file order, so the merge keeps the lowest-indexed failure, marks every sub-stream at or after it dead — **discarding the batches they are holding**, which the first fill round gave them and whose rows are past the error — and goes on draining the ones before it, raising when the held batches run out. So the message is the serial path's message at every job count, where raising whichever sub-stream failed first in time would name a different row on each run ([`../design/architecture.md`](../design/architecture.md), "Partitioned replay" and "`pgdq query` merges the sub-streams back into file order") |
| The interior split | **working, and the mapping pass is its consumer** — `leader.rs` (L4) holds `scan_piece`, the body one fused worker runs over an LF-split piece of an open `COPY` block's interior, and `merge`, the fold performed over the pieces. A piece answers its row count, its own array-shape census, the `\.` terminator if it held one, the offset it consumed through, and how a piece continuing from there must enter — the fifth reported rather than inferred, since a resyncing piece that found no LF leaves that offset mid-row and nothing outside can tell that from a row start. `merge` sums and unions in file order and stops at the first terminator, answering `None` where the window did not reach one. Both are **pure and synchronous** — a `&[u8]` and an absolute base offset, not a source — because that is the body a `spawn_blocking` worker runs, decode and parse in one thread. The cut semantics are the replay's, not a second set: a piece's `limit` is not where reading stops but the line ending at the first LF at or after it, and a piece that does not begin on a row start resyncs by a real forward read, since I7 is about a **line start**. A piece past the block's end may report a terminator of its own, and it costs nothing — every piece before the true one is inside the block, so the earliest report is always real. `map::census_row` is the one fold both the serial mapping pass and a worker call, and it is the census-off binary's patch point. **`scan_region` is the scheduler over those two**: it asks the source where the region may be cut, hands out a window of `workers` partitions at a time through the same `stream::cut` and `stream::worker_count` the partitioned replay uses, and folds until a piece holds the terminator — answering the block's totals, a **decline** that leaves the region serial, or a **cancellation**. It reads `partitions()` for the shape of the cut and never for whether to make one; the one rule it applies is a floor derived from the source's own `partition_bytes()`, checked against what is left of the file, since the region's extent is what the scan is for. A worker is two reads — the piece exactly, which on a block-decoding source is a zero-copy slice of one block, then a **chunk-sized tail** scanned as an ordinary contiguous piece, which is why a partition's stated footprint is a block plus a chunk. Reaching EOF with no terminator is `Error::UnterminatedCopyBlock`, the leader's to raise because `scan_piece` never claims end of file. **A window is drained in file order** through a `futures::stream::FuturesOrdered` — polled concurrently exactly as `try_join_all` polls it, but answered in argument order — so the error raised is the earliest failing piece, after every piece before it has finished or failed, rather than whichever failure a race delivered first. **It is the one read loop that grants `WaitPolicy::MayWait`**, safe because a worker holds exactly one read at a time, and it restores `NeverWait` on the way out, being nested inside another loop rather than one of the three top-level ones. `a_split_interior_answers_what_the_serial_scanner_answers` cuts every `COPY` block of six fixtures ten ways and `a_scheduled_region_answers_what_the_serial_scanner_answers` asserts the same invisibility about the schedule, at eight jobs against four pool slots so the wait really blocks. **`stream::map_forward` is the leader**: its `CopyStart` arm offers each open region to `scan_region`, closes one it took through `close_copy_block` — the `CopyEnd` arm's body made a free function, so the splice/settled/save ordering is one copy and not two — folds the workers' census in through `Builder::absorb_census` (`&[ArrayShape]`, since `map.rs` is L1), and then resumes the serial scanner at `end_offset` with a **fresh** `ChunkCarry`, whatever it held having been the front edge of a chunk the workers read whole. A cancelled region banks nothing and is a resume point like any other. `a_parallel_mapping_pass_builds_the_index_a_serial_one_does` asserts the index against `build_index`'s over four fixtures at two chunk sizes and three job counts — 360 regions scheduled and 126 declined, so both paths are in it — and, since an equality proves nothing where every region was declined, `a_parallel_scan_grants_the_wait_inside_the_mapping_pass_and_takes_it_back` asserts from outside that the leader was reached, off the `MayWait` announcement only its scheduler makes ([`../design/architecture.md`](../design/architecture.md), "The interior split") |
| Caller-stated parallelism | **working end to end: the read path's memory budget, the replay's split and the leader's scheduler all read it, and the mapping pass calls the leader** — `Parallelism` is `Serial { memory_bytes: Option<u64> }` or `Workers { jobs, memory_bytes }`, mirroring `xz_seek::Bulk::new(workers, budget_bytes)`, with a budget-less `Serial` the library default and one worker spelled `Serial` rather than as a `Workers` of one, so `--jobs 1` is the serial path as a property of the value. **The collapse at one worker takes the count down and leaves the budget standing**: the two are independent numbers, so a stated `--parallel-memory` sizes every pool at the default `--jobs` and buys back a compressed file's block path with no second worker asked for beside it, and `None` is reserved for the caller that stated nothing — which is what the status line's `(default)` marker reports. It sits on `ScanOptions` and `QueryOptions` both, and each of the three read loops announces it to the source through a defaulted trait method of its own, `hint_parallelism` — the mapping pass and `scan` from `ScanOptions`, the replay from `QueryOptions`, so a query's two passes are bounded separately. **The stated bytes can be made a ceiling on what is outstanding and not only on what is idle**: `BufferPool::obtain` waits while `slots()` buffers taken under a granted wait are out, and whether a loop's reads may be blocked is a **defaulted method of its own**, `hint_wait_policy` — a **permission** rather than a description of the holder, since what the pool needs to know is whether it may block this loop. **All three top-level read loops state `WaitPolicy::NeverWait`**, which is also the `Default`: the replay loop cannot grant a wait, `RetainedChunks` pinning every chunk a batch has taken a view into before the batch goes to the caller, and `scan` and the mapping pass could but do not, the wait's own test driving a bare pool so a shipped loop's grant would buy exposure rather than coverage. **The one loop that grants it is the leader's fused worker**, which holds exactly one read at a time and restores `NeverWait` on the way out, and a `parse` or a query's mapping pass reaches it whenever the caller states a `Parallelism` over a region the source is willing to cut — which is a thing asked for, and what a flagless CLI resolves on a plain file is one worker. **A granted permission reaches a compressed source's chunk pool only**: the block pool's holder is the retention list rather than the loop, so `BlockCache::slot`'s drain frees no slot for a caller holding a view into what it evicts, and a grant there deadlocked. The charge rides on the buffer rather than on the pool's current policy, and is discharged whether or not the ceiling keeps the buffer. So the bound is two terms in both pools, and the block pool is the one where the library owns both: the chunk pool's are the waiting holders' slots and what in-flight batches pin, the block pool's are `slots` retained blocks and `stream::worker_count` live ones ([`../design/architecture.md`](../design/architecture.md), "Execution model and API surface"). The **bytes** size every pool: `LocalFileSource`'s one free list, and `XzSource`'s two, divided chunks-first so one stated number bounds the source rather than each pool; and they draw the block-decode line, `BLOCK_DECODE_MAX_BYTES`' flat 256 MiB having retired into `BlockCache::affordable`, which is what stops the pool's one-slot floor from making the stated number a fiction. The **jobs** half is the block pool's retention depth, one decoded block per would-be reader, floored at `POOL_DEPTH`, and the ceiling on how many sub-streams `pgdq query`'s replay is cut into; the chunk pool's depth is deliberately untouched by it. `DEFAULT_MEMORY_BUDGET` is 64 MiB — the serial path's number, under which every published figure was taken — so `xz -9 -T0`'s ~192 MiB blocks and `xz --block-size=128MiB` now fall back to the streaming reader where a flat cap took them down the block path over budget; the recourse is `--parallel-memory`, and the fallback **says so on a query**, above ([`../design/architecture.md`](../design/architecture.md), "Execution model and API surface" and "The compressed source"). The CLI states both, and **neither carries a constant default any more**: absent `--jobs` it asks the open source for a worker count (`LocalFileSource` answers one, `XzSource` the cores capped at the block count), and absent `--parallel-memory` it asks the source what one worker holds and fits that against the memory allocation it discovered — so a flagless plain `parse` is still serial and a flagless `.xz` one is not. Zero is refused for each. The budget half of that is `19.13`: a discovered limit less `MEMORY_RESERVE`'s 384 MiB, which `19.16` read off five builds. A stated `--jobs` is **what is asked for, not what is delivered** — two shapes admit no parallelism at all, and on a plain file the chunk pool's `POOL_DEPTH` binds first, so a `parse` above `--jobs 4` runs four workers and queues the rest, which the flag's help text and the manual both say ([`../design/architecture.md`](../design/architecture.md), "Execution model and API surface"; [`../manual/dump-inspection.md`](../manual/dump-inspection.md), "`--jobs` and `--parallel-memory`") |
| Remote input (`--source https://…`), over `object_store` | not started — P14, carved out of P6. `ByteRangeSource` is already shaped against `get_range`/`head`, and there is exactly one implementation: `LocalFileSource` |
| Python bindings, DataFusion `TableProvider` | not started — P6 |
| Device-bound scan performance campaign | **complete** — P7, single-threaded throughout and aimed at the row-extraction path; parallelism came later and is filed beside its own mechanisms. Twelve library changes on timed paths, four measured refusals, and the decomposition that is its durable half ([`../design/architecture.md`](../design/architecture.md), "Where a scan's time goes"). Warm on the 3.00 GiB control a typed `pgdq query` is 15.0× the `dd` floor where it was 31×, a `strings` one 10.9× where it was 13.3×, and a `parse` 1.43×; cold on the SATA SSD every scan shape is inside the device, and cold on NVMe the `COPY` path is 1.06× it. What it refused, and why, is beside each mechanism as a rejected alternative |
| Per-row-group column statistics, sparse row index | not started — **both are P10's**, the index included: the parallel-scan work found that splitting an open `COPY` block's interior at LF boundaries costs one row's resync, so known row boundaries buy it nothing `memchr` does not already give, and P10 owns the index and its interval outright. `CopyBlock::sparse_index` and `CopyBlock::column_stats` stay reserved `None`s |
| `--inserts` row reading; custom/directory/tar archive formats | not started — P8 (the map already locates and attributes `INSERT` runs) |

**Profiles are not figures.** `cd scripts && uv run measure.py
--profile-recipe` prints the sampling-profile sequence and runs none of it; a
profile is a `runs/` artifact with no median, no apparatus gate and no marker.
Some three dozen sit in `runs/` from the scan-performance campaign, over every
shape the recipe knows and both build configurations. **One profile input is
not a registered one**: reading the refused viewing builder's gate needed a
`List<Utf8View>` column and no committed input carries one, so that reading was
taken on a purpose-built `text[]` file whose specification is beside the
refusal ([`../design/architecture.md`](../design/architecture.md), "The
library's own per-row budget"); the generator lives in `runs/` and nothing
consumes its output. **What a scan spends its time on is
[`../design/architecture.md`](../design/architecture.md), "Where a scan's time
goes"**.

**Neither is a heap recording**, and `cd scripts && uv run measure.py
--heaptrack-recipe` is the second printed-never-run instrument recipe: the
`reserve` path step recorded either side of `reader_bytes` and read as a
`--diff`, which attributes at the `malloc` boundary and so sees `liblzma`
where the counting `#[global_allocator]` cannot
([`../design/measurements.md`](../design/measurements.md), "What an instrument
can see"). Nothing in `runs/` from it yet.

**Figures.** All sixteen sweep figures in
[`../design/measurements.md`](../design/measurements.md) come from the
`af15eac` sweep of 2026-09-05, folded in whole, each table carrying an
apparatus line and none carrying a partial-sitting note. `--check` reconciles
twenty-one markers against twenty-one figures, **and the register's boundary as
well**: the three sections the harness does not own — koji, the `cargo bench`
tripwires and the RSS attribution — each carry an
`<!-- outside-register: <id> -->` marker reconciled
against `measure.NOT_OURS` both ways and held to carrying no figure marker, so
the session stamp's "every figure below" now claims only what the register
holds. Each such section also declares **what invalidates it** and the commit
its readings were taken at, so `--stale` reports one whose inputs have moved in
a stanza of its own — koji reads red against `f5768e7` today, discharged by the
next koji run rather than by an acknowledgement, and `benches` declares no edge
because it publishes no number. `session-drift` is derived across that sweep and a second begun the
minute it finished on the same commit, which is the pair `--drift` reads;
`peak-rss` (`41c96bb`), `xz-decode-scaling` (`7d21c6e`), `parallel-peak-rss`
(`e29939c`) and `parallel-scan-throughput` (`20fd77c`) were each taken alone,
which their markers declare and `--check`
holds to descending from the stamp, standing in no borrow edge, and being
accounted for in the stamp's generated sentence.
`measure.ACKNOWLEDGED` carries the two commits of P7's wrap and keystone, both
comment-only against a declared path, plus `a30cc43` — reachability,
not comment-only, discharging `parallel-peak-rss` from the sub-stream divisor's
change rather than a declared path moving nothing; the previous six were spent by the
`af15eac` stamp and `--check` named them so they were deleted rather than kept
as sediment.

**The two parallel figures were built as apparatus and taken afterwards**,
which is what a figure published outside a stamped sweep costs: it names the
commit it was taken at inside its own marker, and a sitting run from a tree
carrying its own uncommitted apparatus has no such commit to name
([`../design/measurements.md`](../design/measurements.md), "A figure may be
published outside the sweep"). `parallel-peak-rss` stands at `e29939c`, the
commit that carried the apparatus and the block pool's deadlock fix both, so a
`--jobs` leg over an `.xz` measured the path that fix moved; `parallel-scan-throughput`
was re-taken alone at `20fd77c` once the sub-stream divisor landed, and the two
therefore no longer share a commit. `parallel-scan-throughput`'s `quoted_by`
names [`../design/architecture.md`](../design/architecture.md), "What
parallelism buys, and where it stops", which reads all four of its legs against
the rule the design argues from; `parallel-peak-rss`'s names
`STATUS.md` itself, for the claim that its `--jobs` axis goes flat past the
point the stated budget stops affording another worker — confirmed on the
128 MiB-block leg, which plateaus at ~2.11 GiB from eight jobs on while the
24 MiB-block leg is still climbing at twenty-four.

**`measure.UNTAKEN` is empty again, which is its healthy state.** Four entries
have left it so far: `projection-widths` and `xz-decode-scaling` earlier, then
`parallel-scan-throughput` and `parallel-peak-rss`, each waiting
only for a **commit to name** as its sitting, since a figure published outside a
stamped sweep declares one inside its own marker and a sweep is the thing none
of these four could be part of, deliberately occupying the quiet machine a sweep
needs.
Three earlier entries left by the list's two exits: `projection-widths` and
`xz-decode-scaling` were taken and moved into `FIGURES`, and
`composite-isolated` was deleted unpublished along
with its whole apparatus — the `--weak-composite` generator flag, the
`composite_text` input and the fidelity case pairing them — because the
projection table makes the same isolation a subtraction between two adjacent
rows of one file.

**The `INSERT`-run reading that moved ~10% between two stamps was code layout,
not code.** Cold 9.85 → 10.87 s and warm 8.37 → 9.19 on a byte-identical input;
`release` builds at the two commits retired the same instructions to 0.03% and
differed only in cycles, the whole difference sat inside the `scan_buf` of the
day whose 293 instructions were byte-identical between the binaries, and
forcing 64-byte function alignment collapsed the gap. There was nothing to
bisect to. What it changes is how a figure is read, which is now a standing
rule ([`../design/measurements.md`](../design/measurements.md), "Two builds of
one source can differ by layout"). The ratio was never in doubt at the time —
16.5× against 16.7× — and the `INSERT` statement scan took it to 4.3×, where the read path's own gains have since put it at 4.9×.

**The sweep's `control` warm floor sits 22.4% above the previous stamp's**,
which is over the ~15% a co-measured floor is judged against — and the sweep
stands, because the disqualifying signature is a slow move *shared* across the
warm floors and the `arrays` file's sits 6.9% below. Drift alone now reaches
that threshold, so the number no longer separates drift from contention on its
own and the shared-move conjunct is what does; the evidence and what it costs
are beside the rule
([`../design/measurements.md`](../design/measurements.md), "The floor is read
directionally").

**Every `<doc>.md`, "section" citation in the tree is resolved by `cd scripts &&
uv run citations.py`, and it is green: 552 citations in 247 files, none
dangling.** The nine the check first reported were repaired, each read for
what it meant — **seven of the nine were one keystone's sweep**, which repointed
a citation's *document* at `architecture.md` and left its *section* naming a
heading only the deleted doc ever had; the other two were an inbox that has
since been drained and a phase spec renaming its own section as it was written.
Nothing acknowledges or baselines a citation: the check fails rather than
reports, so a dangling one is repaired rather than lived with. A **dated entry
whose day has closed is not read** — an entry states what was true on its date,
so a keystone
that deletes a phase doc leaves its citations dangling by rule and repairing one
is not owed; today's entry is still read, since a typo is wrong the moment it is
written ([`history/README.md`](history/README.md), "An entry carries yesterday's
truth"). Its headings remain resolvable *targets*, which is what the
out-of-band ledger's `Why` column cites. The convention it enforces —
a section whose heading states a measured finding is addressed by an
`<!-- section: <id> -->` marker, and cited by that id rather than by the
heading — is beside the mechanism
([`../design/architecture.md`](../design/architecture.md), "Where a scan's time
goes").

## P19 progress

Spec: [`../design/roadmap-P19-efficient-defaults.md`](../design/roadmap-P19-efficient-defaults.md).
**The numbers after the evidence slices are allocation order, not schedule** —
the five orderings that bind are in the spec, not here. The sweep runs last; the
list below is numeric, so its first unticked box is not the next piece of work.
**`19.11`, the closing sweep, is all that remains** — the
new rows were admitted on 2026-09-12 and took the next free numbers
rather than being inserted ([2026-09-12](history/2026-09-12.md), "The reserve
entry closes on 384, and the grilling found the charge wrong below four
readers"; "A `parse` reports what was asked for and never what ran"; "The
margin applies the criterion twice, because `MEMORY_RESERVE` already contains
one").

**All three defaults are shipped, a `parse` says what it delivered and the
charge is repaired, so what remains is the sweep.** `19.18`'s sitting says the
program holds `4 MiB + 49.0 MiB a reader` and that a reader costs 98% of what
one more reader adds to the charge — the charge is right — while the resident
excess above the budget is **glibc arena retention**, and `19.16`'s arithmetic
puts that retention *inside* the per-reader charge, leaving a flat
83.6–214.6 MiB above it for the reserve to cover. `19.16` read the constant off
five builds and picked **384 MiB**, the smallest meeting the 20% margin, and
`19.13` ships it. The block pool's retention list is **no longer the reserve's
to cover**: `19.22` made a source's cost a shape rather than a scalar
(`io::WorkerMemory`), every budget is now solved against it, and `M93` restated
the block half of it as what the pool actually holds — one unit a reader plus
`(POOL_DEPTH.max(jobs) − 1)` shared, one unit less than the charge billed at
every count. `M94` is priced and closed the other way: the chunk pool's free
list is unbilled by 4 MiB at the shipped chunk, and since the repair needs a
count-independent term `WorkerMemory` cannot carry, it is registered as `KD24`
and left inside `MEMORY_UNPOOLED_BOUND`. Nor is the
margin at a large limit: `19.23` put the criterion itself in front of the
count, so what a limit resolves is the same on a host of any width — and since
`19.26` the prediction it refuses against is `at(n) + MEMORY_UNPOOLED_BOUND`,
**256 MiB** read off `19.16`'s grid under today's charge, so the criterion is
enforced once rather than twice and the margin is inert below a 640 MiB limit,
where the cap is already tighter. **The harness
checks that account rather than searching for it** —
`19.24` prints the charge against what was held at every flagless cell, with
the pool's retention list as its own named column and a criterion registered
before the sitting.

- [x] **19.1** `runtime-invariants.md` — the register (`RT1`–`RT7`), and
      `CLAUDE.md`'s read-trigger beside the Postgres one. No code. Notes:
      [`../design/roadmap-P19.1-runtime-invariants-notes.md`](../design/roadmap-P19.1-runtime-invariants-notes.md)
- [x] **19.2** The plain-path account, both instruments run and read: a plain
      `parse` at `--jobs ≥ 2` reads every byte twice, and `POOL_DEPTH` is
      refuted as the cause of the typed-`query` flatness. Notes:
      [`../design/roadmap-P19.2-plain-path-account-notes.md`](../design/roadmap-P19.2-plain-path-account-notes.md)
- [x] **19.3** The CLI runs a `current_thread` runtime, and `rt-multi-thread`
      is gone from the workspace. Notes:
      [`../design/roadmap-P19.3-current-thread-runtime-notes.md`](../design/roadmap-P19.3-current-thread-runtime-notes.md)
- [x] **19.4** `Serial` carries an optional budget, so the worker count's
      collapse at `--jobs 1` no longer takes the stated bytes down with it —
      `KD16` struck. Notes:
      [`../design/roadmap-P19.4-serial-budget-notes.md`](../design/roadmap-P19.4-serial-budget-notes.md)
- [x] **19.5** `Partitioning` states its retained unit, and a plain source's
      partition is eight read chunks capped at `POOL_MAX_BYTES` — `19.2`'s
      repair 1, folded in here. Notes:
      [`../design/roadmap-P19.5-retained-unit-notes.md`](../design/roadmap-P19.5-retained-unit-notes.md)
- [x] **19.6** The reserve figure registered and taken diagnostically
      (`--alone`, NOT PUBLISHABLE): a fixed term exists on the plain path and
      not on the compressed one. Notes:
      [`../design/roadmap-P19.6-reserve-figure-notes.md`](../design/roadmap-P19.6-reserve-figure-notes.md)
- [x] **19.7** `BufferPool`'s accounting alone — `held_bytes()` becomes a bound,
      the block pool's two lists share one slot count, and `affordable` wants
      room for two units. Notes:
      [`../design/roadmap-P19.7-pool-accounting-notes.md`](../design/roadmap-P19.7-pool-accounting-notes.md)
- [x] **19.8** The source's own worker default; `DEFAULT_JOBS` removed. A
      stated `--jobs` wins outright in both directions. Notes:
      [`../design/roadmap-P19.8-source-worker-default-notes.md`](../design/roadmap-P19.8-source-worker-default-notes.md)
- [x] **19.9** The resolution is tested and a run says what it resolved: the
      CLI's **mode report**, on every scanning command, with both numbers
      carrying their provenance. Notes:
      [`../design/roadmap-P19.9-resolution-report-notes.md`](../design/roadmap-P19.9-resolution-report-notes.md)
- [x] **19.10** The manual and both flags' help text, on the correction that a
      **discovered limit is a ceiling, not the budget**. Notes:
      [`../design/roadmap-P19.10-manual-notes.md`](../design/roadmap-P19.10-manual-notes.md)
- [ ] **19.11** The closing sweep — publishes the reserve figure and
      `rss-attribution`, closing `M74`, and re-takes both `parallel-*` figures,
      which were taken before `partition_bytes` changed. **It does not start
      until a `--figure reserve --alone` sitting completes with no killed leg**,
      so it is behind `19.19` and `19.17.1`, not merely behind `19.13`. **Nothing
      blocks it now**: `M88` landed, so every flagless cell names the path it
      ran, `M89` with it, so the acceptance evaluates a cell that bills the
      pool's own term rather than none at all, and `M90`, which puts
      the three-count guard at the model's own boundary and publishes a secant
      below it, so a sitting that loses legs to kills states a slope rather than
      an intercept nothing checks
      ([`../design/out-of-band.md`](../design/out-of-band.md)); `M91` split the
      verdict into bands, and `M92` says which of them the gate releases —
      **`bound` alone**, so a cell between `MEMORY_UNPOOLED_BOUND` and
      `MEMORY_RESERVE` publishes with its finding while an over-bill bars, and a
      band nobody has argued out bars by default. **`M93` closed the last of it,
      and the gate sitting is what found it.** That sitting ran on 2026-09-12
      (`runs/measure-20260912T170937/tables.md`, NOT PUBLISHABLE): no leg was
      killed — the whole axis survived its own allocation, which is the half of
      the gate `19.18` failed — but the charge over-billed by one block unit at
      every count, 130.0 MiB billed against 111.1 held at 24 MiB blocks and 650.0
      against 526.9 at 128, which is the `over-bill` band and not the `bound` one.
      `M93` restated the block term as what the pool holds; one unit off every
      cell's bill lands the whole criterion met with the worst remainder at
      153.4 MiB against a 256 MiB bound, so no constant moved
      ([`../design/out-of-band.md`](../design/out-of-band.md);
      [2026-09-12](history/2026-09-12.md), "The charge over-bills the pool floor
      at every count"). **The gate has to be re-run on the repaired build** —
      the readings above were taken under the old charge, so the sitting that
      releases this box is a fresh `--figure reserve --alone`. **`M95` has
      landed, so no row is in the way**: the charge is piecewise linear with a
      kink at `POOL_DEPTH`, and the fit is now over the remainder that is left
      once the pool's retention list — known before the sitting — comes off each
      ordinate, where one straight line across the bend put ≈421 MiB of it into
      the 128 MiB family's intercept and read its slope 31% low. The gate itself
      was never affected, being per cell; what the repair restores is what the
      sitting *publishes*, `19.18`'s `98% of reader_bytes` comparison included
      ([`../design/out-of-band.md`](../design/out-of-band.md);
      [2026-09-12](history/2026-09-12.md), "The fit is a straight line across a
      kink the charge now states"). **`M96` has landed and `M94` is priced**:
      the billed-against-held pass
      enumerated every
      pool, buffer and retained structure on all three read paths
      ([`../design/architecture.md`](../design/architecture.md), "Billed against
      held") and turned up three further discrepancies — `M97`, an 8× over-bill
      on the plain path, `M98`, a query-path under-bill of every block a batch's
      span pins past the first, and `M99`, the seek table held twice — **none of
      which this sitting can reach**, its legs being `pgdq parse` over a
      compressed input ([2026-09-12](history/2026-09-12.md), "Billed against
      held, in one pass"). `M94` it *does* reach, and the answer is that it
      stays unbilled: the chunk pool's free list is 4 MiB at the shipped chunk,
      flat in the count, and billing a count-independent term needs a third one
      in `WorkerMemory` — so it is a register entry and the sitting's unnamed
      remainder carries it ([2026-09-12](history/2026-09-12.md), "The chunk
      pool's floor is priced, and 4 MiB does not buy a third term"). What this slice then owes is unchanged: the
      sweep itself,
      `reserve`'s `Shared` edge onto `peak-rss` with the `Session.borrow` change
      that lets an RSS reading cross a share, `rss-attribution` published and
      `M74` closed, and both `parallel-*` figures re-taken.
      **The gate sitting at `88cf781` completed and does not release this box,
      for an apparatus reason rather than a library one**
      (`runs/measure-20260912T204517/tables.md`, NOT PUBLISHABLE). No leg was
      killed, and both one-reader cells came back `over-bill` — 24 MiB blocks at
      `-m 512m` and 128 MiB at `-m 1g`, each over-billed by the whole of its
      bill against a 15 MiB resident set. The cause is that
      `target/release/pgdq` is timed and never rebuilt: the copy in the tree was
      built before `M93`, `M94` and `M95` landed, so the sitting ran the charge
      `M93` replaced against `measure.py`'s repaired mirror of it. It is settled
      by arithmetic and not by a second sitting — all eight resolved budgets are
      the pre-`M93` `WorkerMemory` exactly, and the resolved counts are
      byte-identical to `880d09e`'s where `M93` had to move them. `M93`'s
      prediction is therefore untested rather than refuted, and no constant
      moves ([2026-09-12](history/2026-09-12.md), "The gate sitting timed a
      binary three commits stale, and its verdict is about a library nobody
      ran"). The hole is admitted as **`M101`**, blocking this phase: the
      harness guards the census-off binary's provenance and not the binary every
      figure is timed against ([`../design/out-of-band.md`](../design/out-of-band.md)).
      **The gate passed on a hand-built `88cf781` binary**
      (`runs/measure-20260912T211806/tables.md`, NOT PUBLISHABLE): no leg was
      killed and all ten evaluated cells read `met`, the worst unnamed remainder
      155.9 MiB against a 256 MiB `MEMORY_UNPOOLED_BOUND` — `M93`'s predicted
      153.4. The binary check it stands on is satisfied: the resolved counts
      moved to 1/2/10/11/17/24 and 1/1/1/1/4/5, matching the instrument legs the
      harness builds for itself, where the stale sitting's were byte-identical
      to `880d09e`'s. **`M101` has landed, so nothing is in the way**: the harness
      builds `target/release/pgdq` in the first second of every sitting rather
      than timing whatever the last build left there, which is what let that
      gate run three commits stale — a diagnostic sitting's own budgets say
      which binary ran, where most of the twenty-one published tables carry no
      such fingerprint ([`../design/out-of-band.md`](../design/out-of-band.md);
      [`../design/measurements.md`](../design/measurements.md), "The apparatus").
      **What is left of this box is the sweep.**
- [x] **19.12** The reserve re-taken diagnostically against `19.7`'s build
      (`--alone`, NOT PUBLISHABLE): `19.7`'s prediction is refuted, and what the
      sitting found is a divisor under-charging a sub-stream. Notes:
      [`../design/roadmap-P19.12-reserve-retake-notes.md`](../design/roadmap-P19.12-reserve-retake-notes.md)
- [x] **19.13** `discover_memory_limit`, `Parallelism::discover` and the budget
      rule, shipping the **384 MiB** `MEMORY_RESERVE` `19.16` chose, the
      source's own budget recommendation, and the decline's report. Notes:
      [`../design/roadmap-P19.13-budget-discovery-notes.md`](../design/roadmap-P19.13-budget-discovery-notes.md)
- [x] **19.14** A compressed sub-stream is charged what one reader holds,
      through the one `reader_bytes` the charge and the decline both call.
      Notes:
      [`../design/roadmap-P19.14-reader-charge-notes.md`](../design/roadmap-P19.14-reader-charge-notes.md)
- [x] **19.15** The budget rule run in real cgroups, and **the gate does not
      pass** — 0.6% of the limit left at the worst rep, and the fixed term
      rather than the per-reader one is what exceeds the reserve. A `runs/`
      probe, not a figure. Notes:
      [`../design/roadmap-P19.15-budget-probe-notes.md`](../design/roadmap-P19.15-budget-probe-notes.md)
- [x] **19.16** The reserve constant, from a reading rather than a fit — **384
      MiB**, over five builds and 400 runs, reviewed on 2026-09-12 and standing.
      A `runs/` probe, not a figure. Notes:
      [`../design/roadmap-P19.16-reserve-constant-notes.md`](../design/roadmap-P19.16-reserve-constant-notes.md)
- [x] **19.17** The compressed account's instrument, no library code: `reserve`
      becomes four families, the flagless one reading its resolved count off the
      run's own `scan started` line. Notes:
      [`../design/roadmap-P19.17-compressed-instrument-notes.md`](../design/roadmap-P19.17-compressed-instrument-notes.md)
- [x] **19.18** The compressed path's resident account (`--alone`, NOT
      PUBLISHABLE): the charge is right to 2%, and the excess above the budget
      is **glibc arena retention**. Owns nothing. Notes:
      [`../design/roadmap-P19.18-compressed-account-notes.md`](../design/roadmap-P19.18-compressed-account-notes.md)
- [x] **19.17.1** A killed leg is recorded and the sitting continues; a killed
      leg bars publication. An **earned third level** — `19.17` shipped the
      wrong contract. Notes:
      [`../design/roadmap-P19.17.1-killed-leg-notes.md`](../design/roadmap-P19.17.1-killed-leg-notes.md)
- [x] **19.19** The per-file term, billed where the file is open — `KD19`
      struck, the cut size separated from the memory charge, and the tail read's
      duplicate block decode named as `KD20`. Notes:
      [`../design/roadmap-P19.19-per-file-term-notes.md`](../design/roadmap-P19.19-per-file-term-notes.md)
- [x] **19.20** The cut width, decided by measurement — **one unit stays**, the
      read shape becomes the source's own statement, `KD18` is struck, and
      `KD20` now knows the width is not its fix. Notes:
      [`../design/roadmap-P19.20-cut-width-notes.md`](../design/roadmap-P19.20-cut-width-notes.md)
- [x] **19.21** The introspection the compressed account needs: `introspect`, a
      third off-by-default feature, whose build `measure.binary_allocator`
      refuses — so "never timed" is mechanical. Notes:
      [`../design/roadmap-P19.21-introspection-notes.md`](../design/roadmap-P19.21-introspection-notes.md)
- [x] **19.22** The charge bills the pool floor: a source's cost is an
      `io::WorkerMemory` — a per-worker term and a shared floor — and
      `Parallelism::fit` and `stream::worker_count` both solve against it
      instead of dividing. The decline widens, which is the finding. Notes:
      [`../design/roadmap-P19.22-pool-floor-notes.md`](../design/roadmap-P19.22-pool-floor-notes.md)
- [x] **19.23** The count answers to the criterion, not to the core count:
      `MEMORY_MARGIN_PERCENT`, and a `fit` that refuses a count whose predicted
      resident leaves under a fifth of the limit. Notes:
      [`../design/roadmap-P19.23-count-margin-notes.md`](../design/roadmap-P19.23-count-margin-notes.md)
- [x] **19.24** The harness checks the model rather than searching for a
      constant: `charge_model` and a two-sided criterion registered before the
      sitting, the pool floor as its own named column, and the `reader_bytes`
      mirror's pre-`19.19` claim corrected. Notes:
      [`../design/roadmap-P19.24-charge-model-check-notes.md`](../design/roadmap-P19.24-charge-model-check-notes.md)
- [x] **19.25** A `parse` says what ran, not what was asked for: `scan
      arrangement`, once per scan, naming the delivered count beside the
      announced one and what would buy the refused arrangement back. Notes:
      [`../design/roadmap-P19.25-delivered-count-notes.md`](../design/roadmap-P19.25-delivered-count-notes.md)
- [x] **19.26** The margin predicts with its own constant:
      `io::MEMORY_UNPOOLED_BOUND` is **256 MiB**, derived by arithmetic over
      `19.16`'s 400 runs under today's charge, and the harness faults at both
      constants. The flagless axis moved — 9/10/16/23 readers where it had read
      7/8/14/21, since moved again by `M93`'s repair — and the two one-reader
      legs did not. Notes:
      [`../design/roadmap-P19.26-margin-constant-notes.md`](../design/roadmap-P19.26-margin-constant-notes.md)

## Not started

- **A CLI-feedback pass** — the `pgdq info` / `--map` output shape is accepted
  as provisional pending real user trials; resulting changes land as
  out-of-band items. Nothing is pooled here at present.
- **P19 is open and all three defaults it is named for are now set** — the
  worker default comes from the source, and the memory-limit discovery and the
  budget rule are in the tree carrying the 384 MiB reserve `19.16` read and the
  20% margin `19.23` put in front of the count, and `19.25` made a `parse`
  report the count it delivers rather than the one it was asked for. What
  remains is the closing sweep, whose flagless axis `19.26` moved. Its
  checklist is above and its spec is
  [`../design/roadmap-P19-efficient-defaults.md`](../design/roadmap-P19-efficient-defaults.md).
  Six phases remain sketched — P10, P14, P6, P15, P18, P8, in the roadmap
  table's schedule order; a `P<k>` is an identifier, so the numbers say nothing
  about the order they run in. Each gets its own full grilling when it becomes
  current, and every one that carries an inbox must have it drained as part of
  that grilling.

## Known deficiencies

The deficiency register. Every known deficiency carries a stable `KD<k>`,
allocated on discovery and never reused, and **one line here**: what it costs,
its stance, and the file whose paragraph holds the rest. That paragraph sits
beside the mechanism, where `CLAUDE.md`'s read-triggers already send a session
that is about to touch it. This is an index, not the document.

Three stances, because these are not one kind of thing and the difference
decides whether anyone should act. **(a)** a consequence of a deliberate
tradeoff, never to be worked. **(b)** a defect with a known fix and a named
destination. **(c)** a defect with a known fix and no owner — a legitimate
resting state, said in those words, naming whatever would promote it. A
limitation whose remedy the user already has today is not here at all: it is a
property of how the system works, and it lives beside its mechanism with no
identifier.

A coverage statement is not a deficiency:
[`../design/pg-dump-compatibility.md`](../design/pg-dump-compatibility.md)'s
`Unsupported` and `Untested` rows are scope and evidence, and earn a `KD<k>`
only by naming one.

An entry is struck by the change that closes its last part, not at a phase
boundary, and a part closing into a *property* migrates beside its mechanism
rather than being deleted. <!-- deficiency-watermark: KD26 -->
**`KD1`–`KD26` are allocated, and nothing at or below `KD26` is reused** — a
number the index below does not carry is a struck entry, not a typo. That
watermark is what keeps a `KD<k>` in an old commit message resolvable, and the
marker beside it is what a citation resolves against; the names of the struck
entries went at the keystone, `git log` being what answers *when*.

Where a `(b)` entry's owning phase has been sliced, the entry names the slice
and the slice names the entry, so landing one re-reads the other and a re-slice
is obliged to re-target. The two directions are asymmetric: a checklist line is
a **record**, so its `KD<k>` is a citation that may name a struck entry and may
not name a number nobody allocated; an entry is **present tense**, so it may
name only live slices, and naming a ticked one is an error — closing a part
rewrites the entry in the change that ticks the box. A `(b)` stance also needs
its destination to exist: an entry owned by a phase the roadmap's index calls
`Complete` or `Struck`, or does not list, drops to `(c) unowned` unless a phase
actually absorbs it.

`cd scripts && uv run deficiencies.py` reconciles this index against those
paragraphs, against the source-code markers, against the slice checklist above
and against the roadmap's phase index, and fails on any of them. An entry owned
by a phase with no checklist yet names no slice and is not asked to. That last
read is pinned at both ends: a phase carrying a checklist is `Current` in the
index and a `Current` phase carries one, so a wrap that dropped the checklist
and left the state, or a slicing that wrote the checklist and left it, fails
here rather than reading as a phase nobody has sliced.

- **KD1** — a `--disable-triggers` dump loses TOC attribution on every data
  span, `COPY` and `INSERT` alike (I31), costing the coverage diagnostic and
  `Span::toc`. **(c) unowned**; promoted by a dump in hand whose data spans
  need attribution. Detail:
  [`../design/architecture.md`](../design/architecture.md), "TOC enrichment".

- **KD2** — an array nested inside a composite is decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode`. **(c) unowned**; promoted by a schema that holds one,
  the per-path census being deferred on frequency. Detail:
  [`../design/architecture.md`](../design/architecture.md), "What the census
  decides, and who may believe it".

- **KD3** — two array shapes come back as text with no way to ask for more,
  `NestedArrayElement` and `VaryingArrayShape`, though both are fully
  understood. **(c) unowned**; promoted by a caller whose arrays are matrices
  or scientific data, for whom a string is the wrong answer. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Joining a header
  against the metadata".

- **KD4** — a type name that needs quoting resolves `Unknown` (I29): a weaker
  type, never a wrong one. **(c) unowned**; promoted by a dump whose type names
  are not ordinary identifiers, which neither any fixture nor koji is. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Type resolution".

- **KD5** — a map rebuild is still a whole-list clone, so mapping is
  O(blocks²) wherever the save throttle's gate does not close it — which is
  every `--dqcache none` scan, since a no-op save leaves nothing to amortize:
  19.1 s for 4000 blocks. **(c) unowned**; promoted by a dump with thousands of
  blocks scanned under `--dqcache none`. Detail:
  [`../design/architecture.md`](../design/architecture.md), "`parse` resumes,
  and saves as it goes".

- **KD6** — a conflicting table past a query's stopping point is never seen, so
  `Error::AmbiguousTable` is not raised for it and the query returns the
  candidate it found. **(b) owned by P6**, where what the embedded API promises
  is decided. Detail:
  [`../design/architecture.md`](../design/architecture.md), "One target per
  query".

- **KD7** — a column that *states* a collation this build does not implement is
  compared bytewise, so the row set is not the server's: under `<`/`>` always,
  and under `=`/`!=` where the dump declares it `deterministic = false` (I42);
  the fix is a comparison per named collation, up to a provider version.
  **(c) unowned**; promoted by [`../design/roadmap.md`](../design/roadmap.md)'s
  Future item "collation-aware comparison", intent without a phase. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Ordering operators
  compare typed".

- **KD8** — a typed column cannot hold `infinity`, `-infinity` or `NaN`, nor —
  on an `interval` — a time part past `2562047:47:16.854775807`, so
  materializing one raises `Error::FieldDecode` and there is no typed way to
  read the value. **(c) unowned**; promoted by whichever phase takes typed
  materialization, which is where the choice between a null, a sentinel and the
  error belongs. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Decoders and
  render-back".

- **KD9** — an `INSERT` run costs **4.9×** a `COPY` scan's per-byte CPU warm
  and **2.62×** the device's own time cold on NVMe against 1.06×, and two cuts
  against that remainder are known and untaken. **(b) owned by P8**, whose
  Track A row reader extends the very scan both cuts are in; the cold-NVMe
  figure confirmed the entry where it might have retired it. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Bulk regions: one
  span kind, three payloads".

- **KD10** — a column whose declared type this build models no comparison for
  answers `=`/`!=` bytewise, which is not the server's answer for the geometric
  types (`box_eq` compares areas), so the row set is wrong; ordering is refused
  outright, and the announcement misses a type reached through a container
  (`box[]`). **(c) unowned**; promoted by a dump whose queried columns are
  geometric or hold a `money`-shaped extension type. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Equality is typed
  too".

- **KD13** — `money` is below the ADBC floor: the driver answers `int64` and we
  answer `Utf8View`, because `cash_out` renders through the monetary locale and
  `pg_dump` sets `lc_monetary` nowhere, so the file cannot say which locale
  wrote a value. **(a) deliberate tradeoff** — closing it means guessing a
  locale or asking for one, which the bar refuses for every other type. Detail:
  [`../design/architecture.md`](../design/architecture.md), "The floor: the ADBC
  driver's answer bounds ours".

- **KD17** — a plain typed `query` is flat at 0.98× across the whole `--jobs`
  axis: the sub-streams it plans never run concurrently, total CPU staying under
  one core. The named suspect — `POOL_DEPTH` clamping the chunk pool — is
  refuted by a build that lifts it, so what serializes them is unidentified.
  **(c) unowned**; promoted by a phase that takes up plain-source extraction
  throughput, since no defaults change reaches it. Detail:
  [`../design/architecture.md`](../design/architecture.md), "What parallelism
  buys, and where it stops".

- **KD20** — a block-decoding worker decodes its **successor's block as well as
  its own**, nothing sharing the two, so a parallel compressed scan does about
  twice the decode work and its speedup is capped near half the reader count.
  **(c) unowned**; promoted by a phase taking up compressed scan throughput,
  and the fix left is an in-flight map, a wider cut having been measured and
  refused. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Execution model and
  API surface".

- **KD21** — the block pool's slot ceiling follows the worker count a caller
  *announced* rather than the one delivered, so a `--jobs` well above what
  `--parallel-memory` affords holds more than the stated budget — 1,280 MiB
  against 1,024 at `--jobs 24 --parallel-memory 1g` on a 128 MiB-block file.
  **(c) unowned**; promoted by a phase reworking the block pool's sizing rule.
  Detail: [`../design/architecture.md`](../design/architecture.md), "Execution
  model and API surface".

- **KD22** — the leader cuts a window of `workers × partition_bytes` from a
  `COPY` block's start and drains every piece before merging, so a block far
  smaller than that window is found by reading and parsing the whole window and
  discarding all of it: **149× the bytes of a serial scan at `--jobs 4` and
  268× at `--jobs 8`**, on a 57.6 MiB dump of 2,000 small blocks, 34× the wall
  time. Reached with no flag typed on a compressed dump, `XzSource` recommending
  one worker per core. **(c) unowned**; promoted by a phase taking up leader
  scheduling. Detail:
  [`../design/architecture.md`](../design/architecture.md), "The interior
  split".

- **KD23** — a `pgdq query` sub-stream can pin several decoded blocks where
  the budget bills one: a query partition is cut over a whole `CopyBlock` rather
  than through the leader's window, so `BOUNDARIED_PARTITION_UNITS` does not
  bound it and a held batch's `max_source_span` reaches up to four of koji's
  24 MiB blocks. **(c) unowned**; promoted by a phase that takes up query-path
  memory, the repair reversing a recorded decision either way. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Billed against
  held: one row per buffer the process keeps".

- **KD24** — the chunk pool's free list is billed nowhere, so a compressed
  source's charge is short by `⌊budget/chunk⌋.clamp(1, POOL_DEPTH)` chunks —
  4 MiB at the shipped chunk, 64 MiB against 16 billed at `--chunk-size 16m`,
  flat in the count and never above the stated budget. **(c) unowned**;
  promoted by a caller announcing a large chunk, or by a phase reworking
  `WorkerMemory`, which has no count-independent term to bill it with. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Billed against
  held: one row per buffer the process keeps".

- **KD25** — the plain source bills `PLAIN_PARTITION_CHUNKS × chunk` a reader
  where the path holds `POOL_DEPTH` chunks flat, and recommends no count at
  all, so plain readers are bounded by a charge describing nothing held — 8 MiB
  billed against 4 held at the shipped chunk, and unbounded above a budget of
  `8 MiB × jobs`. **(c) unowned**; promoted by a reading showing plain parallel
  beating serial, which is that path's own reopening condition. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Billed against
  held: one row per buffer the process keeps".

- **KD26** — a compressed source holds its seek table twice and no charge bills
  either, the only unbilled term in the account that grows with the file rather
  than the count. **(c) unowned**; the duplication half closes at `M102`, the
  re-sync taking `xz_seek`'s shared-table accessor, and what is left is the
  billing half, which stands on an ordering rather than a size. Detail:
  [`../design/architecture.md`](../design/architecture.md), "Billed against
  held: one row per buffer the process keeps".

- **KD14** — peak resident set is flat in dump bytes but grows ~9.9 KB per
  table, three fifths of it live structure the preamble alone pays, so a
  4,000-table `parse` holds **43.8 MiB** against a one-block one's 5.9 MiB.
  **(c) unowned**; promoted by a dump with tens of thousands of tables, nothing
  in hand being one. Detail:
  [`../design/architecture.md`](../design/architecture.md), "`parse` resumes,
  and saves as it goes".

## Decisions worth another look

Calls made without the maintainer present that a person should still weigh in
on — cautionary and informational, never blocking. **Capped at five.** Closing
an entry is filing it and then deleting it, done by the session that hears the
answer; where the review affirms a call and changes nothing, its reasoning goes
beside the mechanism it governs first. Full rules:
[`../process.md`](../process.md), "Decisions worth another look".

*Nothing open.*
