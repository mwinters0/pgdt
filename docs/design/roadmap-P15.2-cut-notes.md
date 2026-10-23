# P15.2 notes — The cut

The codec-generic layer drawn with xz as its one implementor, behaviour
preserved; gzip arrives on top of it from 15.4
([`roadmap-P15-gzip.md`](roadmap-P15-gzip.md), "Settled in the grilling").

- **`pgdt-io`'s `source/compressed/` is the layer.** `codec.rs` holds the
  `Codec` trait, the `UnitRead` handle, the `Positioned` read a decode pulls
  through (a file, or a fetched `Window`) and the short read; `compressed.rs`
  the live piecewise decode, `piece_of` and the dispatch from an index to its
  codec; `file.rs` and `fetched.rs` are the two arms, `CompressedFileSource`
  and `FetchedCompressedSource`, each written once; `blocks.rs` the block cache
  and `DecodeBudget`; `index.rs` the `CompressionIndex` and the `Format`
  recognition reads off the first bytes; `xz.rs` the adapter — `XzCodec`, the
  fetched walk and the bridge from `Positioned` to `xz_seek::CompressedSource`
  — and `xz/table.rs` the `SeekTable` and the magic, the only files naming
  `xz_seek`.
- **Every edge runs one way** (`module_graph.py`): the traits sit apart from
  the dispatch so an adapter reads them without reading what dispatches to it,
  and the layout type a `CompressionIndex` variant holds is a leaf under its
  adapter, so the index names it without reaching the codec that builds it. A
  gzip adapter keeps that shape.
- **The codec is a `dyn` trait over unit indices**, held as `Arc<dyn Codec>`:
  the codec is chosen at run time, from the magic or a cached index
  ([`decisions.md`](decisions.md), "D15"). The handle and the positioned read
  are object-safe for the same reason.
- **What 15.3 to 15.6 inherit.** A gzip codec implements `Codec` and
  `UnitRead`, adds a `CompressionIndex` variant and a `Format` with its magic,
  and the compiler then names every site still assuming xz: `codec_over`,
  `CompressedFileSource::opened`, `open_remote`'s unknown-format arm,
  `CompressionShape::of`, and `CompressionIndex`'s own methods. `index_refused`
  asks the xz adapter alone, and grows an arm per codec.
  `NonSeekableCompressedSource` still names `xz -T0`; 15.5.2 takes its text
  from the codec.
- **The public surface moved with it.** `XzSource` and `FetchedXzSource` are
  `CompressedFileSource` (`open`, recognising the format itself, and
  `with_index`) and `FetchedCompressedSource` (`with_index`);
  `ByteRangeSource::seek_table` is `compression_index`;
  `KnownCompression::Xz` is `Compressed`; `Error::Xz` is
  `Error::Codec { codec, cause }`, still displayed `xz error: …`, its cause
  read by `codec_cause`. `pgdt-dump`'s own `CompressionIndex` is gone; the
  cache persists `pgdt-io`'s, and `pgdt info --json` exports it as
  `compression_index` where it said `seek_table`.
- **`CACHE_FORMAT_VERSION` is 72**, as the spec asked. The encoded bytes did
  not change — bincode names no field and the moved enum encodes as the old
  one did — so the bump only retires caches 15.5.1's window section would
  retire anyway.
- **`determinism.rs` reads `.xz`**:
  `an_xz_dump_writes_the_serial_cache_at_every_stated_parallelism` compresses
  the generated past-the-chunk dump in 1 MiB blocks and holds the serial cache
  against the recommended count, 4 and 8 workers, a piecewise-starved budget,
  and a gathering pass at 8, the leader's floor asserted against the `.xz`
  source's own partition.

## Negative results

- **A short read past the plaintext now says "compressed stream ended before
  the requested range"** where it said "xz stream": the shared layer names no
  codec, and the error is an `UnexpectedEof` read by kind, never by text.
- **The fetched `Window` is `pgdt-io`'s own**, not `xz_seek::Window`: a gzip
  unit would otherwise be decoded out of a type of the xz crate.
