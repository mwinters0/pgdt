# Known deficiencies

The deficiency register, beside [`STATUS.md`](STATUS.md) and read with it.
Every known deficiency carries a stable `KD<k>`, allocated on discovery and
never reused, and **one line here**: what it costs, its stance, and the `.rs`
file carrying its `deficiency: KD<k>` marker. **The comment at that marker is
the detail** — it sits on the mechanism, so the session editing the mechanism
reads it without being sent anywhere. This is an index, not the document.

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
rather than being deleted. <!-- deficiency-watermark: KD81 -->
**`KD1`–`KD81` are allocated, and nothing at or below `KD81` is reused** — a
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

`cd scripts && uv run deficiencies.py` reconciles this index against the code
markers — one apiece, in the file the entry names, and none anywhere under
`docs/` — against [`STATUS.md`](STATUS.md)'s slice checklist and against the
roadmap's phase index, and fails on any of them. An entry owned by a phase
with no checklist yet names no slice and is not asked to. That last read is
pinned at both ends: a phase carrying a checklist is `Current` in the index and
a `Current` phase carries one, so a wrap that dropped the checklist and left
the state, or a slicing that wrote the checklist and left it, fails here rather
than reading as a phase nobody has sliced.

- **KD1** — a `--disable-triggers` dump loses TOC attribution on every data
  span, `COPY` and `INSERT` alike (I31), costing the coverage diagnostic and
  `Span::toc`. **(c) unowned**; promoted by a dump in hand whose data spans
  need attribution. Detail: `pgdump_query/src/map.rs`.

- **KD2** — an array nested inside a composite is decided optimistically, so a
  multi-dimensional or `[lb:ub]=`-decorated value there is a hard
  `Error::FieldDecode`. **(c) unowned**; promoted by a schema that holds one,
  the per-path census being deferred on frequency. Detail:
  `pgdump_query/src/resolve.rs`.

- **KD3** — two array shapes come back as text with no way to ask for more,
  `NestedArrayElement` and `VaryingArrayShape`, though both are fully
  understood. **(c) unowned**; promoted by a caller whose arrays are matrices
  or scientific data, for whom a string is the wrong answer. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD5** — a map rebuild is still a whole-list clone, so mapping is O(blocks²)
  wherever the save throttle's gate does not close it — which is every
  `--dtcache none` scan, since a no-op save leaves nothing to amortize.
  **(c) unowned**; promoted by a dump with thousands of blocks
  scanned under `--dtcache none`. Detail: `pgdump_query/src/stream.rs`.

- **KD6** — a conflicting table past a query's stopping point is never seen, so
  `Error::AmbiguousTable` is not raised for it and the query returns the
  candidate it found. The DataFusion provider never shows it, reading only a
  complete map. **(c) unowned**; promoted by a concatenated or
  `pg_dumpall`-style file reaching a user through a cold query. Detail:
  `pgdump_query/src/batch.rs`.

- **KD48** — a file's `SET search_path` is not read, so an unqualified name
  resolves as under the default path: a collation a declared one could shadow
  gets the weaker verdict, an unqualified user type resolves `Unknown`, and a
  built-in's name keeps the built-in (D37) — shadowing either takes a path
  naming `pg_catalog` after the declaring schema, which only a hand-written
  file sets (I8). **(c) unowned**; promoted by such a
  file in hand, the fix being to model the path. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD7** — a column that *states* a collation this build does not implement is
  compared bytewise, so the row set is not the server's: under `<`/`>` always,
  and under `=`/`!=` where the dump declares it `deterministic = false` (I42);
  the fix is a comparison per named collation, up to a provider version. **(c)
  unowned**; promoted by [`../design/roadmap.md`](../design/roadmap.md)'s
  Future item "collation-aware comparison", intent without a phase. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD9** — an `INSERT` run costs several times a `COPY` scan's per-byte CPU
  warm and most of a cold NVMe scan's time (`measurements.md`,
  `scan-throughput-warm` and `scan-throughput-nvme`), and three cuts
  against that remainder are known and untaken. **(b) owned by P8**, whose
  Track A row reader extends the very scan all three are in; the cold-NVMe
  figure confirmed the entry where it might have retired it. Detail:
  `pgdump_query/src/preamble.rs`.

- **KD10** — a column whose declared type this build models no comparison for
  answers `=`/`!=` bytewise, which is not the server's answer for the geometric
  types (`box_eq` compares areas), so the row set is wrong; ordering is refused
  outright, and the announcement misses a type reached through a container
  (`box[]`). **(c) unowned**; promoted by a dump whose queried columns are
  geometric or hold a `money`-shaped extension type. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD13** — `money` is below the ADBC floor: the driver answers `int64` and we
  answer `Utf8View`, because `cash_out` renders through the monetary locale and
  `pg_dump` sets `lc_monetary` nowhere, so the file cannot say which locale
  wrote a value. **(a) deliberate tradeoff** — closing it means guessing a
  locale or asking for one, which the bar refuses for every other type. Detail:
  `pgdump_query/src/pgtype.rs`.

- **KD57** — `pgdt query` prints in file order holding one batch a
  sub-stream, so past its first round only the earliest live sub-stream reads
  and `--jobs` buys it no throughput, the later sub-streams' first-round work
  spent as CPU with no return. **(c) unowned**; promoted by a phase taking up
  `pgdt query`'s throughput, the fix a choice between interleaving each
  sub-stream's runs, a reorder buffer under the budget, and unordered output.
  Detail: `pgdt/src/main.rs`.

- **KD20** — a block-decoding worker decodes its **successor's block as well as
  its own**, nothing sharing the two, so a parallel compressed scan does about
  twice the decode work and its speedup is capped near half the reader count.
  **(c) unowned**; promoted by a phase taking up compressed scan throughput,
  and the fix left is an in-flight map, a wider cut having been measured and
  refused. Detail: `pgdump_query/src/io.rs`.

- **KD21** — the block pool's slot ceiling follows the worker count a caller
  *announced* rather than the one delivered, so a `--jobs` well above what the
  read-buffer budget affords holds more than that budget — 1,280 MiB against
  1,024 at `--jobs 24` and a 1 GiB budget on a 128 MiB-block file.
  **(c) unowned**; promoted by a phase reworking the block pool's sizing rule.
  Detail: `pgdump_query/src/io.rs`.

- **KD22** — the leader cuts a window of `workers × partition_bytes` from a
  `COPY` block's start and drains every piece before merging, so a dump of
  blocks much smaller than that window is read and parsed two orders of
  magnitude over, worse at every worker added and reached with no flag typed.
  **(c) unowned**; promoted by a phase taking up leader scheduling. Detail:
  `pgdump_query/src/leader.rs`.

- **KD23** — a `pgdt query` sub-stream can pin several decoded blocks where the
  budget bills one: a query partition is cut over a whole `CopyBlock` rather
  than through the leader's window, so `BOUNDARIED_PARTITION_UNITS` does not
  bound it and a held batch's `max_source_span` reaches up to four of koji's 24
  MiB blocks. **(c) unowned**; promoted by a phase that takes up query-path
  memory, the repair reversing a recorded decision either way. Detail:
  `pgdump_query/src/stream.rs`.

- **KD24** — the chunk pool's free list is billed nowhere, so a compressed
  source's charge is short by `⌊budget/chunk⌋.clamp(1, POOL_DEPTH)` chunks — 4
  MiB at the shipped chunk, 64 MiB against 16 billed at `--chunk-size 16m`,
  flat in the count and never above the stated budget unless one chunk is. **(c) unowned**;
  promoted by a caller announcing a large chunk, or by a phase reworking
  `WorkerMemory`, which has no count-independent term to bill it with. Detail:
  `pgdump_query/src/io.rs`.

- **KD25** — the plain source bills `PLAIN_PARTITION_CHUNKS` chunks a reader,
  capped at `POOL_MAX_BYTES`, where the path holds `POOL_DEPTH` chunks flat, and recommends no per-reader
  memory at all, so plain readers are bounded by a charge describing nothing held — 8 MiB
  billed against 4 held at the shipped chunk, and unbounded above a budget of
  `8 MiB × jobs`. **(c) unowned**; promoted by a reading of a parallel plain
  scan on a real device, which is that path's own reopening condition. Detail:
  `pgdump_query/src/io.rs`.

- **KD26** — no charge bills a compressed source's seek table, the only
  unbilled term in the account that grows with the file rather than the count —
  3.33 MiB on koji's download against 4.11 KiB on the fixtures the bound was
  read off. **(c) unowned**; promoted by a source whose index is not small
  beside `MEMORY_UNPOOLED_BOUND`, or by a phase reworking `WorkerMemory`, which
  has no per-source term to bill it with. Detail: `pgdump_query/src/io.rs`.

- **KD32** — `max_source_span` is solved against the read-buffer budget, which
  D83 leaves at `DEFAULT_MEMORY_BUDGET` (capped by the allowance less
  `MEMORY_RESERVE`) on a plain source whatever `--memory` states, so a plain
  `query`'s sub-stream count and batch size are fixed at the number D3 picked
  to decline block decode on an ordinary `.xz`: a stated allowance raises
  neither, and `--chunk-size` moves them only as far as `KD41` lets it.
  **(c) unowned**; promoted with `KD25` by a reading of a parallel plain scan
  on a real device. Detail: `pgdump_query/src/stream.rs`.

- **KD14** — peak resident set is flat in dump bytes but grows several
  kilobytes per table, over a third of it live structure the preamble alone
  pays (`measurements.md`, `peak-rss` and `rss-attribution`).
  **(c) unowned**; promoted by a dump with tens of thousands of tables, nothing
  in hand being one. Detail: `pgdump_query/src/preamble.rs`.

- **KD29** — a flagless `pgdt` run reads its memory limit once for the
  status lines and the statistics allowance and again inside
  `Parallelism::discover_holding_in` for each read-buffer budget it carves, so
  a limit rewritten between two reads is announced and carved for statistics as
  one number while the pools are budgeted from another. **(c) unowned**; promoted by a limit
  seen to move inside a run, or by discovery that can take a limit already
  read. Detail: `pgdt/src/main.rs`.

- **KD31** — `attach_text` caps a run's one read at `SPAN_STORED_TEXT_MAX_BYTES` per span from
  the run's start, so a span following one longer than the cap can be stored
  empty and `truncated` however short it is. **(c) unowned**; promoted by a
  `--map` listing seen to lose a statement's text. Detail:
  `pgdump_query/src/map.rs`.

- **KD33** — statistics stop where the account fills: nothing releases
  `Term::Retained` while the pass gathers forward, so a dump long enough fills
  its allowance partway through and every block after it declines, leaving statistics a prefix of the
  file and a query pruning nothing over the tail. How much is covered depends on
  the allowance the box resolved, which a user cannot predict. **(b) owned by
  P23**, whose remedy is a granularity derived from the dump's length. Detail:
  `pgdump_query/src/gather.rs`.

- **KD35** — a budget too small for a decoded block is read serially: the
  piecewise arm advises one partition on both providers, where a decoder retains
  far less than the block it decodes and several would fit. What pins the count
  at one is not the budget but one forward-only handle behind a mutex — the
  fetched arm's statelessness, which would have served several partitions for
  want of cut points alone, was spent to stop it re-decoding a block a scan sits
  inside. **(c) unowned**; promoted by a phase taking up compressed scan
  throughput, which is also what takes the figure. Detail: `pgdump_query/src/io.rs`.

- **KD36** — a fetched `.xz` file with no cached seek table is walked one
  request at a time, so a cold remote open costs at least four round trips per
  stream — a footer, an index, a header and a padding probe — before a row is
  read. The remedy is a straddling window, below which one fetch a stream is a
  floor rather than a remaining cost. **(c) unowned**; promoted by a phase that
  tunes the network, which is where the deferred fetch policy belongs.
  Detail: `pgdump_query/src/io.rs`.

- **KD37** — a cancelled read the leader dispatched inside the statistics
  back-fill propagates instead of banking: a source answering a cancellation by
  failing its read ends a `parse` as `Error::ScanCancelled`, saving nothing since
  the throttle's last save,
  where the same Ctrl-C during the mapping pass is an interrupted run. **(c)
  unowned**; promoted by a parallel remote `parse` seen to error on Ctrl-C after
  its map reached EOF, the fix being the arm the mapping pass already carries.
  Detail: `pgdump_query/src/stream.rs`.

- **KD38** — a pgdump scan planned while others hold the session budget gets
  what they left, so a join's second-planned table can run on one reader while
  the first holds the whole allowance: planning order, which the user does not
  choose, decides a scan's speed. **(c) unowned**; promoted by a measured join
  of two pgdump tables slowed by it. Detail: `datafusion-pgdump/src/budget.rs`.

- **KD46** — a block holding one distinct value records `Ascending`, so a
  descending table holding one declares no ordering to DataFusion and a sort
  it would satisfy is planned. **(c) unowned**; promoted by a table seen to lose its
  ordering to one. Detail: `pgdump_query/src/summary.rs`.

- **KD47** — where an exact bound and a clipped one share their text, the
  first folded is kept, so a clipped one first leaves a column's `MIN` or
  `MAX` `Inexact` and read rather than answered from statistics. **(c)
  unowned**; promoted by an extreme seen read where it could answer. Detail:
  `pgdump_query/src/summary.rs`.

- **KD40** — a `--create` or `pg_dumpall` dump states each database's
  collation (I32), and nothing reads it: every text column with no `COLLATE`
  clause still warns that the dump does not record its collation, so a `C`
  database's columns warn where they agree with the server, and another
  collation's are announced as a possibility rather than a fact. **(c)
  unowned**; promoted by the roadmap's "Collation-aware comparison", whose
  environment-free half it belongs to. Detail: `pgdump_query/src/pgtype.rs`.

- **KD41** — a replay plan never announces its read chunk, so a plain
  source is cut by the chunk its last read announced: a `pgdt query` over a
  complete cache plans at the default whatever `--chunk-size` states, and a
  provider's first scan after `SET pgdump.chunk_size` at the chunk before it,
  so the smaller chunk a plan note advises seats nothing more on that plan.
  **(c) unowned**; promoted by a stated chunk seen not to seat what the note
  promised. Detail: `pgdump_query/src/stream.rs`.

- **KD43** — a stated `--row-group-max-rows` can be passed by a block whose
  last group stands unpaired: at 19, 39, 59… groups one merge can lower the
  90th-percentile group it is read at, so a block already past the maximum
  merges further past it. **(c) unowned**; promoted by a stated maximum seen
  to leave a block's groups past it. Detail: `pgdump_query/src/gather.rs`.

- **KD53** — a dynamic filter's cut at the first poll is not made where it
  would put two blocks in one sub-stream that nothing proves in a declared
  order, so a selective join into a table of several blocks declared sorted
  is read balanced over what its static filter keeps rather than over what
  the join's filter does. **(c) unowned**; promoted by such a table's probe
  side seen unbalanced, the fix being a cut that breaks at every boundary
  nothing proves. Detail: `pgdump_query/src/stream.rs`.

- **KD54** — a query's replay keeps every chunk a batch's views point into
  until the batch flushes, and the read pool keeps `POOL_DEPTH` of them, so a
  batch spanning more chunks drops the rest and the reads after it zero fresh
  buffers — a third of the costing row's off-leg user cycles, under mimalloc.
  **(c) unowned**; promoted by a figure pricing the zeroing or a phase taking
  up query-path memory. Detail: `pgdump_query/src/io.rs`.

- **KD55** — a membership over a text-compared column probes std's
  SipHash-keyed set with the field's text, among the costliest terms of the
  costing row's evaluation, where a fast hasher seeded once a process would
  keep a flooded list's defence at a fraction of the cost. **(c) unowned**;
  promoted by row evaluation going on by default, or a figure pricing
  `pgdt --where`'s `in (…)`. Detail: `pgdump_query/src/predicate.rs`.

- **KD56** — an ungrouped aggregate's dynamic filter drops a column's bounds
  in DataFusion 55.1 while a batch of it holds no value, so a scan pruning
  under it answers a `MIN`, or another column's `MIN` and `MAX`, wrongly under
  default settings. **(c) unowned**; promoted by a DataFusion release carrying
  the fix, the shell meanwhile defaulting the filter off
  ([`upstream.md`](upstream.md), "UF1"). Detail:
  `datafusion-pgdump/src/dynamic_filter.rs`.

- **KD34** — `MEMORY_RESERVE`'s 384 MiB does not cover what a run holds above
  its charge and its statistics account: the attribution sitting read a worst
  remainder of 544 MiB on a compressed `query`, and every `wide-xz24` `query`
  leg from 1 GiB up was OOM-killed in every rep on that build. The reserve was
  fixed before statistics existed and has not been read since. **(b) owned by
  P23**, which sets it from those readings and runs the blind gate an
  attribution cannot stand in for. Detail: `pgdump_query/src/io.rs`.

- **KD50** — on a host stating no limit, a flagless run cut to half of
  `MemAvailable` prints its budget as "what this source asks for" and its
  count as "lowered … by the allocation", beside a first line saying nothing
  enforces a limit, and a `--jobs` past one over a source recommending nothing
  prints the library's constant as that source's request too. **(c)
  unowned**; promoted by a status line read as the source's request where it
  was the machine's or the library's. Detail: `pgdt/src/main.rs`.

- **KD51** — a remote `.xz` with no cached seek table, from a server stating
  no usable validator, is walked before its run is refused as one nothing can
  check. **(c) unowned**; promoted by a user who meets it on a dump of many
  streams. Detail: `pgdump_query/src/io.rs`.

- **KD52** — a `--filter` term or `--where` leaf ending `is [not] null` or
  `is [not] unrepresentable` is matched as that literal suffix, so `xis null`
  asks for `x IS NULL` and `x is  null` is refused. **(c) unowned**; promoted by a user meeting
  either. Detail: `pgdt/src/main.rs`.

- **KD58** — `map_file` drops the non-seekable warning a cache load adds and
  never recomputes it, so `pgdt parse` of a one-block `.xz` does not list it
  where `pgdt info` does. **(c) unowned**; promoted by a user who parses such
  a file and is not told. Detail: `pgdump_query/src/stream.rs`.

- **KD75** — a float field spelled past the type's largest finite value, which
  `pg_dump --extra-float-digits` at zero or below writes and `float8in`
  refuses, reads as that largest value where a restore fails the table, and
  no parse refuses a field on PostgreSQL's terms.
  **(b) owned by P31**, slice 31.12. Detail: `pgdump_query/src/decode.rs`.

- **KD81** — a `numeric(p,s)` field is not rounded to its scale or refused past
  its precision as `COPY`'s `apply_typmod` does, so `1.005` in a
  `numeric(10,2)` is refused where the server stores `1.01`.
  **(b) owned by P31**, slice 31.15. Detail: `pgdump_query/src/decode.rs`.
