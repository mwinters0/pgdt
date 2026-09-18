# P14.5 — The remote source

What landed: `io::RemoteSource` over `object_store`'s HTTP backend behind the
default-off `http` feature, `Origin::remote`/`Origin::resolve` and the remote
probe, `open` dispatching over `open_local` and `open_remote`, two errors
(`SourceNotReadable`, `Remote`), `ByteRangeSource::hint_cancellation`, and
`--source <url>` on all three commands. The oracle gained its stall knob,
[`runtime-invariants.md`](runtime-invariants.md) gained `RT13`–`RT17`, and
[`../manual/dump-inspection.md`](../manual/dump-inspection.md), "Reading a dump
over HTTP" is the user-facing half.

## What the rest of the phase inherits

- **`Location::Remote` carries the client, not just the URL.** One
  `HttpStore` per origin, built in `Origin::remote` and handed to the source by
  `open_remote`, so the probe's round trip and every later read share a
  connection pool and cannot disagree about timeouts. 14.6's precondition on
  every ranged GET is a `GetOptions` field on the two call sites in
  `RemoteObject::probe` and `RemoteSource::get`, and nothing else has to move.
- **`RemoteSource` caches size and weak identity from the probe**, so a read
  costs one round trip and a `size()` none. The consequence to carry forward:
  `SourceWatch` re-reads those two through the trait, so **remotely it compares
  a value with itself and a run has no in-flight identity check at all until
  14.6**. That is the slice order rather than a defect — remotely the check is
  the *server's*, a precondition on every ranged GET, at a finer cadence than
  the save throttle ([`roadmap-P14-remote-input.md`](roadmap-P14-remote-input.md),
  "D11") — but it is a gap while it lasts. The between-runs checks are
  unaffected: `claim`'s stored-size refusal and the weak-identity report both
  read the probe.
- **`Origin::resolve` is the whole of D3 and D17**, and it is `#[cfg(feature =
  "http")]`: the URL concept arrives with the backend, so a library built
  without the feature keeps today's dependency set and `Origin::local`. Every
  refusal is `Error::SourceNotReadable`, which names the argument as the user
  wrote it.
- **The cache path is *asked for* remotely, not derived.** `main.rs`'s
  `cache_mode` refuses a remote source with no `--dqcache`, naming the flag.
  That refusal is 14.6's to delete when D4's URL-basename default lands; it is
  the one user-visible contract here that is known to be temporary.
- **A remote `.xz` is refused by name.** `XzSource` opens a *path* — it is not
  built over a `ByteRangeSource`, whatever the composition reads like from the
  spec — so 14.7 is real work: either `XzSource` grows a constructor over a
  source, or the remote case gets its own. The refusal sentence is in
  `open_remote` and is what 14.7 replaces.

## Negative results

- **The deadline needed a second constructor to be assertable at all.**
  `Origin::remote_with_read_timeout` exists because the shipped
  `REMOTE_READ_TIMEOUT` is minutes of wall clock to a test: a stalled origin is
  abandoned only after the read timeout, and then retried ten times under a
  three-minute deadline. Every cheaper route was worse — a shipped value small
  enough to test is a value chosen for the test, and an `#[ignore]`d assertion
  is the "left unproducible" the row refuses. It is under "Decisions worth
  another look".
- **The cancellation is a hint, and a cancelled remote read is an error.**
  14.1 left the choice between a constructor argument and a hint open; the hint
  wins because `ScanOptions` is where the cancellation lives and the hints are
  already announced from there. The consequence is that a dropped request comes
  back as `Error::ScanCancelled` rather than as `MapStop::Interrupted`, so
  `parse` translates it in `main.rs` rather than the library's read loops
  learning a new shape. Also under "Decisions worth another look".
- **The stall knob is addressed to one request, not to a suffix.** Every other
  oracle knob is `_after(n)`. A stall cannot be: what it is used to observe is
  what the client does *next*, and a suffix knob stalls that attempt too. It
  also had to end when the client hangs up — the oracle serves one connection at
  a time, so a stall outliving its client would hold the very retry the test is
  waiting for.
- **`object_store` refuses a short 206 before the body is read**, so
  `RemoteSource::read_range`'s own length check can only ever fire at the end of
  the object (`RT15`). The two truncation knobs are still both exercised,
  because the crate reaches that answer by two different routes.
- **No `D<k>` was added.** `decisions.md` is two lines under its cap and the
  spec's D1–D19 hold this slice's reasoning; `D6`, `D14` and `D26` already
  describe what the code now does — "a remote source is additive behind a
  feature", recognition told its bytes by the probe, and a reader that drops a
  request rather than polling past a retry schedule — so an entry here would
  restate rather than record.
- **An empty remote object cannot be probed.** A ranged GET of the leading
  bytes against a zero-length object is a `416`, where the local probe reads
  zero bytes and calls the file plain. Left alone: an empty file is not a dump,
  the failure names the URL, and the alternative costs a second round trip on
  the warm path the probe exists to keep to one.
