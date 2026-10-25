//! The read chunks a batch's zero-copy views point into, in file order: a
//! read loop says what it read and how far the scanner has got, and this
//! module decides when a chunk becomes an Arrow `Buffer` and how long it is
//! kept, and holds the cached builder block indices the read loop invalidates
//! at a flush (`docs/design/decisions.md`, "D46", "D68").

use std::borrow::Cow;
use std::collections::VecDeque;

use arrow::array::builder::StringViewBuilder;
use arrow::buffer::Buffer;
use bytes::Bytes;

/// A read chunk retained only long enough for zero-copy views to be taken
/// into it. Dropped once the scanner has moved past it for good.
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

    fn block_for(&mut self, col: usize, builder: &mut StringViewBuilder) -> u32 {
        if self.column_blocks.len() <= col {
            self.column_blocks.resize(col + 1, None);
        }
        *self.column_blocks[col].get_or_insert_with(|| builder.append_block(self.buffer.clone()))
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
pub(crate) struct RetainedChunks {
    chunks: VecDeque<SourceChunk>,
}

impl RetainedChunks {
    pub(crate) fn new() -> Self {
        Self { chunks: VecDeque::new() }
    }

    /// Retain one read chunk, `start` being the absolute file offset of its
    /// first byte.
    ///
    /// **This is the one place a read chunk becomes an `arrow::Buffer`.** The
    /// `Bytes` is cloned rather than consumed because the caller still scans
    /// it; both refer to the same allocation, which the buffer pool reclaims
    /// only when the last reference dies (`docs/design/decisions.md`, "D9").
    pub(crate) fn retain(&mut self, start: u64, bytes: &Bytes) {
        self.chunks.push_back(SourceChunk {
            start,
            buffer: Buffer::from(bytes.clone()),
            column_blocks: Vec::new(),
        });
    }

    /// Drop every chunk the scanner has walked entirely past — viewing
    /// happens synchronously as rows are pushed, so everything before `floor`
    /// has had its chance.
    ///
    /// **The chunk the scanner is inside is retained**: `end() <= floor`
    /// holds only past a chunk's last byte, and the row straddling the next
    /// boundary is carried rather than scanned, so it cannot arrive needing a
    /// chunk that has gone.
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

/// Append one already-decoded field to a `Utf8View` column, beside the raw
/// field's offset and length. The caller decodes, so the escaping rules live
/// in one place; a `Cow::Borrowed` result (no escapes present, already UTF-8
/// checked) is what makes the field eligible for a zero-copy view —
/// everything else is copied.
pub(super) fn push_utf8view_field(
    builder: &mut StringViewBuilder,
    col: usize,
    field_offset: u64,
    field_len: usize,
    text: Cow<'_, str>,
    chunks: &mut RetainedChunks,
) {
    match text {
        Cow::Owned(s) => builder.append_value(s),
        Cow::Borrowed(s) => {
            let view = chunks
                .chunks
                .iter_mut()
                .find_map(|c| c.contains(field_offset, field_len).map(|coords| (c, coords)));
            match view {
                Some((chunk, (local_offset, len))) => {
                    let block = chunk.block_for(col, builder);
                    // SAFETY: `contains` confirmed `local_offset..local_offset+len`
                    // is in bounds for `block`, and the `Borrowed` case above
                    // means the decode already validated this exact byte
                    // range as UTF-8. `contains` narrows to `u32`, so this
                    // holds while a retained chunk stays under 4 GiB, which
                    // every shipped chunk and block size is by orders of
                    // magnitude; nothing clamps `ReadOptions::chunk_size_bytes` to
                    // enforce it.
                    #[allow(unsafe_code)]
                    unsafe {
                        builder.append_view_unchecked(block, local_offset, len)
                    };
                }
                // The field's bytes straddle two read chunks (rare: at most
                // one field per chunk boundary) or its chunk was already
                // evicted. Either way, a copy is unavoidable.
                None => builder.append_value(s),
            }
        }
    }
}
