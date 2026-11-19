//! The read chunks a batch's zero-copy views point into, in file order: a
//! read loop says what it read and how far the scanner has got, and this
//! module decides when a chunk becomes an Arrow `Buffer` and how long it is
//! kept, and holds the cached builder block indices the read loop invalidates
//! at a flush (`docs/design/decisions.md`, "D46", "D68").

use std::borrow::Cow;
use std::collections::VecDeque;

use arrow::array::Array;
use arrow::array::builder::StringViewBuilder;
use arrow::buffer::Buffer;
use bytes::Bytes;

/// A read chunk retained only long enough for zero-copy views to be taken
/// into it. Dropped once its read loop's events run out
/// ([`RetainedChunks::release_through`]).
struct SourceChunk {
    /// Absolute file offset of `buffer[0]`.
    start: u64,
    buffer: Buffer,
    /// Cached `StringViewBuilder::append_block` index per column, filled in
    /// the first time a column takes a view into this chunk. Grown lazily,
    /// since a chunk is retained before any column reads it.
    column_blocks: Vec<Option<u32>>,
}

impl SourceChunk {
    fn end(&self) -> u64 {
        self.start + self.buffer.len() as u64
    }

    /// If `[offset, offset + len)` lies entirely within this chunk, the
    /// `(local_offset, len)` view coordinates into it.
    fn contains(&self, offset: u64, len: usize) -> Option<(u32, u32)> {
        let local = offset.checked_sub(self.start)?;
        let local_end = local.checked_add(len as u64)?;
        (local_end <= self.buffer.len() as u64).then_some((local as u32, len as u32))
    }

    /// The block `col`'s builder holds this chunk as, appending it where it
    /// holds none, and whether it was appended here.
    fn block_for(&mut self, col: usize, builder: &mut StringViewBuilder) -> (u32, bool) {
        if self.column_blocks.len() <= col {
            self.column_blocks.resize(col + 1, None);
        }
        match self.column_blocks[col] {
            Some(block) => (block, false),
            None => {
                let block = builder.append_block(self.buffer.clone());
                self.column_blocks[col] = Some(block);
                (block, true)
            }
        }
    }
}

/// The read chunks a batch's zero-copy views may still point into, in file
/// order. A replay loop hands each chunk it reads to [`Self::retain`] and
/// tells it where the scanner has got to; everything else about the
/// arrangement — that a chunk becomes an Arrow `Buffer` at all, that the
/// buffer is what a view is taken against, that a flush leaves a cached
/// block index stale ([`Self::invalidate_block_cache`], which the loop calls)
/// — is Arrow assembly's business, which is why this type lives
/// beside the builders (`docs/design/decisions.md`, "D68").
///
/// Deficiency register: `deficiency: KD171` — a view taken here keeps its
/// chunk's whole buffer, a read chunk or a decoded block, alive in the batch
/// it is emitted in for as long as that batch's consumer holds it. The budget
/// bills a reader its in-flight span or the unit it is decoding
/// ([`pgdt_io::RetainedUnit`]), never a batch it has emitted, so a consumer
/// holding batches — `pgdt query`'s merge one a partition (`KD57`), a
/// DataFusion sort or a join's build side to its end — holds source buffers no
/// carve states, and DataFusion's pool, where an operator reserves for them,
/// charges the whole buffers. Closing it means compacting a batch to its
/// viewed bytes as it is emitted, which `docs/design/decisions.md`, "D46"
/// refuses at every flush, or billing what a consumer holds. **(c)
/// unowned**; promoted by a phase taking up query-path memory.
pub(crate) struct RetainedChunks {
    chunks: VecDeque<SourceChunk>,
}

impl RetainedChunks {
    pub(crate) fn new() -> Self {
        Self { chunks: VecDeque::new() }
    }

    /// Retain one read chunk, or a carried line, `start` being the absolute
    /// file offset of its first byte. Retained in file order of their ends,
    /// so a carried line goes ahead of the chunk that completed it.
    ///
    /// **This is the one place a read chunk becomes an `arrow::Buffer`.** The
    /// `Bytes` is cloned rather than consumed because the caller still scans
    /// it; both refer to the same allocation, which the buffer pool reclaims
    /// only when the last reference dies.
    pub(crate) fn retain(&mut self, start: u64, bytes: &Bytes) {
        self.chunks.push_back(SourceChunk {
            start,
            buffer: Buffer::from(bytes.clone()),
            column_blocks: Vec::new(),
        });
    }

    /// Drop every chunk ending at or before `floor` — viewing happens
    /// synchronously as rows are pushed, so a chunk whose rows have all been
    /// pushed has had its chance. A view already taken keeps its buffer in
    /// the batch; this drops only the read loop's reference.
    ///
    /// **A read loop releases a chunk once its events run out**, not once the
    /// scanner is past it: what a chunk leaves unconsumed is copied into the
    /// carry, which is retained as a buffer of its own
    /// ([`pgdt_dump::ChunkEvents::held`]), so the row straddling the next
    /// boundary never needs a chunk under it, and a line many chunks long
    /// pins none of them while it grows.
    pub(crate) fn release_through(&mut self, floor: u64) {
        while self.chunks.front().is_some_and(|c| c.end() <= floor) {
            self.chunks.pop_front();
        }
    }

    /// `StringViewBuilder::finish()` resets its internal block list, which
    /// invalidates every cached block index held here: a flush must clear
    /// them all, or a later reference to an already-seen chunk resolves to
    /// the wrong block in the new batch.
    pub(crate) fn invalidate_block_cache(&mut self) {
        for chunk in self.chunks.iter_mut() {
            chunk.column_blocks.clear();
        }
    }
}

/// Arrow's largest data block, `GenericByteViewBuilder`'s private
/// `MAX_BLOCK_SIZE`: a value longer is given a block of its own by
/// `append_value` too, so handing it the decoded text as that block keeps
/// that value alone in its block and drops a copy, though the block keeps
/// the text's spare capacity and the builder's next block is sized as if no
/// value had been appended (`docs/design/decisions.md`, "D141").
const OWN_BLOCK_PAST: usize = 2 << 20;

/// Append one already-decoded field to a `Utf8View` column, beside the raw
/// field's offset and length. The caller decodes, so the escaping rules live
/// in one place; a `Cow::Borrowed` result (no escapes present, already UTF-8
/// checked) is what makes the field eligible for a zero-copy view, and an
/// owned one past [`OWN_BLOCK_PAST`] becomes a block of its own — everything
/// else is copied. **Whether the column now holds a retained chunk it did
/// not**, which [`own_views`] gives up.
pub(super) fn push_utf8view_field(
    builder: &mut StringViewBuilder,
    col: usize,
    field_offset: u64,
    field_len: usize,
    text: Cow<'_, str>,
    chunks: &mut RetainedChunks,
) -> bool {
    match text {
        Cow::Owned(s) if s.len() > OWN_BLOCK_PAST => {
            let len = s.len();
            let block = builder.append_block(Buffer::from_vec(s.into_bytes()));
            // SAFETY: the block is the whole of a `String`, so the range is in
            // bounds and UTF-8; `append_block` has asserted it under 4 GiB.
            #[allow(unsafe_code)]
            unsafe {
                builder.append_view_unchecked(block, 0, len as u32)
            };
            false
        }
        Cow::Owned(s) => {
            builder.append_value(s);
            false
        }
        Cow::Borrowed(s) => {
            let view = chunks
                .chunks
                .iter_mut()
                .find_map(|c| c.contains(field_offset, field_len).map(|coords| (c, coords)));
            match view {
                Some((chunk, (local_offset, len))) => {
                    let (block, appended) = chunk.block_for(col, builder);
                    // SAFETY: `contains` confirmed `local_offset..local_offset+len`
                    // is in bounds for `block`, and the `Borrowed` case above
                    // means the decode already validated this exact byte
                    // range as UTF-8. `contains` narrows to `u32`, so this
                    // holds while a retained chunk stays under 4 GiB, which
                    // every shipped chunk and block size is by orders of
                    // magnitude, and `ReadOptions::chunk` clamps a stated chunk
                    // to the line bound, under 1 GiB.
                    #[allow(unsafe_code)]
                    unsafe {
                        builder.append_view_unchecked(block, local_offset, len)
                    };
                    appended
                }
                // No retained buffer holds the field whole: a read loop
                // retains the carried line beside the chunk, so only a row
                // pushed from bytes it never retained gets here.
                None => {
                    builder.append_value(s);
                    false
                }
            }
        }
    }
}

/// **Re-own a `Utf8View` column's values**, so it holds none of the read
/// chunks its views pointed into: its values so far are copied, the bytes its
/// views read alone, into a fresh builder that goes on as it would have. A
/// block index [`RetainedChunks`] cached for it is then stale, which the
/// caller clears ([`RetainedChunks::invalidate_block_cache`]).
pub(super) fn own_views(builder: &mut StringViewBuilder) {
    let held = builder.finish().gc();
    let mut owned = StringViewBuilder::with_capacity(held.len());
    owned.append_array(&held);
    *builder = owned;
}
