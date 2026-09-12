# P15 inbox — facts filed for its grilling

Evidence that P15 (gzip input) will need. **This is a queue, not a
document**: when P15 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## Relaxing `size()` is cheaper than it looks, and the `.xz` source deliberately did not spend it

**Fact.** `ByteRangeSource::size()` still means the exact addressable
length, because xz answers it exactly from its stream index. The alternative was
weighed first: every read loop in the library is already written as
`want = chunk_size.min(size - read_pos)` … `read_pos += bytes.len()`, so **all
of them already tolerate a short read**. What actually depends on the number
being exact is the loop's exit test, and the coverage denominator —
`total_size`, `scanned_through`, and `pgdq info`'s percentages.

**Why P15 cares.** This is the decision this phase cannot avoid, since gzip
cannot answer it: `ISIZE` is the uncompressed length mod 2^32, useless above
4 GiB, and the single-member shape has nothing else to offer short of a full
decode. So the choice is a first pass to compute the size, or a relaxed
contract — and the relaxation's cost is now known to be a loop-termination
change plus an honest answer for `info` when the total is not yet known, rather
than a rewrite of every caller. Note that the decision is shared with P18,
which meets the same problem in zstd's form; whichever phase runs first settles
the contract for both.

**Origin.** 2026-09-02, grilling the compressed-input work; the reasoning is
now beside the mechanism ([`architecture.md`](architecture.md), "The compressed
source", which states what `size()` promises and the two relaxations refused).

---

## The seekable-xz crate is xz-only on purpose, and generalizing it was rejected

**Fact.** The external crate the `.xz` source reads through
(`/mnt/wd12t/fedora/experiments/xz-seek/docs/design/historical/initial.md`) puts
gzip and zstd explicitly out of scope. The reason is the entry above: the exact
uncompressed size from the container's own footer is load-bearing in that
crate's interface (its R3), and a crate that treats three codecs as one shape
gets the size contract wrong for all three.

**Why P15 cares.** This phase will want an equivalent addressing layer for the
multi-member shape — BGZF above all — and the natural instinct will be to
extend the xz crate. The interface is reusable; the size guarantee is not.
Whether that becomes a sibling crate, a generalized trait across both, or
something this repo owns is P15's to decide — but it decides it knowing the xz
crate declined the generalization deliberately rather than by oversight.

**Origin.** 2026-09-02, grilling the compressed-input work.

---

## The decisions the `.xz` source already made, which this phase inherits

**Fact.** Four decisions bind any later decompressing source, and all four are
built and filed by subject ([`architecture.md`](architecture.md), "The
compressed source" and "The cache"): `stored_size()` on the trait with
`SourceIdentity` recording it, so the staleness check stays a `stat`; the seek
table living in the cache envelope as a sibling of `ContainerKind`, which stays
`Plain` because the span offsets genuinely are plain-format offsets; one
streaming decoder behind a mutex, restarted on seek, with no retained decoded
output; and a non-seekable file being warned about rather than refused.

**Why P15 cares.** A `.gz` source is the same shape of object and should not
re-decide any of them. The one that will feel wrong for gzip is the per-block
seek table — a single-member gzip index is a set of *checkpoints* carrying
32 KiB of dictionary each, not a list of independently decodable blocks. The
envelope field was shaped for exactly that: `CompressionIndex` is an enum from
its first commit with one variant in it, so this phase adds sibling variants of
its own layout rather than reshaping the field or fitting checkpoints into a
block list. It may need two of them rather than one, since the multi-member
shape's index genuinely is a block list. What still needs checking is naming
rather than shape — whether the accessor that hands a table to the cache reads
as xz-specific once a second codec is in.

**Origin.** 2026-09-02, grilling the compressed-input work.

---

## No test in the tree asserts that a compressed `parse` is deterministic in `--jobs`

**Fact.** `pgdump_query-cli/tests/determinism.rs` asserts that `pgdq parse`
writes one byte-identical `.dqcache` at every stated `--jobs`, and its whole
input tree is plain `.sql`. The compressed source's own suites assert something
weaker: `xz_source.rs` carries end-to-end row and `info --json` parity across
container shapes, and `parallelism.rs` carries budget parity, neither on cache
bytes. So the claim "a compressed parse at two job counts writes one cache"
holds in exactly one place — the 2026-09-08 koji run
([`measurements.md`](measurements.md), "koji full scan"), once, on one machine,
over a 40 GB file nobody can rescan cheaply. Nothing in CI can see it.

**Why this phase cares.** It adds the second decompressing source, so it either
inherits this gap or closes it — and closing it is cheap at fixture scale, the
compressed helpers and the determinism sweep both already existing. Two things
make it worth deciding at spec time rather than discovering later: a compressed
source is the one where `--jobs` changes which *decode* work happens (blocks
decoded whole, retained, possibly twice), so it is where a determinism defect
would live if there is one; and a gzip index is checkpoints rather than
independently decodable blocks, so whatever the assertion is for `.xz` may not
transfer unchanged.

**Origin.** The parallel-scan work's determinism and koji slices, 2026-09-08
([`architecture.md`](architecture.md), "Testing philosophy").

---

## The budget rule's two constants are both xz-derived, and one of them predicts every source's resident

**Fact.** `io::MEMORY_RESERVE` (384 MiB, the cap a discovered limit hands back)
and `io::MEMORY_UNPOOLED_BOUND` (256 MiB, what `margin_allowance` predicts a
count's resident with) were both read off one grid: 400 runs over `.xz` inputs
at 24 MiB and 128 MiB block sizes ([`architecture.md`](architecture.md),
"Execution model and API surface";
[`roadmap-P19.26-margin-constant-notes.md`](roadmap-P19.26-margin-constant-notes.md)).
The second bounds what a scan holds *outside* what `WorkerMemory::at` bills, and
the measured remainder is not flat in the codec's parameters: 83.5–214.6 MiB at
24 MiB blocks against 10.9–13.8 MiB at 128, i.e. **an order of magnitude
smaller where the decode unit is larger**.

**Why this phase cares.** A new decompressing source states its own
`default_worker_memory`, which is the per-worker term — and nothing obliges it
to look at the constant its resident is then *predicted* against. The two are
separate numbers in separate places, so a source whose decoder retains more per
unit than `liblzma` does gets a correct charge and an optimistic prediction, and
the failure surfaces as a cgroup kill at a large limit rather than as an error.
So the phase either argues the bound carries over — the term is glibc arena
retention as far as any reading goes, which is not codec-specific — or re-derives
it, which needs no new sitting if the phase's own resident readings cover more
than one reader count. `scripts/measure.py`'s `charge_model_problem` is what
would report it: its inner fault line is exactly this constant.

**Origin.** `19.26`, 2026-09-12.

---

## A source's own file index is unbilled and grows with the input, and that was decided rather than overlooked

**Fact.** `XzSource` holds its seek table — one 80 B entry a stream, one 32 B a
block — and `xz_seek::Reader` holds a second copy of it, and no charge bills
either: 6.65 MiB held on koji's 31,150-stream download against 8.2 KiB on the
3 GiB fixtures `MEMORY_UNPOOLED_BOUND` was read off. It is the only unbilled
term in the account that grows with the *input* rather than with the reader
count, the decode unit or the announced chunk. **Not billing it was argued from
an ordering, not from its size**: the index is built before `hint_parallelism`
states a budget, so a charge carrying it could not refuse a file whose index
does not fit — it would only subtract an already-spent allocation from the
allowance a worker count is solved against. That buys accuracy in the account
and no protection, at the cost of the count-independent third `WorkerMemory`
term `KD24` priced and refused. Registered as `KD26`
([`architecture.md`](architecture.md), "Billed against held: one row per buffer
the process keeps").

**Why this phase cares.** A seekable source of this phase's codec carries the
same kind of structure — a `.gzi` index over a `bgzip`-style file is one entry
a block — so the phase meets this decision in its own terms and inherits both
halves of the answer: the ordering argument that says an index need not be
billed, and the warning that the argument is only sound while the index stays
small next to the bound, which is a property of the producer's frame size
rather than of the codec. It also inherits a shape to avoid — the duplicate
exists because `xz_seek::Reader` takes its table **by value**, so a decoder
crate written or vendored for this phase should hand out a shareable handle
instead.

**Origin.** `M99`, 2026-09-12
([`../status/history/2026-09-12.md`](../status/history/2026-09-12.md), "The seek
table is held twice, and the walk runs before the budget does").
