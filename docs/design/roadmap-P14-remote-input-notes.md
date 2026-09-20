# P14 — remote input: what the phase left behind

The phase's decisions are its spec,
[`roadmap-P14-remote-input.md`](roadmap-P14-remote-input.md), "D1"–"D21", until
the keystone folds them into [`decisions.md`](decisions.md); the mechanisms are
the code, and every call a slice made beyond the spec is filed beside the
mechanism it governs. What is here is what neither carries: the results that
were negative, and the facts aimed at the phases that follow.

## What it delivered, and what it did not

**Delivered: a dump is read over HTTP, plain and `.xz`, end to end.** Behind the
default-off `http` feature, on all three commands: one ranged GET probes, one
reader reads, no credential leaves the process, the cache is named after the
URL and records the origin it was written for, and the server checks the
object's version on every request it answers. A `.xz` object is read block by
block out of windows we fetch. A cancelled read is an interrupted run on both
providers rather than an error.

**Not delivered: anything about the network's speed.** The phase was scoped to
correctness, and three costs are left priced by nobody: the cold footer walk's
round trips a stream (`KD36`), the single partition both piecewise arms advise
(`KD35`), and the fetch policy — readahead, straddling, concurrency — that a
phase tuning the network would own. No such phase is in
[`roadmap.md`](roadmap.md)'s index, which is why both entries are `(c)`.

## What the phases that follow inherit

- **A compressed source composes over any transport.** `FetchedXzSource` holds
  an `Arc<dyn ByteRangeSource>` and delegates every trait answer to it, so the
  whole composition is exercised over a local file with no server and no
  feature. P15 and P18 add a format, not a transport: the shape to copy is
  `XzBudget` — one budget policy both sources hold, the transport reaching it as
  the decoder charge its constructor is handed and as the wait policy each
  source announces for itself.
- **A URL is a source, not a mode.** `Origin` answers stored size, weak
  identity and leading magic before any source exists, and `cache::claim`,
  recognition and `CacheMode::resolve` all consume it, so nothing downstream
  branches on where a dump came from. An embeddable surface (P6) gets a
  `ByteRangeSource` whose failures already name the URL.
- **The in-flight identity check is the server's, remotely.** It rides on every
  ranged GET as a precondition, at a finer cadence than `SourceWatch`'s, and
  `SourceWatch` is therefore inert on that provider — it re-observes through a
  source that answers off its probe. A server stating neither validator is read
  unpinned and has no check at all.
- **P6's inbox already holds this phase's two entries for it** — the cancelled
  read as `MapRun::interrupted`, and what an embedder sees between two reads.
  Nothing here repeats them.

## Negative results

**Cancellation.** `tokio_util::sync::CancellationToken` was costed and refused:
a dependency for twenty lines over the `sync` feature already carried.
`tokio::select!` went with it, needing the `macros` feature for a two-branch
race `futures::future::select` runs. A plain `Notify` is not enough on its own —
`notify_waiters` stores no permit, so the waiter registers *first* and loads the
bit second, and the store is `SeqCst` to order against that load. The predicate
that routes a dropped read is `ScanCancelled` **and** the flag: a source
reporting a cancellation nobody asked for is misreporting itself and stays an
error. Nothing was added to `ByteRangeSource` to say "this source cancels by
failing" — the variant and the flag say it between them. `map_for_query` was
deliberately left turning the interrupt back into an error, which is why the
mapping pass and not "every read loop" was the right scope.

**The origin.** The probe does not reuse `ByteRangeSource`: reaching
`stored_size` through a source means opening the thing the early refusal exists
to spare, which for a many-stream `.xz` is the whole footer walk. One probe is
cached and a *failed* one is not, a failure having a sentence to say at the open
that follows. An empty remote object cannot be probed at all — a ranged GET of
the leading bytes against a zero-length object is a `416` — and was left that
way rather than spending a second round trip on the warm path.

**Identity.** `CacheSourceMismatch` did not grow a field: its `path` is the
*cache's* and always was, and the source's name is put in front of the refusal
by the CLI, which is the division every other source-moved refusal now shares.
`CacheMtimeChanged` kept its wording and gained a sibling rather than being
widened to "the modification signal", there being a payload to route on.
`SourceIdentity` compares through signal accessors rather than matching a
variant pair, so a local cache read over a fetched source reports a difference
instead of meeting an irrefutable `let`. No `RT<n>` was added for the
descriptor-keeps-its-inode property: it is POSIX, and a test that renames a file
over an open source's path establishes it. No test races a rewrite against a
scan — the identity moves in the *source*, which is what the library sees.

**The oracle.** No HTTP crate was taken as a dev-dependency, and its self-tests
deliberately do not go through `object_store`: an instrument checked with the
thing it judges agrees with it by construction, so the checks write requests
onto a `TcpStream`. Every connection is answered once and closed, which is what
makes a knob's ordinal a request number and a truncation a plain short write. A
malformed `Range` is a `400`, never a full body — "an unparseable header is an
absent header" would let a test assert a 206 and silently get a 200. A short
range and a truncated body are two knobs, because a reader trusting
`Content-Length` over what arrived passes one and fails the other. The stall
knob addresses one request rather than a suffix, what it exists to observe being
what the client does *next*. The oracle serves no `.xz`: nothing about the
composition is HTTP-shaped, so a compressed fixture through `serving_file` is
all that was needed. The shipped read deadline needed a second constructor to be
assertable at all — a shipped value small enough to test is a value chosen for
the test.

**The fetched `.xz`.** The cold walk is announced on the status channel and not
as a `Diagnostic`: a `Diagnostic` rides on a `DumpIndex` or a `CacheStatus`,
both built from a source that already exists, which is after the walk, and the
point is that the line arrives before. `default_workers` stays at one while
`default_worker_memory` does not, and the reason is asymmetry rather than
deferral — too few workers costs throughput a `--jobs` recovers, too many opens
`min(cores, block_count)` connections to a third party's server for a flagless
command. Keeping `partitions()` honest is what forced that split: a single
partition over a seekable table reads as a declined block path, so advising one
unconditionally would have reported every fetched run as declining the arm it
was taking. The piecewise arm is not free of the budget — it holds the window,
the decoder and one chunk — which is the local fallback's shape and the same
precedent. This source alone refuses the wait a read loop grants: its fetch must
`await`, so the buffer is obtained on the runtime's own task, where the pool's
`Condvar` would block the `current_thread` runtime and the releasing sibling
would never run.

**Retaining what a block read holds.** Every read of a block was a re-fetch
*and* a whole-block re-decode before the phase reopened for it, so a forward
scan of a single-block file re-read the file once per chunk — a path turned
unusable, not one made slower. The window could not be retained by cloning it
into the decode, cheap as a `Bytes` clone is: that puts the retention beside the
decode instead of after it, and the block a read ends in is the one worth
keeping. A failed read retains nothing, the slot being written back past the
short-read check. Nothing counts decodes in a shipped build — the counter is
absent without `introspect`, and the oracle's request count could not stand in,
one fetch a block having already been won. The always-completed guarantee became
the local arm's moment: a handle is completed when it leaves a block, so one
dropped instead compares nothing, and a `--check=none` stream is abandoned
rather than drained, there being nothing to verify. Neither `measurements.md`
nor the manual was swept for the *streaming decoder* wording: the figures' legs
were taken on a mechanism that no longer exists and renaming them would describe
a reading by a mechanism it did not run on, and what the manual claims — each
block's extent fetched whole, a backward read decoding forward from its block's
start — is still what happens.

**The budget extraction.** The premise was re-tested before anything was
written: the six methods were compared body for body with comments stripped and
were identical. The seek table stayed out of the policy — the cut points are a
property of the file, and holding it would read as the budget owning the file's
shape. `hint_wait_policy` and `default_workers` stayed out for the opposite
reason: they are the two answers the sources genuinely differ on, and moving
them would put a provider branch inside the value the extraction exists to keep
free of one.

**The register was never the place.** No slice added a `D<k>`:
[`decisions.md`](decisions.md) sat at or within a couple of lines of its cap
throughout, the spec held the phase's reasoning, and D14, D15, D18, D19 and D26
were *corrected* rather than joined — each having described a mechanism this
phase moved. That is the harvest the keystone still owes.
