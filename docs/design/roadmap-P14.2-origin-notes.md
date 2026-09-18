# P14.2 — The origin

What landed: `io::Origin` and `io::OriginProbe`, public L1, plus the two
signature changes they exist for — `cache::claim` is `async` and takes an
`&Origin` in place of a `&Path`, and `open_local` takes one in place of a
path and is `async` with it.
[`decisions.md`](decisions.md), "D14" is amended in the same change. No remote
code, no flag, and no change to any command's output.

## What the rest of the phase inherits

- **The seam a remote source plugs into is `Location`**, a private enum inside
  `io.rs` with one variant. 14.5 adds its second there and a constructor beside
  `Origin::local`; nothing outside the module names it, because what a caller
  reads is `OriginProbe`, which is the same three answers whichever kind
  produced them. `Origin` is what D14 of the spec calls public L1, and the
  dispatching entry point it names sits above `open_local` rather than
  replacing it.
- **One probe, cached, is the contract.** `Origin::probe` is the only route to
  an answer and `tokio::sync::OnceCell` holds it, so the cache claim and
  recognition see one consistent set of answers and a source whose probe is a
  round trip pays for one. A *failed* probe is deliberately not cached: `claim`
  swallows a probe failure into `KnownCompression::Unknown` exactly as it
  swallowed a failed `stat`, and the open that follows is still where that
  failure has a sentence to say. Both properties are asserted
  (`an_origin_probes_at_most_once`, `a_failed_origin_probe_is_not_remembered`).
- **`ORIGIN_LEADING_BYTES` is `XZ_MAGIC.len()`** — the longest magic
  recognition compares, not a round number chosen against formats nobody
  reads yet. A gzip or zstd slice that needs fewer is already covered; one that
  needs more raises the constant and nothing else.
- **`Origin: Display`** is how a message names a source, and it is what 14.6's
  generalization of `Error::CacheSourceMismatch`'s `path` field will want. The
  CLI's `cache_written_for_another_file` already takes one.
- **`OriginProbe::modified` has no in-tree reader yet**, which is the state the
  row predicted rather than a gap: the probe is committed to answering the weak
  identity, and 14.3 is where a caller compares one. It is exercised by
  `an_origin_probe_answers_size_identity_and_the_leading_bytes` alone. What 14.3
  still has to decide is *which* observation its in-flight check reads — this
  one is taken before the run, where D10's check compares against the
  descriptor the source already holds, on purpose. The origin is not that
  mechanism and does not grow into it.

## Negative results

- **`CacheMode::resolve` still takes a `&Path`**, and was left alone. D4's
  URL-basename default is 14.6's row, and moving the resolver onto the origin
  here would have put a decision that phase owns into a slice that has no
  remote variant to test it against.
- **The probe does not reuse `ByteRangeSource`.** The trait's `stored_size` and
  `modified` answer the same two questions, and a source is exactly what must
  not exist yet: reaching them means opening the thing whose opening the early
  refusal exists to spare (`decisions.md`, "D20"), and for a many-stream `.xz`
  that is the footer walk. So the probe is its own path, and `OriginProbe`'s
  accessors carry the trait's names rather than its methods.
- **Recognition kept `Recognized`, its two variants and every one of its
  tests.** Only where the magic comes from changed, which is why the diff is a
  signature change and a deleted `is_xz_by_magic` rather than a rewrite of a
  path the suite already covers.
- **No `D<k>` was added.** The origin's shape is D14's sentence about where
  recognition's magic comes from, amended in place; the register is within a
  handful of lines of its cap, and a second entry restating the first is what
  that cap exists to refuse.
