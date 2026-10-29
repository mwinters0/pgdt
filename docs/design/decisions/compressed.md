# The compressed source and the cache

Part of the decision register, [`../decisions.md`](../decisions.md), whose rules are `docs/process.md`, "The decision
register"; an entry is cited by its `D<k>` against that index. Reading a dump through a compression layer, and
remembering what a scan found: recognition, the `.xz` and gzip sources and their seek structures, the first pass that
gives a gzip file one, and the cache file — its identity checks, what it persists and when it is refused. `pgdt-io`'s
`source/compressed`, `pgdt-gzip-seek`, the vendored `xz-seek` and `pgdt-dump`'s `cache`.

### D14 Recognition sniffs content; `xz-seek` is a vendored read-only copy
Recognition compares a file's first bytes with each format's magic, never its name, outside the source trait, told them
by `Origin`'s one probe (`CompressedFileSource::open`, opened alone, reads its own), which also answers D20's stored
size, so nothing settled before a source exists assumes a local file. Rejected: extension dispatch. `xz-seek` is a
vendored read-only copy (`scripts/vendor_xz_seek.py`): a defect or a missing seam is fixed upstream and re-synced, never
patched or bridged here, as its sans-IO walk and block handle were for a fetched source. Rejected: a path dependency,
failing `cargo check` without the sibling checkout.

### D15 A read decodes whole blocks and retains them; reading one in pieces is the fallback
A read decodes whole units and retains them, LRU, where the caller's budget affords one reader and the retention list
beside it, from file-wide inputs (`BlockCache::affordable`). Rejected: a fixed refusal line; keying on block count.
Below that line a read goes through one live forward unit, the local arm's behind a mutex and the fetched arm's taken by
its read, kept across reads so a forward scan continues, advising one partition (`KD35`); the fetched arm's window is
charged, not booked unpooled. Both arms are written once over a `dyn Codec` and their own `Positioned` read, never one
body branching on the provider. Rejected: a source per codec and transport; generic arms or an enum, holders being `dyn`
(D6) and the codec chosen at open. Evidence: `reserve`.

### D18 The seek table is persisted and identity is three-state
A many-stream `.xz` file costs a footer read per stream, so a save persists the compression index and an open hands it
to the source, which walks nothing (`CompressedFileSource::with_index`); a local BGZF file's walk is persisted alike, as
its gzip index (D116). `KnownCompression` is three-state, not an `Option`, because a claim of plain is one the file's
magic can contradict; a contradiction is `Recognized::Mismatch` and condemns the span index too, both written by one
save of one file.

### D19 A budget decline is a `PlanNote`; a non-seekable file is warned, not refused
A budget decline is a `PlanNote`, a property of the file *and this run's budget*; a `Diagnostic` is a finding about the
file alone. A file with no seek structure opens and is warned (`NonSeekableCompressedSource`), only its user being able
to judge decoding a unit from its start: an `.xz` file of one block, a gzip file with a unit wider than twice the
spacing, which only a writer coding more than a spacing in one dynamic block leaves (D118), or a false BGZF header. A
gzip first pass is a note, the pass making the file seekable. A statistics skip is noted at zero wherever statistics
were consulted, a narrowed span (D84) wherever the plan charged less than stated. Rejected: omitting a zero; counting
only believed filtered columns; warning gzip by unit count, which says nothing of the writer. Reopens: a puzzling zero.

### D112 `pgdt-gzip-seek` inflates through zlib-rs, by `libz-rs-sys`'s zlib C API
Of the engines exporting what an index needs through the zlib API, zlib-rs is the fastest at both of `pgdt-gzip-seek`'s
costs, a first pass and a unit resumed from its checkpoint, and the only one built with no C toolchain; why its C API
rather than its safe one is UF4's. zlib-ng and stock zlib stay features, the figure's legs. Rejected: zlib-ng and stock
zlib, slower at both; the system's `libz.so.1`, a dependency a release does not promise (D104); ISA-L, its block stops
only in rapidgzip's fork, its resume priming undocumented state. Reopens: another engine ahead at either cost in a
re-take, at a bump of any engine's pin. Code: `crates/pgdt-gzip-seek/Cargo.toml`'s features, `src/zlib.rs`. Evidence:
`gzseek-backend`; RT23.

### D113 gzip's checkpoint spacing is the block `xz -T0` writes by default
A shipped constant, never derived from the file. Across xz's block sizes a spacing moves only gzip's window section and
index size (`gzseek-spacing`), so it is chosen by what a unit shares with xz: a gzip unit is an ordinary `.xz` block to
D3's default-budget line and to the resident bounds read at that block, its inflater billed no term above xz's decoder.
Rejected: finer, more windows and index for nothing; coarser, past the block sizes the bounds were read at, toward the
one whose legs overran `MEMORY_RESERVE`; a power of two for its own sake, a unit always running past its multiple to a
boundary. Reopens: a resume's fixed cost not small at it; a gzip resident reading overrunning `MEMORY_UNPOOLED_BOUND`.
Code: `CHECKPOINT_SPACING`. Evidence: `gzseek-spacing`, `gzip-resident`, `reserve`.

### D114 A gzip cache's windows follow its encoded file, and every save writes them all
Each checkpoint's window follows the encoded `CacheFile` in the one cache file, so a load holds none and D20's and D21's
checks and D78's rename cover them with the map; every save rewrites the whole section through the provider the saved
index carries, its cost under D62's ratio. Rejected: windows inside the envelope, resident at open before any budget
exists; a file beside the cache, which no identity check covers, as D78 refused one for statistics; appending in place,
which leaves a killed save no whole previous cache. Reopens: a save's copy of the section, not the index's, setting a
save's duration. Code: `pgdt_dump::cache::file::windows`. Evidence:
`a_cache_saved_over_itself_keeps_its_windows_and_reads_its_file`.

### D115 A gzip first pass spills its windows to a file with no name, beside the cache or in the temp directory
Each window is written as it is placed to a file with no name (RT24), each save copying the section out of it: none is
resident in any cache mode, nothing outlives the run, and no name exists for D20's and D21's checks to cover. It is made
in the enabled cache's directory, else the OS temp directory, `open`'s caller naming it so `pgdt-io` names no cache.
Rejected: the windows in memory, unbilled, or billed by a `WorkerMemory` term that bounds nothing; re-basing onto each
saved cache, covering no disabled cache; the temp directory always, a tmpfs charged to the run's cgroup; a disabled
cache's would-be directory, beside a dump often on a read-only mount. Reopens: a spill directory named apart from the
cache's. Code: `CacheMode::spill_dir`. Evidence: `a_first_pass_spills_beside_its_cache_or_in_the_temp_directory`.

### D116 A forward pass coalesces BGZF members as the walk does, so a fetched BGZF file is read by a first pass
A fetched source recommends one reader (D2), so a member walk would download the object for parallelism the run does not
take, before the scan downloads it again; the first pass reads it once and coalesces members as the walk does, so a file
has one index whichever arm reads it, a broken chain included. Each member is decided by its own header and the unit it
would join, never by a stated length, which a resumed pass cannot know. Rejected: the walk on both arms, two downloads;
the coalesced run ending for good at the first plain header; a BGZF mode the caller states or recognition sets, a broken
file then indexed per arm. Reopens: a fetched source recommending more than one reader (D2). Code: `pgdt-gzip-seek`'s
`placement.rs`; `open_remote`. Evidence: `a_remote_bgzf_file_is_read_once_by_a_first_pass_that_builds_the_walk_s_index`.

### D117 A parallel first pass speculates with a decoder of `pgdt-gzip-seek`'s own, and hands out only the engine's plaintext
No zlib-API engine decodes from unknown history, so a speculation finds a chunk's bounds and ending history natively, by
decoding it or off its BGZF headers; the engine decodes from there, and the frontier takes a decode only standing at its
start with that history, so the index is the serial pass's. Its decoder decides who decodes a byte, never what it is: a
member's only check is its trailer, so a defect in ours would reach a caller unchecked till the member ends. Rejected:
native plaintext handed out, saving the engine's share of the CPU; the frontier waiting on its own chunk's speculation,
slower than serial; per-task fetches. Reopens: the second decode keeping the pass from lifting the inflate. Code:
`pgdt_gzip_seek::Parallel`.

### D118 A checkpoint inside a static block is placed by walking its codes beside the engine
A writer ending no block across a spacing is checkpointed as any other, with no inflater of our own: the engine walks a
static block's codes on the native decoder's tables, writing nothing, and caps its own output at the symbol (D117).
Rejected: `Z_TREES`'s header stop for a block's type (RT23); a taken chunk's decode walking its static blocks, which
needs marks only the frontier holds; holding a unit ending at a symbol to its compressed end, a second walk on every
read to refuse a unit sooner than the next unit's CRC-32 does; a bound on the bytes consumed, catching only a
misplacement past the lookahead. Reopens: a consumer acting on `IndexMismatch` apart from `UnitCheck`; the walk visible
in a first pass's profile. Code: `native/walk.rs`. Evidence: `a_checkpoint_moved_inside_a_static_block_is_refused`.

### D120 A parallel first pass gives decodes a room of their own, and places its speculations a lead ahead
A decode begins only once the frontier anchors the chain before its chunk, and the frontier waits for most of one begun
just ahead of it. So `Ahead::top_up` hands decodes out under `DECODES_A_TASK` a task, apart from the speculations and
guesses it holds under `GUESSES_A_TASK`, and a speculation is placed `Chunking::lead` chunks past where it would land
just ahead of the frontier, where the bound reaches (`Parallel::set_lead`). Rejected: one room for both, guesses held
keeping a worker from a decode; one run of guesses in flight at a time; a worker kept for decodes; the frontier
running another queued decode while it waits. Reopens: the second decode removed (D117). Code: `ahead.rs`. Evidence:
`a_speculation_is_placed_its_lead_past_where_it_lands`, `decodes_and_speculations_are_handed_out_and_counted_apart`.

### D121 A parallel first pass cuts its file at rapidgzip's chunk, four input chunks
`Chunking::SHIPPED`'s chunk is four of the input chunks every gzip decode pulls, rapidgzip's length: at the default
count the input chunk beats it on no file and is beaten on a level-6 writer's. Every task is charged it twice, as its
chunk and its feed, which the figure, affording every count its memory, does not price. Rejected: the input chunk
(`gzip-1mib-chunks`), the limit held, which a local task reads in one positioned read. Reopens: a re-take at the default
count on which the input chunk beats the shipped one on two of the three level-6 writers' files and is beaten on none.
Code: `ahead.rs` (`CHUNK_BYTES`). Evidence: `gzip-first-pass`.

### D122 A speculation's guess is cut at a bound on its events and on its bytes past its chunk, and continued
The limit counts plaintext, and an empty fixed block is ten bits of none, so a run of them grows a speculation's input
and the events a taken chunk's decode records without bound. So a guess is cut at the last boundary it reached past
`EVENTS_A_GUESS` events or the limit's worth of compressed bytes past its chunk, and continued from its stop as a
limit-cut guess is; its sinks grow to the limit's room and no further. Rejected: charging them instead, a cost on every
worker for writers zlib is not; the count a parameter a test sets small, the shipped one untested. Reopens: a writer
whose guesses the count cuts often. Code: `speculate.rs` (`EVENTS_A_GUESS`, `bound_past`), `native/sink.rs`
(`Held::new`). Evidence: `a_run_of_empty_blocks_inside_a_chunk_is_cut_at_the_event_bound`,
`a_marked_run_grows_its_sink_no_further_than_the_limit_asks`.

### D123 A taken chunk's decode records a BGZF member as its start, header and end, three events a chain read counts
Inside a BGZF member the placement owes nothing but its start (D116, D118), so a taken chunk's decode records, in one
whose header it reads, no static block, no block's end but its stop and one piece a member, and a chain read counts
three events a member against `EVENTS_A_GUESS`, cut and continued as D122's guesses are: a chunk of members of empty
blocks is bounded by construction. Rejected: handing back nothing past an event bound, which wastes the decode; cutting
it at its last boundary under the bound, as `cut_at_static` does, a cut where nothing need be recorded. Reopens: a
checkpoint owed inside a BGZF member. Code: `chunk.rs` (`Running::event`), `speculate/members.rs` (`EVENTS_A_MEMBER`).
Evidence: `a_decode_through_bgzf_members_records_only_their_starts_headers_and_ends`,
`a_chain_the_event_count_cuts_is_continued`.

### D124 A gzip first pass charges a task what `pgdt-gzip-seek` bounds it to write, and frees no decode buffer while it runs
`Chunking::task_bytes` sums bounds the crate states beside its code (`ChunkTask::speculation_bytes`, `decode_bytes`,
`GUESS_BYTES`), counting what is written. A decode's plaintext buffer, made for the limit, is kept and lent the next
decode handed out, never outnumbering the decodes charged (but `KD153`). Rejected: counting capacity, pages no write
touches; the larger sink alone, which a member begun inside a marked decode passes; a freed buffer charged, a fit to
what allocators keep; a mapping a buffer, zeroing a limit each decode. Reopens: `gzip-resident` under-billing. Code:
`Chunking::task_bytes`; `most_held` in `speculate.rs` and `chunk.rs`; `parallel/chain.rs`. Evidence:
`a_speculation_s_sinks_fill_together_within_its_bound`, `a_decode_of_a_guess_the_count_cut_records_within_its_bound`,
`a_decode_s_plaintext_buffer_is_lent_the_next`.

### D125 A parallel first pass hands out no speculation from when one finds nothing until its frontier crosses a block or member start
A speculation finds a chunk's start by a block's header, so over a member whose writer ends no block before its last
every one finds nothing, and the searches slow the frontier past the serial pass. The frontier's own block and member
starts are the signal: a pause lasts one block over a writer ending blocks often, and the member's rest over one ending
none. Rejected: a run of empty speculations before pausing, a constant to choose; recognising such a file and reading it
serially, a second rule beside speculation. Reopens: `gzip-first-pass` reading a writer ending blocks often slower than
before it. Code: `Parallel::speculation_through`. Evidence:
`a_member_ending_no_block_is_speculated_on_until_one_finds_nothing`,
`a_speculation_finding_nothing_pauses_speculation_until_the_next_block_start`.

### D126 A local gzip first pass keeps its frontier on one blocking thread from read to read
A `spawn_blocking` a read wakes the idle blocking thread the condvar picks, the longest idle in `pgdt`, so once
the tasks ahead leave threads idle the frontier changes thread and core at every read, unlike a serial pass's. Its jobs
run in order on one `spawn_blocking`, ended at the hand-over, on drop, or by its runtime's shutdown, dropping the task
holding its channel's sender (RT28). Rejected: a thread of our own (D12); pinning to a core, the operator's; keeping the
fetched arm's (D15); a kept thread only where tasks run ahead, a second path; an idle timeout, a value nothing anchors.
Reopens: `gzip-first-pass` reading a default count no faster than before it. Code: `first_pass/kept.rs`. Evidence:
`every_job_runs_on_one_thread_while_other_blocking_threads_idle`,
`a_runtime_shuts_down_at_once_with_a_pass_left_unretired`, `gzip-first-pass`.

### D127 `pgdt-gzip-seek` is a leaf crate of this workspace: a gzip index of our own over a bought inflater
A container's positioned read names no `pg_dump`, so it is a crate below `pgdt-io`, as `xz-seek` is, its size contract
its own: open until a pass ends, where xz's footer states it. It is built here beside the codec trait it serves until
the maintainer splits it into a repo of its own, vendored as `xz-seek` is (D14); its first pass is held to zlib-ng's
serial inflate. Rejected: `rapidgzip`, which holds every window resident in each reader, checks no unit, reads a
truncated file as whole, cuts its index by a chunk its thread count moves, runs a pool of its own (D1, D12) and fails a
release link; `rusty-rapidgzip`, a whole-file decoder with no index; a module of `pgdt-io`; a repo of its own from the
start. Code: `crates/pgdt-gzip-seek/`. Evidence: `inflate-throughput`, `gzseek-backend`.

### D128 A gzip index is one checkpoint list, the first full pass's by-product, each unit checked by its own CRC-32
A checkpoint is a plaintext offset, a bit and a window, empty at a member start, so a single member, concatenated
members and BGZF are one layout, a unit apiece; only BGZF's is found otherwise, by its walk (D18). A single member
states nothing of its interior, so the decode places them by a rule of the file alone, and a resumed pass or any
`--jobs` builds one index. A member's CRC-32 covers it all, so the pass records each unit's and a unit decode compares
it, but where whole BGZF members' trailers do. Rejected: a variant per shape, members wider than the spacing fitting
none; an indexing pass, two cold decodes; checkpoints where reads land; trusting units after a checked pass, a flipped
bit then wrong rows; a `.gzi` sidecar, a file no identity check covers. Code: `GzipIndex`, `pgdt_gzip_seek::placement`.
Evidence: `determinism.rs`'s gzip tests.

### D20 The library never replaces cache data automatically
A cache not ours, damaged, another build's or another file's (stored size, or a compression layer the source
contradicts) is almost always a wrong path or a changed file, so every open refuses it (`CacheMode::refusal`) before the
dump is read past its magic. `--overwrite-unusable-cache` (`with_overwrite_unusable`) starts cold over one recognisably
ours, for a file replaced under a stable name; foreign bytes never. Rejected: a cold start (a silent re-scan after a
typo or an upgrade); the guard in `CacheMode::save` (policy an embedder cannot override); keeping a damaged one of ours
from the flag (nothing readable lost, an unbumped version its likely cause); `load` checking size alone (a claimless
open gets another file's map).

### D21 Identity is `stored_size()` plus weak signals, in an opaque enum
`stored_size()` keeps the staleness check a `stat` where `extent()` needs a decompressing source opened first.
`SourceIdentity` is opaque, read through signal accessors, so two kinds compare rather than refuse. Between runs a weak
signal — modification, origin (D87) — is advisory, warned and never persisted, binding only under its own
`StrictIdentity` term, absence included. During a run a change is an error, checked where asking is cheap: locally at
the save's cadence (D62), at run end and before a failure is reported (the failure is often the change); remotely by a
precondition on every ranged GET. A source nothing can pin is refused unless `NONE`. Rejected: a check per chunk (rows
still reach a caller first); an unpinned run merely warned of.

### D87 A remote cache is named from the URL, and the origin it records is advisory
Default `./<last URL segment>.dtcache`; no last segment is refused by name. Another origin is a diagnostic, the stored
size still refusing (D20), so two same-named dumps of equal size from different hosts in one directory read each other's
map with a warning the origin makes legible. A local cache records none, its default sitting beside the dump. Rejected:
a canonical path as a local origin, advisory after every move.

### D90 The DataFusion provider reads only a complete cache, never maps
`PgDump::open` believes only a cache reaching the file's end whose identity checks pass, and refuses anything short
naming the mapping pass that builds it; `pgdt sql` never parses, a parse wanting `pgdt`'s discovery, interrupt guard,
status lines and cache rules. So every block a table owns is seen (`KD6` is the cold query's), a schema is stated from
every block's census, no database's DDL is unread, the partitioned replay is always available, and there is no partial
map for a dropped stream to keep (D26). Rejected: falling back to a cold query's early-stopping map, which brings each
of those back. Code: `datafusion_pgdump::PgDump::open`.

### D22 `Held` is its own type and `CACHE_FORMAT_VERSION` is bumped freely
`Held` is its own type: an incomplete cache is usable, `Disabled` is about the caller and `Replacing` carries its reason
to every caller. `CACHE_FORMAT_VERSION` bumps on any persisted reshape, a parse fix changing what a span or the metadata
holds included, and the bump is recorded nowhere: an older cache is refused until replaced by the flag or deleted (D20),
which is what a user hears of first. Rejected: an unbumped parse fix, its cache answering as the old parse did. Code:
`pgdt_model::CACHE_FORMAT_VERSION`.
