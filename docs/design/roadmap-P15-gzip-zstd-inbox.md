# P15 inbox — facts filed for its grilling

Evidence that P15 (gzip and zstd input) will need. **This is a queue, not a
document**: when P15 is grilled, walk every entry, fold it into the spec or
discard it as stale, and delete this file. See `docs/process.md`, "Inboxes:
facts filed by destination".

---

## Relaxing `size()` is cheaper than it looks, and P13 deliberately did not spend it

**Fact.** P13 kept `ByteRangeSource::size()` meaning the exact addressable
length, because xz answers it exactly from its stream index. It measured the
alternative first: every read loop in the library is already written as
`want = chunk_size.min(size - read_pos)` … `read_pos += bytes.len()`, so **all
of them already tolerate a short read**. What actually depends on the number
being exact is the loop's exit test, and the coverage denominator —
`total_size`, `scanned_through`, and `pgdq info`'s percentages.

**Why P15 cares.** This is the decision this phase cannot avoid, since neither
codec can answer it: gzip's `ISIZE` is the uncompressed length mod 2^32, useless
above 4 GiB, and zstd's frame content size is optional and omitted by streaming
writers. So the choice is a first pass to compute the size, or a relaxed
contract — and the relaxation's cost is now known to be a loop-termination
change plus an honest answer for `info` when the total is not yet known, rather
than a rewrite of every caller.

**Origin.** 2026-09-02, grilling P13; the reasoning is in
`docs/design/roadmap-P13-compressed-input.md`, D1.

---

## The seekable-xz crate is xz-only on purpose, and generalizing it was rejected

**Fact.** The external crate P13 is blocked on
(`/mnt/wd12t/fedora/experiments/xz-seek/requirements-pgdump-query.md`) puts
gzip and zstd explicitly out of scope. The reason is the entry above: the exact
uncompressed size from the container's own footer is load-bearing in that
crate's interface (its R3), and a crate that treats three codecs as one shape
gets the size contract wrong for all three.

**Why P15 cares.** This phase will want an equivalent addressing layer for
BGZF and zstd's seekable format, and the natural instinct will be to extend the
xz crate. The interface is reusable; the size guarantee is not. Whether that
becomes a sibling crate, a generalized trait across both, or something this
repo owns is P15's to decide — but it decides it knowing the xz crate declined
the generalization deliberately rather than by oversight.

**Origin.** 2026-09-02, grilling P13.

---

## P13's other decisions this phase inherits

**Fact.** Four decisions in `roadmap-P13-compressed-input.md` bind any later
decompressing source: `stored_size()` on the trait with `SourceIdentity`
recording it (D4); the seek table living in the cache envelope as a sibling of
`ContainerKind`, which stays `Plain` because the span offsets genuinely are
plain-format offsets (D5); one streaming decoder behind a mutex, restarted on
seek, with no retained decoded output (D6); and a non-seekable file being warned
about rather than refused (D2).

**Why P15 cares.** A `.gz` source is the same shape of object and should not
re-decide any of them. The one that will feel wrong for gzip is D5's per-block
seek table — a gzip index is a set of *checkpoints* carrying 32 KiB of
dictionary each, not a list of independently decodable blocks — so the envelope
field must be able to hold either, and P13's naming should be checked for
having assumed xz's shape.

**Origin.** 2026-09-02, grilling P13.
