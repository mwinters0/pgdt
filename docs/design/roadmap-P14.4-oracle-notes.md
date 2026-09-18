# P14.4 — The oracle

What landed: `pgdump_query-cli/tests/common/oracle.rs`, a misbehaving HTTP/1.1
origin server in-process, and `pgdump_query-cli/tests/oracle.rs`, the twenty
tests proving it misbehaves as asked. No library code, no dependency, no
feature, no flag, and no subject — deliberately
([`roadmap-P14-remote-input.md`](roadmap-P14-remote-input.md), "How it is
sliced, and why in that order").

## What the rest of the phase inherits

- **The oracle is `common::oracle`, not its own test file.** Each `tests/*.rs`
  is its own crate and cannot import another, so the shared module is the only
  place 14.5, 14.6 and 14.7 can all reach it from — the same reason
  `tests/common/mod.rs` exists at all. Every CLI test binary compiles it; the
  module's `#![allow(dead_code)]` is what keeps a knob nobody has used yet from
  warning.
- **`Oracle::serving(bytes)` or `serving_file(path)`, then knobs, then
  `start()`.** `url()` is what a `--source` argument gets, `addr()` what a raw
  client connects to, and dropping the `Oracle` stops the thread and frees the
  port. A started oracle with no knob set is a **correct** origin server, and
  that control is what every misbehaving case is read against.
- **Seven knobs, against the row's five.** `ignoring_range`,
  `etag_changing_after`, `short_range_after`, `truncating_body_after` and
  `not_found_after` are the five the spec named. `without_etag` and
  `without_last_modified` are added because D5's "absence is a failure" cannot
  be reached remotely without a server that sends neither, and because
  `object_store` substitutes the epoch for an absent `Last-Modified` — the
  asymmetry the spec records under "What the backend actually does" needs both
  halves producible before 14.6 can test the rule that reads it.
- **`_after(n)` is a 1-based request ordinal, compared strictly**, so
  `_after(0)` fires on the first request and `_after(2)` on the third.
  Connections are served one at a time in arrival order, which is what makes an
  ordinal mean something rather than race; a remote source that reads
  concurrently would need that revisited, and D7 sets `default_workers = 1`.
- **`Oracle::requests()` is the other half of the instrument.** It is how 14.5
  asserts that the origin probe cost **one** round trip rather than a `HEAD`
  plus a `GET` (D2's claim about the 206 carrying `Content-Range`'s total), and
  how 14.6 asserts that the precondition rode on *every* ranged GET rather than
  the first (D10/D11).
- **A short range and a truncated body are two different knobs**, and the split
  is the one 14.5 has to handle separately. `short_range_after` answers fewer
  bytes than were asked for and **declares what it sent** — a legal response the
  caller must notice rather than a failure. `truncating_body_after` declares the
  full length and then closes the connection partway, which is a transport
  failure. A `read_range` that trusted `Content-Length` over what arrived would
  pass the first and fail the second.
- **`http_date`/`parse_http_date` are `pub`**, for a test that wants to build an
  `If-Unmodified-Since` by hand or read a `Last-Modified` back.

## Negative results

- **No HTTP crate was taken as a dev-dependency**, and the maintainer's review
  of that call kept it: the reasoning and both rejected alternatives are
  [`roadmap-P14-remote-input.md`](roadmap-P14-remote-input.md), "D6".
- **The self-tests do not go through `object_store`**, and that is the point
  rather than an accident of ordering: an instrument checked with the thing it
  judges agrees with it by construction. So `common::oracle`'s `raw_get` /
  `raw_head` write a request onto a `TcpStream` and read until the server closes,
  and `RawResponse::complete()` — declared length against bytes that actually
  arrived — is what distinguishes the two truncation knobs on the wire.
- **Every connection is answered once and closed.** `Connection: close` on every
  response, no keep-alive, no pipelining. It makes the ordinal a request number
  rather than a connection's, it makes the truncating knob a plain short write,
  and it costs a remote scan one TCP handshake per ranged GET over loopback,
  which is not a figure this phase takes.
- **A malformed `Range` is a `400`, not a full body.** The tempting reading —
  "an unparseable header is an absent header" — would make the oracle silently
  answer 200 where a test meant to assert a 206, which is the failure an oracle
  exists to prevent. The same rule sends an unparseable `If-Modified-Since` to
  `400` rather than to "no precondition".
- **No `D<k>` was added.** The spec's D6 already decides that the oracle is an
  in-process misbehaving server and why; everything above is either how the
  thing works — which is its rustdoc — or a negative result, which is here.
  `decisions.md` is two lines under its cap, and an entry restating D6 is what
  that cap refuses.
- **No `RT<n>` was added.** The spec's list of `RT` entries this phase owes is
  entirely `object_store`'s behaviour, and none of it is depended on yet: the
  oracle *produces* those conditions and asserts nothing about how the crate
  reads them. 14.5 is where the first of them lands.
- **The oracle serves no `.xz`.** Nothing about the composition is HTTP-shaped —
  `XzSource` wraps a `ByteRangeSource` and does not know what is under it
  ([`roadmap-P14-remote-input.md`](roadmap-P14-remote-input.md), "D13") — so
  `serving_file` over a compressed fixture is all 14.7 needs, and building a
  compressed body into the oracle would have pre-committed to one.
