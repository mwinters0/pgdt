# P15.4 notes — `gz-seek` built in the workspace

`crates/gz-seek/` is the gzip addressing layer the spec asks for
([`roadmap-P15-gzip.md`](roadmap-P15-gzip.md), "Settled in the grilling"), a
leaf member nothing links yet: `pgdt-io`'s row in `scripts/layering.py`
admits it, and 15.5's `deflate` adapter is its first caller. Its crate doc is
the overview; what follows is what the next slices inherit.

- **The API, all of it sans-IO and push-driven.** `Forward` is the first pass:
  `read(input, out)` takes the next compressed bytes from
  `compressed_position()` and space for plaintext, returns at each checkpoint
  it places with that checkpoint's window at `window()`, and a step with no
  progress and no checkpoint means it needs more input; `finish()` turns the
  input's end into the complete `Index` or a `Truncated`. `index()` is the
  partial index an interrupted pass saves, its last unit open, and
  `Forward::resume(&partial, window)` continues it into the index an
  uninterrupted pass builds. `Index::unit(i)` hands out a `UnitTask` (`Copy`,
  holding no index): `compressed_range()`, `window_len()`, `decode(window,
  compressed, out)` whole, or `begin(window)` for a `UnitRead` fed as `Forward`
  is. `Walk` is BGZF's, `Request`/`supply` as `xz_seek::Walk`, reading as many
  members as a supply holds. `bgzf_member_len` and `BGZF_HEADER_LEN` are
  recognition's 18 bytes; `GZIP_MAGIC` the plain magic.
- **The index is what 15.5 persists, by parts.** `spacing()`,
  `checkpoints()` and `end()` out, `Index::from_parts` back in, validated, so
  `CompressionIndex`'s gzip variant mirrors the parts as `pgdt-io`'s own type,
  as `SeekTable` mirrors `xz_seek`'s, and `pgdt-dump` never names `gz_seek`. A
  checkpoint states its window's length (all of its member's history, to
  32 KiB); the window itself is the forward pass's output, for the cache's
  window section and the window provider. An `Index` clone is a reference
  count.
- **Unit CRCs.** A forward pass records every unit's, a whole member's
  included; a walk records none, each of its units being whole members. A
  unit carries `crc: None` only where it starts at a member start and ends at
  a member boundary, which `from_parts` enforces. A unit decode compares the
  unit's CRC where recorded and the trailer of each member it holds whole, and
  refuses a unit ending anywhere but where its index says
  (`Error::IndexMismatch`).
- **Size.** `Index::size()` is `Size::Exact` once complete and
  `Size::Open { at_least }` before, `Forward::size()` open until `finish`; a
  BGZF walk's is exact, each `ISIZE` refused past 64 KiB.
- **The spacing is the caller's**, `NonZeroU64`, persisted in the index, and
  a resume continues at the index's own.
- **The backend is zlib-rs through `libz-rs-sys`'s C API**, all of it in
  `src/zlib.rs`, the crate `deny(unsafe_code)` elsewhere
  ([`../status/upstream.md`](../status/upstream.md), "UF4"). What the decode
  relies on of it is [`runtime-invariants.md`](runtime-invariants.md), "RT23".
  D27's one shipped `unsafe` is the seven libraries'
  ([`decisions.md`](decisions.md), "D27"), and `gz-seek` is none of them
  ([`decisions.md`](decisions.md), "D68"), so 15.5 linking it leaves D27 true.
- **Bytes after the last member are refused** as `Error::NotGzip` at their
  offset, trailing zeros included; GNU gzip's "trailing garbage ignored" is
  not followed, `pg_dump` and `gzip` writing none.

## What 15.1.2 and later slices inherit

- **15.1.2 times this crate.** The checkpointing first pass is `Forward` over
  `control_gz` at each candidate spacing against the `gzip-inflate` member's
  zlib-ng leg; the decoder stops at every block's end whatever the spacing,
  so the spacing moves only the window copies (`Forward::window`), one per
  checkpoint. A resumed unit's fixed cost is `UnitTask::begin` — a fresh
  inflate state and the window set — to its first plaintext; the window
  section is `WINDOW_SIZE` a checkpoint, less only within a member's first
  32 KiB.
- **15.5's footprint terms** are one inflate state and its 32 KiB window a
  decode, and `Forward`'s own 32 KiB window copy; the crate states no
  constant for them yet.
- **15.6** drives `Walk` at open; its coalescing is the spacing's, every unit
  beginning at a member start.
- **15.8** reads `rusty-rapidgzip`'s `deflate::block_finder` first (below).
- **A unit can be the whole member.** A checkpoint is a block boundary, and
  zlib-rs's level 1 ends no block before the stream's finish, so such a file
  indexes as one unit whatever the spacing
  ([`../status/history/2026-10-22.md`](../status/history/2026-10-22.md),
  "15.4: `gz-seek` landed, and a writer that ends no block"); 15.5 warns of
  one ([`decisions.md`](decisions.md), "D19") and 15.10 places checkpoints
  inside its block ([`../status/deficiencies.md`](../status/deficiencies.md)). A gzip test assumes
  nothing of where blocks end: the suite's `flate2` backend is whichever
  feature unification picks.

## Negative results

- **`rusty-rapidgzip` 0.2.1**, the native port the spec named first, was read
  and not built: a streaming whole-file decoder that maps its whole input and
  runs threads of its own (`pipeline.rs`, `streaming.rs`), with no index and,
  by its README, no random access; its `deflate` module inflates whole slices
  into a growing `Vec`, primed by no window. Neither the layer nor a backend.
  Its speculative block finder is the nearest existing code to 15.8's
  technique.
- **zlib-rs ends a raw stream holding no unused bit** at every buffer size
  tried, so the trailer begins at the next byte and the engine keeps no
  bytes back from its inflater (RT23). A stash for bytes the inflater took
  past a stream's end was written, never reached by any test, and removed.
- **A stop just after the final block makes no progress**: `Z_BLOCK` stops
  there (`data_type` 192) before the stream's end, so the engine inflates
  on to it in the same step, or a full output buffer there stalls the pass.
