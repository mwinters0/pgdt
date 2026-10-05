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
`total_size`, `scanned_through`, and `pgdt info`'s percentages.

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
now beside the mechanism ([`decisions.md`](decisions.md), "The compressed source and the cache", which states what `size()` promises and the two relaxations refused).

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
built ([`decisions.md`](decisions.md), "The compressed source and the cache" and "The compressed source and the cache"): `stored_size()` on the trait with
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

**Fact.** `pgdt/tests/determinism.rs` asserts that `pgdt parse` writes one
byte-identical `.dtcache` at every stated `--jobs`, and its whole input tree is
plain `.sql`. The compressed source's own suites assert something weaker:
`xz_source.rs` carries end-to-end row and `info --json` parity across container
shapes, and `parallelism.rs` carries budget parity, neither on cache bytes. So
the claim "a compressed parse at two job counts writes one cache" holds in
exactly one place — the 2026-09-08 koji run
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
([`decisions.md`](decisions.md), "D73").

---

## The budget rule's two constants are both xz-derived, and one of them predicts the resident of every source that states a cost

**Fact.** `io::MEMORY_RESERVE` (384 MiB, the cap a discovered limit hands back)
and `io::MEMORY_UNPOOLED_BOUND` (256 MiB, what `margin_allowance` predicts a
count's resident with) were both read off `.xz` inputs alone, at 24 MiB and
128 MiB block sizes ([`decisions.md`](decisions.md), "I/O, memory and parallelism"). The second bounds what a scan holds *outside* what
`WorkerMemory::at` bills: its worst over the 400-run grid the reserve was read
off is 238.6 MiB at 24 MiB blocks and 142.0 MiB at 128, and the published
`reserve` sitting's `Unnamed` column ([`measurements.md`](measurements.md), "What a scan holds above the budget it was given") is one codec at two decode units, and no
reading on any other.

**Why this phase cares.** A new decompressing source states its own
`default_worker_memory`, which is the per-worker term — and nothing obliges it
to look at the constant its resident is then *predicted* against. The two are
separate numbers in separate places, so a source whose decoder retains more per
unit than `liblzma` does gets a correct charge and an optimistic prediction, and
the failure surfaces as a cgroup kill at a large limit rather than as an error.
So the phase either argues the bound carries over — the term is the
allocators' retention as far as any reading goes, which is not codec-specific,
though on the shipped build's mimalloc the `reserve` figure's 128 MiB-block
cells at four and five readers already overrun it where their `system` twins
do not — or re-derives it, which needs no new sitting if the phase's own resident readings cover more
than one reader count. `scripts/measure.py`'s `charge_model_problem` is what
would report it: its inner fault line is exactly this constant.

**Origin.** The margin constant's derivation, 2026-09-12
([`decisions.md`](decisions.md), "I/O, memory and parallelism").

---

## A source's own file index is unbilled and grows with the input, and that was decided rather than overlooked

**Fact.** `XzSource` holds its seek table — one 80 B entry a stream, one 32 B a
block — shared with the `xz_seek::Reader` that built it, and no charge bills it:
3.33 MiB held on koji's 31,150-stream download against 4.11 KiB on the
3 GiB fixtures `MEMORY_UNPOOLED_BOUND` was read off. It is the only unbilled
term in the account that grows with the *input* rather than with the reader
count, the decode unit or the announced chunk. **Not billing it was argued from
an ordering, not from its size**: the index is built before `hint_parallelism`
states a budget, so a charge carrying it could not refuse a file whose index
does not fit — it would only subtract an already-spent allocation from the
allowance a worker count is solved against. That buys accuracy in the account
and no protection, at the cost of the count-independent third `WorkerMemory`
term `KD24` priced and left untaken. Registered as `KD26`
([`decisions.md`](decisions.md), "D4").

**Why this phase cares.** A seekable source of this phase's codec carries the
same kind of structure — a `.gzi` index over a `bgzip`-style file is one entry
a block — so the phase meets this decision in its own terms and inherits both
halves of the answer: the ordering argument that says an index need not be
billed, and the warning that the argument is only sound while the index stays
small next to the bound, which is a property of the producer's frame size
rather than of the codec. It also inherits a shape to insist on — `xz_seek`
took its table **by value** and so held a second copy of it until that API
was widened to share one, so a decoder crate written or vendored for this phase
should hand out a shareable handle from the start.

**Origin.** The seek table's account, 2026-09-12
([`decisions.md`](decisions.md), "D4";
[`../status/history/2026-09-12.md`](../status/history/2026-09-12.md), "The seek
table is held twice, and the walk runs before the budget does").

---

## A compressed source composes over any transport, and one value holds its budget policy

**Fact.** `FetchedXzSource` holds an `Arc<dyn ByteRangeSource>` and delegates
every trait answer to it, so the whole compressed-over-fetched composition is
exercised over a local file with no server and no feature enabled. The budget
policy that both `.xz` sources answer from — apportionment, the charged chunk,
the block path and its partitions, the block decode charge — is one value they
hold (`io::XzBudget`), with the transport reaching it only as the decoder
charge its constructor is handed and as the wait policy each source announces
for itself; `hint_wait_policy` and `default_workers` stayed outside it, being
the two answers the two sources genuinely differ on.

**Why P15 cares.** This phase adds a *format*, not a transport, so the shape
to copy is that one: a source per codec, each holding the budget value and
composing over whatever byte source it is given, rather than a source per
(codec, transport) pair. If the codec's reader cannot be driven from a caller's
window the way `xz-seek`'s block handle is, that is the difference to find
before the slices are written, since it is what decides whether the fetched arm
exists at all.

**Origin.** The remote-input work, 2026-09-20. *Contingent on* both `.xz`
sources still sharing one budget value — see `pgdump_query/src/io.rs` and
([`decisions.md`](decisions.md), "D15").
