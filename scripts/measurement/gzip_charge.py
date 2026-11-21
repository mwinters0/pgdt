"""The gzip first pass's own charge terms: what a pass, a speculative decode and a task hold
resident, from the decoder's and the library's constants."""

from __future__ import annotations

from generate_gzip_input import BGZF_BLOCK

from measurement import memory_model


# -- what a gzip scan holds against its charge ------------------------------

#: `pgdt_io`'s gzip `CHECKPOINT_SPACING`, mirrored: the plaintext a checkpoint
#: unit holds at least, and so what a BGZF file's members are coalesced to
#: (`bgzf_unit`).
LIBRARY_GZIP_SPACING = 24 << 20

#: `pgdt_gzip_seek::DECODER_BYTES`, mirrored: the inflater's own state, which a gzip
#: decode is charged whichever engine it runs on.
GZIP_DECODER_BYTES = 64 * 1024

#: `pgdt_gzip_seek::WINDOW_SIZE`, mirrored: deflate's 32 KiB history, read as a unit's
#: decode begins and copied out at each checkpoint a first pass places.
GZIP_WINDOW_BYTES = 32 * 1024

#: The gzip adapter's `INPUT_CHUNK` (`source/compressed/deflate.rs`), mirrored:
#: the compressed input a decode pulls through at a time, and what a local
#: task reads its chunk in.
GZIP_INPUT_CHUNK = 1 << 20

#: `first_pass/ahead.rs`'s `CHUNK_BYTES`, mirrored: how many compressed bytes
#: a first pass cuts a chunk at (`Chunking::SHIPPED`), rapidgzip's length
#: (`docs/design/decisions.md`, "D121"). **Not `GZIP_INPUT_CHUNK`**: a task's
#: charge and a file's chunk count read this, a decode's input buffer that.
GZIP_CHUNK = 4 << 20

#: `pgdt_gzip_seek::SPECULATION_LIMIT`, mirrored: twice the largest block zlib ends,
#: `lit_bufsize - 1` symbols of at most 258 bytes at its largest `memLevel` —
#: the most plaintext one speculation or decode ahead of the frontier holds,
#: and most of what a task is charged (`gzip_task_bytes`).
GZIP_SPECULATION_LIMIT = 2 * ((1 << (9 + 6)) - 1) * 258

#: `first_pass/ahead.rs`'s `DECODES_A_TASK` and `GUESSES_A_TASK`, mirrored:
#: how many decodes and guesses a task may hold for the frontier, which
#: `gzip_task_bytes` charges.
GZIP_DECODES_A_TASK = 2

GZIP_GUESSES_A_TASK = 5

#: `pgdt-gzip-seek`'s `EVENTS_A_GUESS` (`speculate.rs`), mirrored: the most events a
#: guess spans before a bound cuts it, which bounds what a chunk's decode
#: records (`gzip_decode_bytes`).
GZIP_EVENTS_A_GUESS = 16 * (GZIP_SPECULATION_LIMIT // ((1 << (8 + 6)) - 1))

#: The bytes of one event and one piece a chunk's decode records
#: (`size_of::<(u64, Event)>()` and `size_of::<(u64, u64, u32)>()` in
#: `pgdt-gzip-seek`'s `chunk.rs`), mirrored.
GZIP_EVENT_BYTES = 24

GZIP_PIECE_BYTES = 24

#: `pgdt-gzip-seek`'s `STATE_BYTES` (`speculate.rs`), mirrored: what a task's own
#: state holds besides the buffers its bound names.
GZIP_TASK_STATE_BYTES = 32 * 1024

#: `pgdt-gzip-seek`'s `LONGEST_HEADER` (`native/inflate.rs`), `PAD`
#: (`native/input.rs`) and `ROOM_PAST_LIMIT` (`native/sink.rs`), mirrored:
#: what a speculation's input holds past where its decode waits, the padding
#: past its bytes, and the room each sink keeps past its limit.
GZIP_LONGEST_HEADER = (3 + 5 + 5 + 4 + 19 * 3 + (288 + 32) * 7 + 7) // 8

GZIP_INPUT_PAD = 8

GZIP_ROOM_PAST_LIMIT = 258 + 32

#: `pgdt-gzip-seek`'s `CHAIN_PAST` (`speculate.rs`), mirrored: the most a BGZF chain
#: read holds past its chunk, a member as long as a `BSIZE` states and a header.
GZIP_CHAIN_PAST = (1 << 16) + 18

#: The first pass's `SKIP_SCRATCH` (`first_pass/pass.rs`), mirrored: what a
#: read skipping to its first wanted byte decodes into.
GZIP_SKIP_SCRATCH = 64 * 1024


def gzip_pass_bytes() -> int:
    """What a first pass holds besides its tasks: two forward decodes — the
    frontier and one behind it — each its inflater, its window copy and its
    input chunk, and the skip scratch. Its windows are on disk.

    `PASS_BYTES` (`first_pass.rs`), mirrored."""
    decode = GZIP_DECODER_BYTES + GZIP_WINDOW_BYTES + GZIP_INPUT_CHUNK
    return 2 * decode + GZIP_SKIP_SCRATCH


def gzip_speculation_bytes(chunk: int, feed: int) -> int:
    """The most one running speculation holds: its input, from its chunk to
    the limit's worth past it, a block's header and a feed; its two sinks at
    their rooms, the marked one two bytes a value; the guess it hands back;
    and its state.

    `pgdt_gzip_seek::ChunkTask::speculation_bytes` (`speculate.rs`, `most_held`),
    mirrored."""
    limit = GZIP_SPECULATION_LIMIT
    past = max(limit, GZIP_CHAIN_PAST)
    held = chunk + past + GZIP_LONGEST_HEADER + feed + GZIP_INPUT_PAD
    sinks = 3 * (limit + GZIP_ROOM_PAST_LIMIT)
    return held + sinks + 2 * GZIP_WINDOW_BYTES + GZIP_TASK_STATE_BYTES


def gzip_decode_bytes() -> int:
    """The most one chunk's decode holds, handed out or done: its window, its
    plaintext and a byte, its events and pieces, its engine and its state.

    `pgdt_gzip_seek::ChunkTask::decode_bytes` (`chunk.rs`, `most_held`), mirrored."""
    guessed = GZIP_EVENTS_A_GUESS + 3
    events = (2 * guessed + 1) * GZIP_EVENT_BYTES
    pieces = guessed * GZIP_PIECE_BYTES
    buffers = GZIP_WINDOW_BYTES + GZIP_SPECULATION_LIMIT + 1 + events + pieces
    return buffers + GZIP_DECODER_BYTES + GZIP_TASK_STATE_BYTES


def gzip_task_bytes(chunk: int = GZIP_CHUNK) -> int:
    """What one task ahead of a first pass's frontier is charged: a running
    speculation, fed a read of `INPUT_CHUNK` or a chunk where that is longer;
    `DECODES_A_TASK` decodes held for the frontier; `GUESSES_A_TASK` guesses,
    each with the window of a decode queued behind it; and its read buffer.
    No decode's plaintext buffer is freed while the pass runs, the pass
    keeping each for the next decode (D124).

    `Chunking::task_bytes` at `Chunking::SHIPPED` (`first_pass/ahead.rs`),
    mirrored, or at a `chunk` a measured leg cuts in its place
    (`GZPASS_CHUNK_BYTES`). **All of it but a few chunks' worth is six
    speculation limits** — a speculation's input past its chunk and its two
    sinks four, and two decodes held — which is why this figure is
    what prices that constant: no leg varies it, and a first-pass
    cell's remainder is read against a charge it dominates
    (`docs/design/decisions.md`, "D124")."""
    feed = max(chunk, GZIP_INPUT_CHUNK)
    running = max(gzip_speculation_bytes(chunk, feed), gzip_decode_bytes())
    decodes, guesses = GZIP_DECODES_A_TASK, GZIP_GUESSES_A_TASK
    held = decodes * gzip_decode_bytes() + guesses * 3 * GZIP_WINDOW_BYTES
    return running + held + GZIP_INPUT_CHUNK


def gzip_first_pass_charge(jobs: int, stored_size: int) -> int:
    """What the budget rule charges `jobs` workers of a single member's first
    pass over a `stored_size`-byte file: each the larger of the pass's own
    charge and a task's, on a file of more than one chunk, where a task can
    run at all, and the pass's alone otherwise. **No pool**: a first pass
    retains no block, its index being the by-product.

    `FirstPass::default_worker_memory`, mirrored — a local file, so no
    download is added. Charged at one worker too, though a serial pass runs no
    task: the charge is per worker whatever runs on it."""
    chunks = max(1, -(-stored_size // GZIP_CHUNK))
    per_worker = max(gzip_pass_bytes(), gzip_task_bytes()) if chunks > 1 else gzip_pass_bytes()
    return jobs * per_worker


def gzip_block_reader_bytes(unit: int) -> int:
    """What one block-decoding reader of a local gzip file with `unit`-sized
    units holds: its block slot, the scan's chunk, and the decode's pulling
    footprint — the inflater, the window read as the unit begins, and the
    input chunk, every registered unit compressing past `GZIP_INPUT_CHUNK` so
    the chunk saturates its cap, as `XZ_DECODE_FOOTPRINT`'s does.

    `BlockCache::reader_bytes` over `DeflateCodec::pulling_footprint`,
    mirrored — `reader_bytes` with gzip's decoder in place of liblzma's."""
    return unit + memory_model.LIBRARY_CHUNK_BYTES + GZIP_DECODER_BYTES + GZIP_WINDOW_BYTES + GZIP_INPUT_CHUNK


def gzip_block_charge(unit: int, jobs: int) -> int:
    """What the budget rule charges `jobs` block-decoding readers of a gzip
    file with `unit`-sized units: the per-reader term times the count, plus
    the retention list they share (`pool_bytes`), as `charge_bytes` is for
    `.xz`."""
    return jobs * gzip_block_reader_bytes(unit) + memory_model.pool_bytes(unit, jobs)


def bgzf_unit() -> int:
    """The plaintext a unit of a BGZF file holds where every member is a full
    `BGZF_BLOCK`: members join a unit until it holds the spacing, so a unit is
    the fewest members reaching it (`pgdt_gzip_seek::Walk`, `placement.rs`). Every
    unit of `control_bgzf` but its last is this size, the plaintext holding
    many units, and the last is smaller, so it is the file's largest unit."""
    members = -(-LIBRARY_GZIP_SPACING // BGZF_BLOCK)
    return members * BGZF_BLOCK
