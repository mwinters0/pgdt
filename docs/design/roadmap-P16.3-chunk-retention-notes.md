# P16.3 — `layering.md`'s L3 deviation closed

What the next slice inherits. What this phase committed to is
[`roadmap-P16-parallel-scan.md`](roadmap-P16-parallel-scan.md); what has landed
is [`../status/STATUS.md`](../status/STATUS.md).

## What exists now

**`batch::RetainedChunks`** — the retained read chunks a zero-copy `Utf8View`
may still point into, and the four things that go with them: the deque, the
`Bytes` → `arrow::Buffer` conversion, the release rule read off the scanner
position, and the block-index invalidation a flush owes. `SourceChunk` is
private to `batch.rs`; nothing outside that module names it any more.

Its surface is four methods, and 16.4 changes what they mean rather than what
they are called:

| Method | What the caller says |
|---|---|
| `new()` | — |
| `retain(start, &Bytes)` | "I read this, at this absolute offset" |
| `release_through(floor)` | "the scanner has reached here" |
| `invalidate_block_cache()` | "a batch just flushed" |

`stream.rs`'s replay loop is what calls them, and it is now the only thing that
does. It no longer names `arrow::buffer` or `std::collections::VecDeque` at
all.

The change is behaviour-preserving: the same struct, the same eviction
predicate (`end() <= floor`), the same conversion, at the same points in the
same order. The whole diff is a move plus the two call-site renames.

## What the next slice inherits

**16.4 is now a change to one module.** The re-derivation the spec asks for —
`max_source_span` against block-shaped rather than chunk-shaped pinning — is a
change to what `RetainedChunks` holds and to the bound `should_flush` is
checked against, and both of those are in `batch.rs`. Nothing in `stream.rs`
has to move with it, because the replay loop's three statements say *what
happened* (a read, a scanner advance, a flush) rather than *what to keep*.

**The two facts 16.4 must not lose**, which are now written where it will read
them rather than inlined in a loop:

- A chunk is released only once the scanner has walked past its **last** byte,
  not once it has entered the next one. The row straddling a boundary is
  carried rather than scanned, so it never arrives asking for a chunk that has
  gone — and a release rule that rounded the other way would break exactly that
  row and nothing else, which is the hardest kind of bug to see.
- A cached `StringViewBuilder` block index is dead the moment a builder
  `finish()`es. Any new flush trigger owes the invalidation; the type is where
  that obligation is stated.

**`retain` takes `&Bytes` and clones inside.** The clone is a refcount bump on
the pooled buffer, and the caller still needs the `Bytes` to scan — so the
borrow is what keeps the *conversion and the clone that feeds it* on the L3
side of the boundary. Taking it by value would push the clone back into the
read loop, which is the half of the deviation that is easiest to reintroduce
without noticing.

## The calls worth knowing about

**`push_utf8view_field` reaches `chunks.chunks` directly** rather than through
an accessor. It is a free function in the same module, the search is
`iter_mut().find_map(…)` over the deque, and wrapping it would have meant either
an `iter_mut()` that hands out `&mut SourceChunk` — the same coupling with a
method call in front of it — or moving the view-taking into `RetainedChunks`,
which puts `StringViewBuilder` inside a type whose job is retention. Left as
field access on purpose.

**`SourceChunk` went private rather than staying `pub(crate)`.** That is what
makes the deviation's closure checkable by the compiler instead of by reading:
a later slice cannot reconstruct a deque of them outside `batch.rs` without
first making the type visible again, which is a diff someone reviews.

**No figure was re-taken, and `peak-rss` is now red.** It was the register's one
green figure standing at a commit after the read-path work, and its `depends`
carries `pgdump_query/src/stream.rs`, so this change stales it. Neither
mechanical oracle reaches it — the diff adds and moves executable lines, so
reachability does not excuse it, and byte-identity settles generator changes
only — so it stays red with the reason written down
([`../status/STATUS.md`](../status/STATUS.md)), which is what the rule asks for.
What a reader should expect of the re-take is *no movement*: the allocation
count, the retained set and the release schedule are unchanged, and RSS is
measured with a three-rep instrument that did not resolve three rounds of real
read-path work.
