# P14.3 — In-flight identity on the local source

What landed: `cache::StrictIdentity` riding on `CacheMode`, `cache::SourceWatch`
and `cache::WeakIdentity` beside it, two new errors
(`SourceChangedWhileRead`, `StrictIdentityUnmet`), and `--strict-identity` on
`pgdq parse` and `pgdq query`. [`decisions.md`](decisions.md), "D21" is amended
in the same change, and the manual gains
[`../manual/dump-inspection.md`](../manual/dump-inspection.md),
"`--strict-identity`: when a moved file should stop the run". No remote code.

## What the rest of the phase inherits

- **The strictness rides on `CacheMode`, which is now two struct variants.**
  `Enabled { path, strict }` and `Disabled { strict }`; `Offline` carries none,
  having no live source to compare against. `CacheMode::enabled(path)`,
  `CacheMode::DISABLED` and the unchanged `resolve` all leave
  `StrictIdentity::ADVISORY`, and `with_strict_identity` is the only thing that
  states otherwise — so an embedder gets today's stance by construction. The
  rename is most of the diff: it is mechanical, and no test's subject changed.
- **`SourceWatch` is the whole of D10/D11, and 14.5 adds no second mechanism.**
  It holds the identity the run opened on and re-reads it through the
  `ByteRangeSource` the run already holds. A remote source answers
  `stored_size`/`modified` off its own probe, so the same type compares ETag and
  `Last-Modified` once `SourceIdentity` gains its remote variant (14.6) — what
  14.5 owes is the *precondition on every ranged GET*, which is a second, finer
  cadence beside this one and not a replacement for it.
- **Three check points, and no fourth.** `CacheMode::save` checks before it
  writes, which is the throttled cadence; each of `map_file`, `preamble_only`,
  `table_stream` and every sub-stream of `table_stream_partitions` checks once
  when it finishes. Nothing is in the read loop. A sub-stream checks for
  itself rather than the set checking once, since each is a reader that ends.
- **`WeakIdentity` replaced `CacheStatus`'s `mtime_changed` bool** because
  `time` refuses on two of its three states: absence is a failure, and a `bool`
  cannot say "neither side has one". `pgdq info` reads it and keeps reporting
  rather than refusing.
- **`StrictIdentity::location` is parsed, carried and read by nothing.** A local
  cache records no origin (spec D19), so it binds nothing until 14.6 gives one.
  `location_binds_nothing_on_a_local_source` pins that as a property rather than
  leaving it untested.

## Negative results

- **The free `cache::save` kept its three arguments.** The check lives in
  `CacheMode::save`, which is what every scan entry point calls; putting it in
  the writer would have made the watch an argument at twenty test sites whose
  subject is the encoding, and bought a guarantee against a caller that has
  opted out of run management altogether. The cost is one extra identity
  observation per save, which the throttle already bounds at a small fraction of
  scan time (`decisions.md`, "D62").
- **`pgdq info` has no `--strict-identity`.** It never scans, so there is no
  in-flight window, and its job is to *report* what the cache holds — including
  that the modification time moved. A flag that made it refuse would remove the
  one command that can tell you why. Filed under STATUS's "Decisions worth
  another look": nothing in the spec settles it.
- **No `RT<n>` was added for the descriptor-keeps-its-inode property.** The spec
  names the `RT` entries this phase owes and they are all `object_store`'s;
  this one is POSIX and is established here by a test that renames a file over
  an open source's path (`a_dump_replaced_by_rename_under_an_open_source_is_not_a_change`),
  which is the check a register entry would otherwise be standing in for.
- **No test races a rewrite against a scan** (`decisions.md`, "D73"). The
  identity moves in the *source*: `tests/identity.rs`'s `Shifting` wraps a real
  `LocalFileSource` and answers a different identity from a chosen observation
  onward, which is exactly what the library sees through the trait. The one
  filesystem property is tested against the filesystem.
- **The source's name is added by the CLI, not carried by the error.** The
  library raises `SourceChangedWhileRead` from inside the save, where the only
  name in hand is the *cache*'s; `main.rs`'s `naming_the_source` puts the
  `Origin` the user typed in front of it, which is the same division
  `cache_written_for_another_file` already uses. 14.6 generalizes
  `CacheSourceMismatch`'s `path` to a source's display form and may fold this
  in; nothing was added to `ByteRangeSource` to pre-empt it.
- **The per-read `fstat` rejection stays in the phase spec.**
  [`roadmap-P14-remote-input.md`](roadmap-P14-remote-input.md), "D11" holds it;
  the register is two lines under its cap and D21 carries the conclusion, so the
  reasoning is harvested at the keystone rather than duplicated now.
