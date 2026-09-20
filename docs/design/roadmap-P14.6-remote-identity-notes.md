# P14.6 — Remote identity and the remote cache

What landed: `cache::SourceIdentity::Remote` and `cache::OriginMatch` beside
`WeakIdentity`, `ByteRangeSource::remote_identity` and
`hint_in_flight_identity` with `io::RemoteIdentity`, the identity precondition
on every ranged GET, `CacheMode::resolve` over an `Origin` with D4's
URL-basename default, two diagnostics (`CacheEntityTagChanged`,
`CacheOriginChanged`), and `Error::StrictIdentityUnmet` naming which term it
refused on. `CACHE_FORMAT_VERSION` is 23.
[`runtime-invariants.md`](runtime-invariants.md), "RT18" is the external
behaviour the precondition rests on, and
[`../manual/dump-inspection.md`](../manual/dump-inspection.md), "Reading a dump
over HTTP" is the user-facing half.

## What the rest of the phase inherits

- **Three signals, not two variants.** `SourceIdentity`'s comparisons read
  *origin*, *modification* and *stored size* through private accessors rather
  than matching a variant pair, so a cache written for one kind of source and
  read against another is compared rather than refused — which is how a local
  cache read over a fetched source reports an origin difference instead of
  panicking on an irrefutable `let`. A future variant adds accessors, not a
  combinatorial match.
- **`hint_in_flight_identity` is announced by `SourceWatch::open`**, which is
  the one place that already holds both the source and the strictness. **The
  default is the binding one**, because the hint arrives after the origin probe
  and the cache claim have already read bytes; `--strict-identity=none` is the
  only thing that ever turns it off. 14.7's `XzSource` over a remote source
  inherits this unchanged — the precondition rides on `RemoteSource::get`, so
  every fetch a footer walk makes is pinned without the walk knowing.
- **`RemoteObject` keeps the probe's whole `ObjectMeta`.** The precondition is
  built from the validators as the server stated them, which is what avoids
  naming `chrono` — `if_unmodified_since` wants a `DateTime<Utc>`, and a
  `SystemTime` round trip would have meant a direct dependency on a crate only
  `object_store` and `arrow` pull in today.
- **The etag outranks `Last-Modified` where both sides carry one**, which is
  the precedence RFC 9110 gives conditional requests and the one the oracle
  already implements. D5 names both signals without ordering them; this is the
  refinement, and it is what makes a server that keeps a stale `Last-Modified`
  across a rewrite still report the change.
- **`cache::advisory_identity_diagnostics` is public for the same reason
  `strict_identity_refusal` is**, and it is the same caller: `pgdq info` reads
  the whole `CacheStatus` through `cache::load` and so never passes through
  `CacheMode::load`, where both the refusal and the two warnings are raised.
  Routing three `WeakIdentity` states and an `OriginMatch` onto diagnostics in
  two places is a drift nobody would notice.
- **`CacheMode::resolve` takes an `&Origin`**, so the URL-basename default is
  the library's rule and `pgdq` only passes the flag through. The CLI's
  `cache_mode` helper is gone with it. A URL that names no object is refused
  when the *origin* is built, not when the cache path is derived: a bare host
  or a trailing slash is not a dump whatever `--dqcache` says.

## Negative results

- **`CacheSourceMismatch` did not grow a field**, and the spec's D18 was
  corrected instead. The entry said its `path` would generalize to a source's
  display form; the field is the **cache's** path and always was
  (`pgdump_query/src/cache.rs`, `CacheMode::source_mismatch`). The source's
  name is added by the CLI's `naming_the_source`, which already did exactly
  that for the in-flight refusal — the division 14.3 recorded and expected this
  slice to either fold into or leave alone. Reviewed since and affirmed, with
  the classification widened to every refusal that fires because the source
  moved and shared by all three commands (`M127`).
- **`CacheMtimeChanged` kept its wording, and a third kind was added.**
  Widening it to "the modification signal" made the local sentence vaguer for
  every user who has no entity tag, and the comparison already distinguishes
  the two cases (`WeakIdentity::Differs` against `TagDiffers`), so there was a
  payload to route on rather than a wording to blur.
- **`SourceWatch` still compares a remote source with itself**, as 14.5's notes
  predicted, because `RemoteSource` answers size and identity off the probe.
  That is not a gap any more: the run's in-flight check remotely *is* the
  precondition, at a finer cadence than the watch's, so the watch's own
  re-observation is the local provider's mechanism and remains inert on the
  other one.
- **No `D<k>` was added.** The spec's D4, D5, D10, D11, D18 and D19 hold this
  slice's reasoning and `decisions.md` is within a couple of lines of its cap;
  the two calls this slice made that they do not cover — the tag's precedence
  and the third diagnostic kind — are recorded above, beside the mechanisms
  they govern.
- **A local cache still records no origin**, so `location` binds nothing there
  (D19). `location_binds_nothing_on_a_local_source` passes unchanged, and the
  new `a_local_cache_read_over_a_fetched_source_differs_in_origin` pins the
  other half: absence is a positive statement, so two absences agree and one
  absence against a URL does not.
