# P14 — Remote input

Read a dump over the network: `pgdq parse --source https://example.com/foo.dump`,
and `query` and `info` beside it. The phase adds one `ByteRangeSource` backed by
`object_store` and whatever the layers above it need in order to be honest about
a source that has no `stat`.

**This document is under construction**: it is being written round by round as
[`roadmap.md`](roadmap.md), "P14 — Remote input" is grilled, and the sections
below hold only what is settled. Its inbox is
[`roadmap-P14-remote-input-inbox.md`](roadmap-P14-remote-input-inbox.md), drained
into this file as the grilling walks it.

## Scope

**HTTP and HTTPS only.** `object_store` ships backends for S3, GCS, Azure and
more behind their own features; none of them is in this phase. The URL schemes
this phase accepts are `http://` and `https://`, and an object-store URL of any
other scheme is refused with a message saying so. A second backend is a later
phase's, and the reason to hold it back is that each one adds credentials,
region resolution and its own failure vocabulary to a phase whose job is to
prove the *shape* works.

**Correctness only.** What this phase owes is right answers over a ranged-GET
source: the same rows, the same map, the same cache, the same refusals as a
local file. What it does **not** owe is a number. Fetch concurrency,
coalescing, prefetch, the ranged-GET size, the readahead depth, the per-worker
resident term a network buffer sets — all of it is a later phase's, run when
there are more backends to tune against and a second device class to tune for.
So the inbox entries that ask this phase to *price* something are deferred by
this rule rather than answered, and the entries that ask it to *decide a
contract* are in scope.

Two consequences worth stating, because they read as omissions otherwise:

- A default this phase picks is picked to be *defensible and correct*, not
  measured. Where a constant has to exist it is chosen and marked as unmeasured,
  and the later phase that tunes it is named.
- `measurements.md` gains nothing from this phase. No figure is taken, and no
  slice row commits to one ([`roadmap.md`](roadmap.md), "A slice row that
  commits to a measurement names its instrument" therefore binds no row here).

## Identity: advisory by default, strict on request

**The default is exactly today's stance.** A remote source's ETag and
`Last-Modified` are treated as the local source's mtime is treated
([`decisions.md`](decisions.md), "D21"): **advisory**. A mismatch is a
diagnostic, never a refusal, and it is never persisted. What still refuses,
remotely as locally, is the *size* check — a cache recorded against a different
stored size is refused before a byte of the dump is read
([`decisions.md`](decisions.md), "D20"), and that rule is unchanged.

The maintainer accepts that ETag and `Last-Modified` are not perfect guarantees:
an ETag is opaque and a server may change it without the bytes changing, or keep
it across a rewrite; `Last-Modified` has one-second granularity. That is the
reason they stay advisory by default, and it is the same reason the local
mtime is advisory today.

**A flag opts in to strict identity**, and it applies to the local file source
too. Under it a modification-time mismatch is a **loud failure** rather than a
diagnostic: the run stops and says the cache was written against a different
version of this file. It exists for the operator who knows their pipeline
rewrites dumps in place and would rather stop than read a stale map, and it is
one flag governing both providers because the question it answers — *may I trust
the weak identity?* — is the same question on both.

**Two different questions hide under one word, and only one of them is
advisory.** *Between* runs, a dump that was moved, copied or touched is a
different weak identity holding the same bytes — data moves, and that is the case
the advisory default is right for. *During* a run, a source whose identity
changes while a read is in flight is not a moved file: it is bytes changing
underneath a read that has already returned some of them. That is corruption on
every provider, so **it is an error by default** (D10), and `none` is what makes
it advisory.

## What the backend actually does

`object_store` **0.14.2**, read at that version and **verified by running it**
against four local HTTP servers (range-capable, range-ignoring, no-ETag,
slow-body) on a `current_thread` runtime, 2026-09-17. These are the facts the
decisions below stand on; each one a decision comes to depend on earns an entry
in [`runtime-invariants.md`](runtime-invariants.md) as that decision lands.

**What it costs to depend on.** The `http` feature is 120 crates and pulls
`aws-lc-rs`/`aws-lc-sys`, a C build needing cmake. The module itself is gated on
`http-base` (83 crates, no TLS at all, and a runtime error rather than a compile
error if no connector is supplied); the documented escapes from the C toolchain
are `http-base` + `reqwest` + `reqwest/native-tls` or `reqwest/rustls-no-provider`.
**Nothing anywhere in that tree enables `tokio/rt-multi-thread`**, and there is
no `tokio::spawn` on the request path — `current_thread` is verified to work,
which is what [`decisions.md`](decisions.md), "D12" requires. It does need the IO
and time drivers, so the CLI must declare `tokio`'s `net` and `time` features
itself rather than inheriting them by feature unification from this dependency.

**`head()` is a real `HEAD`.** PROPFIND appears only on `list`/
`list_with_delimiter`, which this project never calls. So a plain static host —
nginx, a release asset — is readable, and the failure mode that would have made
that false does not exist.

**A ranged GET answers everything the probe needs, in one round trip.** On a 206
the crate rewrites `ObjectMeta.size` from `Content-Range`'s total, so one ranged
GET of the file's first bytes yields the magic bytes, the stored size, the ETag
and the `Last-Modified` together — no separate `head()`. That is what makes D2's
origin probe one request remotely rather than two.

**A server that ignores `Range` is a clean, dedicated error.** A 200 where 206
was asked for raises `Error::NotSupported` carrying `RangeNotSupported`,
**on the status line, before the body is drained** — so a range request against
a range-ignorant server does not download a 784 GB object first. There is no
fallback to fetch-and-slice, and positioned reads are simply unavailable there.

**`Content-Length` is unconditionally required**, so a chunked or
content-encoded response fails; the crate defends this by disabling response
compression on its own client.

**Weak identity is asymmetric, and one half cannot say "absent".** `e_tag` is
`Option<String>` and honestly `None` where the server sends none. `last_modified`
is **not** optional: where the server sends no `Last-Modified` the crate
substitutes the Unix epoch. So "the server said nothing" and "the server said
1970-01-01" are indistinguishable in the returned metadata, which the strict
identity rule has to account for rather than assume away.

**The server can enforce identity itself.** `GetOptions` carries `if_match`,
`if_none_match`, `if_modified_since` and `if_unmodified_since`, all of which the
HTTP backend sends. `version` is silently dropped by this backend.

**The client defaults, which are not ours yet.** `timeout` is `Some(30 s)` and is
a **total deadline including the response body**; `connect_timeout` 5 s;
`read_timeout` `None`; `http1_only` **true**; `allow_http` **false**, so an
`http://` URL builds and then fails at request time unless the option is set;
`randomize_addresses` true. `RetryConfig` is 10 retries with a 3-minute
retry deadline, backoff 100 ms doubling to 15 s, retrying 5xx/429/408 and
connection errors.

**A missing ETag also costs mid-stream resume.** The crate resumes a broken
response body by re-requesting from where it stopped, but only when an ETag was
present and still matches; without one a mid-body failure is fatal.

**`get_ranges` coalesces across gaps of up to 1 MiB**, with no public knob short
of implementing the method.

**`parse_url` classifies some `https://` URLs as other backends** — an
`*.amazonaws.com`, `*.blob.core.windows.net`, `*.r2.cloudflarestorage.com` or
Fabric host parses as S3/Azure rather than HTTP. That is where D3's "refused by
name" has to actually look, since the scheme alone does not say which backend a
URL selects.

## Decisions

### D1 — Remote composes with `.xz`, and the cold footer walk is announced rather than refused

A remote `.xz` is read exactly as a local one is. Where a cache holds the seek
table the walk does not happen at all — `claim` reads the table and
`XzSource::with_table` walks nothing ([`decisions.md`](decisions.md), "D18") —
and a warm remote query is therefore a cache read, an identity probe and one
block's bytes. Where there is no cache, the walk is **one ranged GET per stream
footer**, which is 31,150 round trips on the koji download, and the source
**announces that at open** through the diagnostic channel, naming what it is
about to cost and the remedies: parse once against a local copy, or keep the
cache.

*Rejected: refusing a cold remote walk above some stream count.* A threshold is
a tuned number, and this phase has said it produces none; worse, it denies a
user a command that would work. The precedent is exact —
[`decisions.md`](decisions.md), "D19" warns about a one-block `.xz` and refuses
nothing, because only the user can judge one decode-from-zero. The same sentence
holds with round trips in place of a decode.

*Rejected: leaving remote `.xz` out of the phase.* It is the composition that
motivates the phase: the dumps that are actually shipped around arrive
compressed, koji's included, and the warm case already works.

The cold walk earns a **known deficiency** rather than a fix here. Its remedy is
a **straddling window**: one read per stream, positioned backward from the
pending request and sized to cover what the walk asks for next, which answers
three of the walk's four per-stream requests and leaves one fetch a stream. The
walk is a strictly sequential backward chain, so neither coalescing nor
concurrency buys anything on its own
([`../status/history/2026-09-18.md`](../status/history/2026-09-18.md)), and the
window is what `xz-seek`'s own synchronous driver already does — which is why the
containing form of `supply` is the one a remote driver is written against. That
is exactly the network tuning this phase has deferred, and the entry is owned by
the phase that takes that up.

**One fetch a stream is a floor on a file with no stride, not a cost still
looking for a remedy.** A stream's header is adjacent to its predecessor's
padding, footer and index, so one read straddling that boundary answers all but
the request that first touches it; and which request that is can be changed but
not removed, because the distance to the next boundary is knowable only from the
index just read. **What the straddle rests on is `Backfill`'s window reaching
*back* from the end of the range asked for**, rather than forward from its
start — the property to re-check when 14.7 re-vendors, `P9` having rewritten
that code around the walk machine without changing its arithmetic. So no rearrangement of what the walk emits gets below one round
trip a stream, and 14.7 should take that fetch: every cold path that pays the
walk goes on to read the whole file — `parse` scans to persist, a `query` with
no cache scans too — so its round trips precede a transfer of the same order.
Only prediction would beat the floor, which is why it is conditional on
the paragraph above and not absolute: it is the *rejected* alternative's
hypothetical producer — one padding its streams to a compressed boundary — that
would break it, and such a file would be worth speculating on after all. The
half of this that is measured rather than argued is that no such stride is
present here.
The deficiency is therefore open at the optimum for a driver that fetches what it
is asked for, and what would beat it is not a better driver but the forward
table build, which stops paying separately for the walk at all.

*Rejected: a speculative fetch at a guessed stream stride, verified by footer
magic.* This was the remedy named here until `xz-seek`'s `9.1` drove the walk's
request sequence over the koji download and confirmed none of its guesses, at
any depth or slack it tried, while costing an extra fetch per stream; the hit
rate it moved was the noise it rode on. The file has no stride to have guessed,
and the reason generalises past this file: a parallel `xz` chunks its *input*,
so what a multistream producer holds constant is the uncompressed stream size —
koji's download is uniform in it, which is what lets
`scripts/generate_xz_input.py` take a stream-aligned prefix of a stated
plaintext size — while the walk addresses compressed space, where that constant
arrives divided by a ratio that varies with the content. A stride hypothesis is
not approximately wrong there, it is the wrong kind of model. A producer padding
each stream to a compressed boundary would have one, and we know of none.

### D2 — An **origin** answers the cheap questions before any source is constructed

`cache::claim` today is synchronous, `stat`s the dump path, and settles two
things before a `ByteRangeSource` exists: the persisted seek table, and D20's
refusal of a cache recorded against another stored size — which is why all three
commands report that refusal without opening anything, and why an `.xz` size
mismatch pays no footer walk to reach it. Recognition then sniffs the file's
magic from the **path**, not through the trait. Both are local-file assumptions
sitting on the warm path.

This phase names the missing concept instead of special-casing the caller. An
**origin** is what a `--source` argument resolves to, and it answers two
questions cheaply and asynchronously, before any expensive source is built: its
**stored size and weak identity**, and its **leading magic bytes**. Locally that
is a `stat` and a small `pread`; remotely it is one `head()` and one small ranged
GET. `claim` consumes the probe's answer rather than taking a `&Path`, and
recognition is told what the file holds rather than reading the path itself.

D20's property is preserved exactly: the refusal is still settled before a byte
of the dump is read, and still costs one probe rather than a footer walk.

*Rejected: a second `claim_remote` beside the first.* It writes the refusal in
two places and guarantees they drift. Refusing that combinatorial branch is what
[`decisions.md`](decisions.md), "D6" made the trait dyn-compatible for.

**Consequence.** `claim` becomes async and every caller changes — a rework of an
already-tested core path, so it is its own slice, landed before the backend
exists and with the local source as its only user at that point
([`../process.md`](../process.md), "Size a slice by its review, not by its
scope").

### D3 — `--source` is a URL first and a path second; the scheme selects the backend

The argument is parsed as a URL, and falls back to a path when it is not one.
`http` and `https` select the remote backend; **any other scheme is refused by
name**, which is where this phase's "HTTP only" scope is enforced rather than
merely stated. Anything that is not a URL is a path, exactly as today. Which
URLs are accepted, and what a scheme is allowed to imply, is D17.

*Rejected: a second flag naming a remote source.* One argument means one concept
— where the dump is — and every command already takes it.

### D4 — A remote cache's default path is the URL's basename, and the origin it was written for is advisory

`CacheMode::resolve` defaults to `<dump_path>.dqcache` beside the dump, which a
URL has no equivalent of. For a remote source the default is the URL's **last
path segment plus `.dqcache`, in the working directory** —
`https://example.com/d/koji.dump` becomes `./koji.dump.dqcache`. It mirrors the
local rule, it is predictable without introducing a cache directory or a hashed
name the user cannot guess, and `--dqcache` still states anything else. A URL
with no final path segment — a bare host, or a trailing slash — is refused by
name, there being no dump named there to read.

The cache envelope **records the origin it was written for**, and a cache
claimed by a different origin is **advisory**: a diagnostic, never a refusal.
That is the same stance the weak identity takes above and for the same reason —
the maintainer's call is that an origin difference is information, not grounds
for stopping a run. What still refuses is the stored size
([`decisions.md`](decisions.md), "D20"), unchanged.

*Consequence, stated rather than defended away:* two same-named dumps of equal
stored size from different hosts, cached in one working directory, read each
other's map with a warning. The default path makes that reachable and the
recorded origin makes it visible.

### D5 — `--strict-identity` selects which weak signals bind, and absence is a failure

The flag of "Identity: advisory by default, strict on request" above takes a
comma-separated selection, because **data moves**: a dump legitimately copied
from one host to another is a different *location* holding the same bytes, and an
operator who wants the bytes guarded does not thereby want the move refused.

- `--strict-identity=time` binds the **modification signals**: the local mtime,
  and remotely `Last-Modified` and the ETag.
- `--strict-identity=location` binds the **origin** the cache records (D4),
  promoting that diagnostic to a refusal.
- `--strict-identity` bare is `--strict-identity=time,location`.
- `--strict-identity=none` binds nothing, **and is the only way to turn off the
  in-flight check of D10**, which is on by default and is not one of the
  selectors — it is not a question of whether to trust a weak signal, but of
  whether a read whose bytes changed underneath it may be believed.

Under a selected term, **absence is a failure**. A cache that recorded an mtime
against a source that now offers none, or a server sending neither
`Last-Modified` nor ETag, stops a run that asked for `time`: the user asked for a
guarantee and the honest answer is that the source cannot give one, silence being
exactly what strict exists to prevent. Because `object_store` substitutes the
Unix epoch where a server sends no `Last-Modified` and so cannot report absence
("What the backend actually does"), **the epoch is read as absence** — no dump
was modified in 1970.

The library owns the switch, because the library owns the refusal: it rides on
`CacheMode` beside the path, so an embedder gets the same comparison without
re-implementing it, and the flag only sets it.

### D6 — The oracle is a misbehaving HTTP server in-process, and the feature is default-off

**The interesting failures are all server behaviours**, not our arithmetic: a
server that ignores `Range`, one whose ETag changes between the probe and the
read, one that short-reads, one that fails mid-body, one that 404s mid-scan, one
that accepts the connection and then stalls. None is reachable against a
well-behaved server, which makes "serve a fixture over nginx" the weakest
available test. So the oracle is a **small HTTP server inside the test binary**,
serving the existing fixtures over loopback with a knob per misbehaviour:
deterministic, no dependency, no container, no `mise` tool, and the only
instrument that can produce the failures this phase has to get right.
`object_store`'s in-memory store sits beside it for unit-level seams and is not
the oracle — it never speaks HTTP.

**The stall is the knob D9 and D16 are written against**, and it lands with the
source that first meets it. D9 keeps the crate's 30-second total deadline and its
ten retries under a three-minute retry deadline; D16 reworked cancellation into
an awaitable form because a Ctrl-C under those retries waits that long. Neither
condition is reachable by truncating a body — a stalled origin answers slowly or
not at all, which is what a deadline and a cancellation race are timed against.

*Rejected: an off-the-shelf origin*, container or pinned binary, with a proxy for
the transport failures. Two knobs cannot be emitted by anything that ships: an
**honest short 206**, whose `Content-Range` describes the smaller span it
actually sent, and a **declared `Content-Length` the body then contradicts**,
which is what a correct server is built to prevent and which no byte-cutting
proxy can produce, being unable to rewrite the header it truncates under. So
socket-level code exists either way — and once it does, the **control must be the
same implementation as the treatment**: a knob-off oracle is a correct origin,
and reading a misbehaving case against a *different* server's baseline credits
the knob with what may be the implementation. `Oracle::requests()` is the other
half, since D2's "the probe cost one round trip" and D11's "the precondition rode
on every ranged GET" are assertions about the request stream, which off the shelf
becomes access-log parsing. **It generalizes to nothing** — the justification is
those two behaviours and the shared control, not a preference for writing over
depending.

*Rejected: a mock-server dev-dependency* (`wiremock`, `httpmock`), which is what
this entry first described. It reaches every knob but the contradicted
`Content-Length`, and its responder still hand-writes the `Range` parsing and
slicing, so it trades the HTTP framing for a second server implementation in a
workspace whose one binary is about to gain `object_store`'s.

**Reopens:** the risk a bespoke oracle carries is that it encodes our own
misreading of HTTP, which no knob can surface. That is answered by an **opt-in**
conformance test against a real static origin, absent from the default suite —
[`out-of-band.md`](out-of-band.md), `M118`.

**The feature is `http`, default-off in the library and enabled by the CLI.**
Default-on was considered and refused on the measured cost: 120 crates and an
`aws-lc-sys` C build ("What the backend actually does"), which is too much to
impose on every embedder by default. The coverage hole that argument was made
against is closed differently — **the oracle tests live in the CLI crate**, which
enables the feature, so `cargo test --workspace` runs them unconditionally with no
reliance on feature unification and no silent skip
([`roadmap.md`](roadmap.md), "A test may assume the tools `mise` pins"). The
library's own unit tests sit under `#[cfg(feature = "http")]` behind that same
guarantee.

### D7 — Every advisory trait member is answered conservatively and marked unmeasured

`ByteRangeSource`'s advisory members default, and **defaulting is itself a
decision** — a source silent about `retained_unit` is charged a batch span on top
of its read size, and one silent about `hint_read_size` loses pooling without
saying so. This phase measures nothing, so it answers each deliberately at the
conservative setting and names the phase that will price it:

- **`default_workers` = 1.** A plain remote source has no block structure to cut
  at, and concurrency here is tuning.
- **`partitions` declines to advise**, taking the default single partition and
  with it the conservative `ReadChunk` retained unit and its batch-span charge.
- **`default_worker_memory` recommends nothing**, which is precedent: the plain
  local source recommends nothing today (`KD25`).
- **`hint_read_size` stays the no-op default.** Here that is *correct* rather
  than merely defaulted — the backend hands back its own `Bytes` and this source
  recycles no buffer for a pool to keep.
- **`size_is_exact` is true**, and **`stored_size == size`**: one `head`, or one
  206's `Content-Range`, answers both.

The ranged-GET size is therefore whatever the caller's chunk size is — 1 MiB,
measured on local devices and very likely wrong for a network. It is deferred by
name, not guessed at.

**This source is the first that could falsify `MEMORY_UNPOOLED_BOUND`**, an
in-flight HTTP body being held outside every pool this crate owns. This phase
does not measure it; it records that the constant's next reader is here.

### D8 — The dependency is `object_store`'s own `http` feature, and `mise` pins what it builds

`http` resolves to rustls backed by `aws-lc-rs`, whose `aws-lc-sys` is a C build
needing cmake. Taken as it ships: it is the crate's supported path, it needs no
crypto-provider wiring of our own, and with the feature default-off (D6) the cost
falls on whoever opted in. **`cmake` is added to `mise.toml`** in the slice that
adds the dependency, the project's own rule being that a checkout declares the
tools its build needs.

*Rejected: `reqwest/native-tls`*, which trades a cmake build for a system OpenSSL
dependency that is harder to pin. *Rejected: `http-base` + `reqwest` +
`reqwest/rustls-no-provider`*, which saves about 37 crates and buys a piece of
process-global crypto-provider initialization to get wrong. If the C toolchain is
ever unacceptable in a target environment, both escapes are documented and the
change is a manifest line.

### D9 — Four client defaults: one is overridden, three are kept, none becomes a flag

- **The total request timeout is disabled and replaced by a read timeout.**
  `object_store` defaults `timeout` to 30 s *including the response body*, which
  is a limit on link speed rather than on liveness: a single ranged GET whose
  body is slow fails even while progressing. `read_timeout` is per-read and
  resets, so a slow-but-progressing transfer completes and a dead connection
  still fails. Its value is chosen to be defensible and is **unmeasured**, like
  every other constant this phase sets.
- **`allow_http` is set when the scheme is `http`**, with no warning: typing
  `http://` is the user's statement that plaintext is acceptable, and D3 accepts
  the scheme.
- **The retry configuration is the crate's**, unchanged — 10 retries, a
  three-minute deadline, 100 ms doubling to 15 s, over 5xx/429/408 and connection
  errors. Changing it is tuning.
- **`http1_only` is left true**, on the crate's own measurement.

**None of these becomes a flag.** [`roadmap.md`](roadmap.md), "Two tunables fit
pgdq to hardware" governs what a person must state; a timeout knob is scope here,
and the phase that tunes the network is where one would be justified by a reading
rather than by a preference.

### D10 — A source that changes under an in-flight read is an error, on every provider

The identity questions split by tense. *Between* runs the weak identity is
advisory, because data moves. *During* a run it is not: bytes changing underneath
a read that has already returned some of them cannot produce a right answer, and
the failure is silent — a map or a row set mixed from two versions of the file.
So **a detected identity change while work is in flight aborts the run**, by
default, on the local source as well as the remote one. `--strict-identity=none`
is the only opt-out and makes it a diagnostic.

Each provider enforces it where enforcement is cheapest, and the two mechanisms
are not the same shape:

- **Remotely the server enforces it.** The ETag (or `Last-Modified`) taken at the
  origin probe is sent as an `if_match`/`if_unmodified_since` **precondition on
  every subsequent ranged GET**, so a rewrite mid-scan comes back as a 412
  instead of as mixed bytes. This holds **cold as well as warm**: with no cache
  to compare against, the pin still means "the object may not change under us",
  which makes the rule one rule rather than two.
- **Locally the open file descriptor is re-checked.** An `fstat` on the fd the
  source already holds is the analogue, and it is the *right* analogue rather
  than merely the available one: it detects the dangerous case — the file
  modified in place, truncated or rewritten — and ignores the harmless one, a
  file replaced by rename, where the descriptor goes on reading the intact inode
  it opened. A cadence for that check is owed (D11).

*Superseded: treating a mid-scan rewrite as a property with the user's remedy
already in hand.* That was the recommendation when the flag was read as being
about stale caches alone; it does not survive the split above, since a local file
rewritten mid-scan has been silently wrong all along and nobody chose that.

### D11 — The local check rides the save throttle; the remote one rides every request

**Locally the `fstat` happens at cache save and nowhere else.** That operation is
already throttled — [`decisions.md`](decisions.md), "D62"'s ratio gate spends a
bounded fraction of the scan on saving — so the check inherits a cadence this
project has already tuned, and **nothing is added to the read loop**. A per-read
check was considered and refused on exactly that ground: it is a syscall added to
a loop kept device-bound, in a phase that has promised to take no measurement
that could defend it.

**Remotely the precondition stays on every ranged GET**, because there it costs
nothing: the check rides on a request the read was already making, and the server
does the comparing. So the two providers detect at different granularities, and
that asymmetry is deliberate rather than an oversight — **the cadence follows the
cost of asking**. Writing the rule as uniform would be false: remotely a change
is caught on the first read after it happens, locally at the next save.

**A run that never saves is checked once, when it finishes.** `--dqcache none` is
one such run, and a warm `pgdq query` over a complete cache is the other — which
is the very case this phase is designed around, since a warm query does no
mapping pass and therefore reaches no save. One `fstat` at the end of the run
closes both: it is off the read loop entirely, and it means a local run cannot
exit successfully having read a file that changed underneath it. By then the rows
are emitted, so what this recovers is a non-zero exit naming the cause in place
of a silent wrong answer — which is the most that is recoverable at that point,
and more than nothing.

### D12 — An abort saves nothing and deletes nothing

The check tells us *when the change was detected*, never when it happened, so
every byte this run read is suspect.

- **The run saves nothing** — not the partial map, not statistics.
- **The cache already on disk is left exactly as it is.**
  [`decisions.md`](decisions.md), "D20" says the library never replaces cache
  data automatically, and that binds deleting as much as overwriting. The
  pre-existing cache describes the file as it was; if the new file differs in
  stored size the next run refuses it in D20's voice, and if it does not, the
  weak identity reports it at whatever strictness was asked for.
- **The error names the source, says the file changed while it was being read,
  and says that nothing was saved** — because the user's next move is either to
  re-run against a settled file or to ask why their pipeline rewrites dumps in
  place.

### D13 — `xz-seek` is vetted here, and the seam is fixed upstream rather than bridged here

This phase is the crate's remaining consumer, the statistics phase having turned
out to make no call into it.

**The "two real consumers" gate was ours, not theirs.** That crate's record
carries no such condition — not in its standing decisions, not in its register,
not in its frozen handoff — which its maintainer's session reported on reading
them. So nothing there is narrowed by what we stop calling, and the only written
condition is our own maintainer's: publish once gzip and zstd are supported.

**What this consumer exercises narrows with D20** all the same: after 14.8
nothing here holds an `xz_seek::Reader`, so we vet the walk machine, `Layout`
and the block handle. That driver keeps consumers of its own — their CLI, their
differential sweep against `xz -dc`, and two of their figures — so what changes
is that *our* review stops exercising it, which is worth stating rather than
assuming. Keeping a path we had just found redundant in order to exercise a
convenience wrapper would be the wrong way round.

**The composition is not free, which 14.5 established by reading the code.**
This entry said it was, on the strength of `XzSource` wrapping a
`ByteRangeSource`; it does not. `XzSource` holds an
`xz_seek::Reader<std::fs::File>` (`pgdump_query/src/io.rs`), and the crate reads
through `CompressedSource` (`vendor/xz-seek/src/source.rs`), a **synchronous**
positional trait, where `ByteRangeSource` is async.

**What that costs is not fetch policy, and 14.7 writes no bridge.** The crate's
backfill window is a floor on what its walk *requests*, not a ceiling on what a
caller may fetch, so a caching `CompressedSource` of ours could coalesce and
speculate underneath it today. What is irreducible is that nothing can `await`
inside `read_at`. The seam a remote source needs is therefore sans-IO, and it
belongs in the crate, which already states that contract for a block:
`BlockTask` carries a resolved check and decodes out of a caller's `Window`, so
the block path needs no source of ours at all.

**The crate is taking that as its own phase** — a caller-driven footer walk
constructed with the file size, a resumable block decode rooted on `BlockTask`
rather than on its private incremental form, and a sourceless handle for the
queries that touch no bytes. `CompressedSource` is unchanged by all three, and
our `Origin` probe already holds what the walk needs to start: the stored size it
is constructed with, and the leading magic that is its first request
([`../status/history/2026-09-18.md`](../status/history/2026-09-18.md)).

**14.7 waits for that phase whole**, not slice by slice, and no intermediate
state is vendored. The message it waits on is *every slice has landed*: that
phase stays open across our review, and wraps and keystones on our approval,
after which `scripts/vendor_xz_seek.py` re-syncs and 14.7 proceeds. The vendored
read-only copy stays as [`decisions.md`](decisions.md), "D14" left it.

**Publication waits for gzip and zstd** — for all three codecs to be supported
and to sit well together in one codebase — which is the maintainer's condition
and belongs to the phases that make it true, not to this one.

## What this phase defers, by name

Every remaining inbox entry is **deferred by scope rather than answered**, under
"Correctness only" above. Each is network tuning, and each belongs to the phase
that tunes the network with more than one backend to tune against:

- The **pre-fetched `Window` composition** and the decode-out-of-a-window trade —
  priced and refused for a local file, and reversing over ranged GETs where the
  saving is a round trip per block rather than a `pread` out of page cache.
- The **fetch policy**. *That* this project takes the fetch entirely is settled
  by D13; what is left to tune is its concurrency, coalescing and speculation.
- The **ranged-GET partition size**, which is a property of the network rather
  than of the file, and which multiplies with an `XzSource`'s block boundaries
  rather than being overridden by them.
- **`MEMORY_UNPOOLED_BOUND`**, which this source is the first that could falsify,
  an in-flight HTTP body being held outside every pool this crate owns.
- The **cold footer walk** of D1, whose remedy is the straddling window, and
  whose residual cost is one fetch a stream.

### D14 — The remote source is part of the library's public surface

The **origin** of D2 is public L1, `open_local` keeps its name and meaning, a
sibling constructor takes a URL behind `#[cfg(feature = "http")]`, and a
dispatching entry point over the origin sits above both.

**P6 is why.** That phase presents a `TableProvider` and Python bindings, and it
is scheduled after this one precisely because it commits to the I/O layer beneath
it ([`roadmap.md`](roadmap.md), "P6 — Embeddable engine story"); an embedder who
can reach a remote dump only by shelling out to `pgdq` has been handed half a
library. The cost is that this phase commits to a surface P6 will live with,
which is the ordinary price of going first.

### D15 — A network failure is an error, and the save throttle is what makes it tolerable

A ranged GET can fail after the crate's ten retries and three-minute deadline.
The project has two shapes for a scan that stopped early: **Ctrl-C**, which sets
the cancel flag, saves, marks the run interrupted and is resumable
([`decisions.md`](decisions.md), "D63"); and an **I/O error**, which propagates
and saves nothing. A network failure is the second. Nobody asked for it, and
treating it as a cancellation would make `parse` report partial success on a dump
it could not read.

**No new mechanism is added, because the tolerance is already built.** The save
throttle means the on-disk cache holds progress to the last save and a re-run
resumes from it, so a failure costs one throttle interval rather than the scan.
What the phase does owe is the **message**: it names the URL and the underlying
failure rather than surfacing `object_store`'s wording raw.

## What lands beside the code

Obligations this phase carries that are not decisions, recorded so the slices
inherit them rather than rediscovering them:

- **`runtime-invariants.md` gains entries.** The facts under "What the backend
  actually does" are properties of something outside our control that decisions
  here depend on — the `HEAD`/PROPFIND split, the 206 carrying the total size,
  the range-ignored refusal arriving on the status line, `Content-Length` being
  required, the epoch substituted for an absent `Last-Modified`, and
  `current_thread` sufficing. Each earns an `RT<n>` **in the slice whose decision
  starts depending on it**, with its re-verification command
  ([`../process.md`](../process.md), "The assumptions register").
- **The manual gains a remote section**, in
  [`../manual/dump-inspection.md`](../manual/dump-inspection.md): a URL as
  `--source`, where the cache goes, and `--strict-identity`. User-facing, no
  rationale.
- **`pg-dump-compatibility.md` gains nothing.** That matrix is about `pg_dump`
  variants; where a dump is fetched from is not one.
- **[`decisions.md`](decisions.md), "D26" is amended** by the slice that makes a
  read cancellable (D16) — the entry describes a polled flag, and the mechanism
  gains an awaitable form.
- **A `KD<k>` is allocated for the cold footer walk** (D1) by the slice that
  ships remote `.xz`, owned by the phase that tunes the network.

### D16 — A read in flight is cancellable, because an unresponsive program is an incorrect one

Cancellation is a cooperative `AtomicBool` polled per chunk or leader window
([`decisions.md`](decisions.md), "D26"). Locally that bounds a Ctrl-C by one
`pread`, tens of milliseconds. Remotely it bounds it by the retry configuration —
**three minutes in the worst case** — which is long enough that a user reaches
for `kill -9` and loses the progress the save throttle had banked.

**That is a correctness defect, not a cost.** So the flag gains an awaitable
form: a cancellation type holding the polled bit *and* a signal a future can wait
on, built from `tokio`'s `sync` feature, which the library already depends on, so
no dependency is added. Existing call sites keep polling and are unchanged. The
remote source races its in-flight request against the signal, and dropping the
future cancels the request — which makes the async path **more** responsive than
the local one rather than less.

*Rejected: accepting the wait and naming the bound in the manual.* It documents
a defect instead of fixing one. *Rejected: lowering the retry deadline instead.*
It reverses D9's decision to keep the crate's retry defaults, and picking the
replacement number is precisely the unmeasured tuning this phase said it would
not do — and it would still leave a Ctrl-C waiting tens of seconds.

**Consequence.** This reworks an already-tested core path and changes what a
register entry describes, so it is **its own slice**, it lands before the backend
needs it, and it amends [`decisions.md`](decisions.md), "D26" in the change that
lands it. The local source is untouched: a blocking `pread` cannot be interrupted
mid-read and does not need to be.

### D17 — Every `http(s)` URL is plain HTTP, `file://` is a local path, and no credential is handled

**"Not S3" is about the backend, not the URL.** `object_store::parse_url`
classifies `*.amazonaws.com`, `*.blob.core.windows.net` and
`*.r2.cloudflarestorage.com` as S3/Azure/R2 **by hostname**; this phase does not
call it. The store is built from the whole URL with an empty object path, a form
that **preserves the query string** — so a **public S3 object URL works, and so
does a presigned one**, the signature in the query surviving intact. A presigned
URL is the natural way to read a private object with no credential handling at
all, and it costs this phase nothing.

**No credentials.** No basic auth, no bearer token, no region resolution. A URL
carrying `user:pass@` userinfo is **refused by name** rather than silently
dropped, so nobody believes a password was sent.

**`file://` is accepted and resolves to the local source.** A user who has seen
`--source https://…` work may reasonably conclude that a local file now needs
`file://`, and being right about how the tool works should not be a way to get an
error. An empty or `localhost` host is the local path, percent-decoded; any other
host is refused by name, since this phase speaks no network file protocol.

### D18 — One identity enum, one refusal, one name that means something on both providers

`SourceIdentity` gains a remote variant carrying **`{ origin, etag,
last_modified, stored_size }`**: the origin is D4's, the ETag and `Last-Modified`
are the weak signals D5 governs, and the stored size is what still refuses.

D20's refusal keeps **one wording**: `Error::CacheSourceMismatch`'s two sizes are
unchanged, because the size check works identically over a remote source, and its
`path` generalizes to **the source's display form** — a path locally, the URL
remotely. The inbox's alternative, a second refusal beside the first for an
identity with nothing numeric in it, is refused: the numeric part is not missing,
only the field that named the file.

### D19 — Origin is a remote concept; a local source records none

The hazard D4's origin exists to make visible is specific to the remote default:
two same-named dumps from different hosts colliding on one `./name.dqcache` in a
working directory. **A local cache has no such hazard** — its default sits
*beside* the dump, so the pairing is the filesystem's rather than a name we
derived. So a local cache records no origin, `location` binds nothing locally,
and a dump moved together with its cache stays as silent as it is today.

*Rejected: recording the canonical path as a local origin.* It buys the symmetry
of one rule across both providers, and charges every user who relocates a dump an
advisory diagnostic on every run thereafter — for a case nobody considers a
problem. Suppressing it where the cache sits beside the dump would rescue that,
at the price of a special case written into the comparison.

The asymmetry is real and is stated plainly in the manual rather than papered
over: `location` is about where a remote object was fetched from, and a local
source has none.

**Reopens:** feedback from users who want a local path checked. That arrives as a
third selector, `--strict-identity=path`, rather than by changing what `location`
means.

### D20 — One mechanism reads inside a block, with two sources, and its window is charged

A read landing at an arbitrary offset inside a block is served two ways today:
by `xz_seek::Reader::read_at` where the stated budget cannot hold a decoded
block, and by a decoded block's own slice where it can. The first exists only
because a local file can be pulled from, and a remote source cannot be. So 14.7
moves it: **one mechanism, `xz-seek`'s resumable block handle, with the source
being the difference** — a `File` locally, which the crate pulls from, and a
`Window` over the block's compressed range remotely, which we fetch. How bytes
reach a decoder is a property of the transport and is the one asymmetry worth
keeping; *what got verified* is not, and today it differs, `Verify::Full`
draining a partly-read block for the local path while the handle makes
completion a call we make.

**Locally the crate pulls and nothing is materialized**, which is what
`CompressedSource` is for and which keeps the charge below a remote-only term.
The cost is that the window-fed branch would have no local twin to disagree
with, so a local file's block is put through the window-fed path **in tests
alone** — the shape that crate's own lending axis already takes, so a divergence
surfaces as the right disagreement rather than as two sources differing. It
needs nothing of them: `BlockTask::compressed_range()` and `Window::new` are
public, and their own phase owes the same agreement as evidence.

**The parallelism this does not take.** The streaming arm advises one partition
because one reader sits behind a mutex (`pgdump_query/src/io.rs`), and per-reader
handles would let several decoders run where the blocks they decode would not
fit. That is a throughput claim and this phase produces no figures ("Scope"), so
the advice is unchanged here and the gain is `KD35`, owned by the phase that
takes the figure. It also binds upstream: the handle must stay usable N at a time
for that phase to exist at all, which is why `BlockRead` is asked to be `Send`
and to borrow nothing (D13).

**A block is always completed, and that is not a knob.** Completion compares the
check by draining the block's remainder, which is what `Verify::Full` already
does for the path being replaced — so always-completing is today's guarantee
carried over, not a new cost. What is new is only that the call is ours, and a
flag to skip it would have silently weaker verification as its failure mode. A
reader that touches a few kilobytes of a large block pays a decode of the rest;
if that ever shows up in a profile it becomes a figure and a decision then.

**The compressed window is charged against the stated budget**, not booked as
unpooled. `BlockTask::compressed_range()` gives its length before the fetch, so
it is a term we can price exactly — unlike an in-flight body of unknown length,
which is what `MEMORY_UNPOOLED_BOUND` was left to absorb. A term that can be
priced and is not is the falsification of that bound rather than an instance of
it.

## How it is sliced, and why in that order

The rows are [`../status/STATUS.md`](../status/STATUS.md)'s checklist; the
reasoning for their order is here.

**Two core-path reworks land alone and first.** Making a read cancellable (D16)
and making the pre-source probe an origin (D2) each rework a path the suite
already covers, and bundling either with new-module work would force one review
to accept both at one confidence ([`../process.md`](../process.md), "Size a slice
by its review, not by its scope"). They also both precede the backend that needs
them, so the backend is written against a seam that already exists rather than
one it invents.

**The identity semantics are proven on the provider we control absolutely.** The
in-flight check, the flag and the abort (D5, D10–D12) land against the *local*
source first, where a test can rewrite a file mid-scan with no server in the way.
The remote half then inherits semantics that are already pinned, and what it has
to get right is narrowed to the mechanism — a precondition on a request.

**The instrument precedes its subject.** The misbehaving HTTP server (D6) lands
before the source it exists to test, with its own tests proving it misbehaves as
asked. An oracle written alongside the thing it judges is an oracle shaped to
agree with it.

**The remote work then goes bytes, identity, compression** — a plain dump read
end to end, then what the cache records about it, then the `.xz` composition —
each row being a thing that can be demonstrated whole against the oracle.

**The local path migrates last, against this phase's other ordering rule.**
Proving semantics on the provider we control absolutely is why the identity work
went that way round, and it does not bind D20's mechanism swap, which has no
semantics of its own to pin: what binds instead is that a tested path is never
migrated onto a mechanism that has not yet run anywhere. So 14.7 proves the
handle where no alternative exists, and 14.8 moves the local path onto one
already in use.
