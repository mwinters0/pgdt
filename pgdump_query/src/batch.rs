//! Row/batch assembly: turns rows inside a `COPY` block into `Utf8View`
//! Arrow `RecordBatch`es.
//!
//! Per `docs/design/scan-performance.md`, a field that needs no unescaping
//! is appended as a zero-copy view into the Arrow `Buffer` backing the read
//! chunk it came from, rather than copied into the builder's own storage —
//! retrofitting that later would be expensive, so it's built in now even
//! though the rest of the performance work (roadmap Phase 5) is not. Only
//! fields that need unescaping, or whose bytes straddle two read chunks,
//! take a copying path.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::Arc;

use arrow::array::builder::StringViewBuilder;
use arrow::array::{ArrayRef, RecordBatch};
use arrow::buffer::Buffer;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};

use crate::copy::{CopyHeader, DELIMITER, decode_field};
use crate::io::ByteRangeSource;
use crate::scan::{CopyScanner, Event, ScanOptions};
use crate::{Error, Result};

/// Tuning knobs for batch assembly.
#[derive(Debug, Clone)]
pub struct BatchOptions {
    /// Rows per batch. A batch is flushed once it reaches this many rows.
    pub max_rows: usize,
    /// Optional cap on a batch's total field-byte count. Whichever of this
    /// or `max_rows` is hit first flushes the batch.
    pub max_bytes: Option<usize>,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self { max_rows: 8192, max_bytes: None }
    }
}

/// A read chunk retained only long enough for zero-copy views to be taken
/// into it. Dropped once the scanner has moved past it for good.
struct SourceChunk {
    /// Absolute file offset of `buffer[0]`.
    start: u64,
    buffer: Buffer,
    /// Cached `StringViewBuilder::append_block` index per column, filled in
    /// the first time a column takes a view into this chunk. Grown lazily
    /// rather than sized up front, since more than one schema (from
    /// sequential or same-name-different-schema blocks) can reference the
    /// same chunk.
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

/// `StringViewBuilder::finish()` resets its internal block list to build the
/// next batch, which invalidates every cached block index in `chunks` — a
/// flush must clear them all, or a later reference to an already-seen chunk
/// would resolve to the wrong (or out-of-bounds) block in the new batch.
fn invalidate_block_cache(chunks: &mut VecDeque<SourceChunk>) {
    for chunk in chunks.iter_mut() {
        chunk.column_blocks.clear();
    }
}

/// Build the schema for a `COPY` header's columns. A header with no
/// explicit column list means "all columns, in table order" (see
/// `CopyHeader::columns`) — Phase 1 has no DDL parsing to name them from, so
/// placeholder names are used instead, sized to `field_count` (the first
/// row's field count).
fn schema_for(header: &CopyHeader, field_count: usize) -> SchemaRef {
    let names: Vec<String> = if header.columns.is_empty() {
        (1..=field_count).map(|i| format!("column{i}")).collect()
    } else {
        header.columns.clone()
    };
    let fields: Vec<Field> =
        names.into_iter().map(|name| Field::new(name, DataType::Utf8View, true)).collect();
    Arc::new(Schema::new(fields))
}

/// Accumulates rows from a single `COPY` block into `Utf8View`
/// `RecordBatch`es.
struct RowBatcher {
    schema: SchemaRef,
    columns: Vec<StringViewBuilder>,
    rows_in_batch: usize,
    bytes_in_batch: usize,
    options: BatchOptions,
}

impl RowBatcher {
    fn new(schema: SchemaRef, options: BatchOptions) -> Self {
        let columns = (0..schema.fields().len()).map(|_| StringViewBuilder::new()).collect();
        Self { schema, columns, rows_in_batch: 0, bytes_in_batch: 0, options }
    }

    fn is_empty(&self) -> bool {
        self.rows_in_batch == 0
    }

    fn should_flush(&self) -> bool {
        self.rows_in_batch >= self.options.max_rows
            || self.options.max_bytes.is_some_and(|max| self.bytes_in_batch >= max)
    }

    /// Append one raw (still-escaped) COPY TEXT data row.
    fn push_row(
        &mut self,
        header_offset: u64,
        row_offset: u64,
        raw: &[u8],
        chunks: &mut VecDeque<SourceChunk>,
    ) -> Result<()> {
        let mut col = 0;
        let mut pos = 0usize;
        loop {
            let end = memchr::memchr(DELIMITER, &raw[pos..]).map_or(raw.len(), |i| pos + i);
            let field = &raw[pos..end];
            let expected = self.columns.len();
            let builder = self.columns.get_mut(col).ok_or(Error::ColumnCountMismatch {
                header_offset,
                row_offset,
                expected,
                found: col + 1,
            })?;
            push_field(builder, col, row_offset + pos as u64, field, chunks)?;
            self.bytes_in_batch += field.len();
            col += 1;
            if end == raw.len() {
                break;
            }
            pos = end + 1;
        }
        if col != self.columns.len() {
            return Err(Error::ColumnCountMismatch {
                header_offset,
                row_offset,
                expected: self.columns.len(),
                found: col,
            });
        }
        self.rows_in_batch += 1;
        Ok(())
    }

    fn flush(&mut self) -> Result<RecordBatch> {
        self.rows_in_batch = 0;
        self.bytes_in_batch = 0;
        let arrays: Vec<ArrayRef> =
            self.columns.iter_mut().map(|b| Arc::new(b.finish()) as ArrayRef).collect();
        Ok(RecordBatch::try_new(self.schema.clone(), arrays)?)
    }
}

/// Append one still-escaped field to `builder`. Reuses [`decode_field`] so
/// the escaping rules live in exactly one place; a `Cow::Borrowed` result
/// (no escapes present, already UTF-8 checked) is what makes the field
/// eligible for a zero-copy view — everything else is copied.
fn push_field(
    builder: &mut StringViewBuilder,
    col: usize,
    field_offset: u64,
    field: &[u8],
    chunks: &mut VecDeque<SourceChunk>,
) -> Result<()> {
    match decode_field(field)? {
        None => builder.append_null(),
        Some(Cow::Owned(s)) => builder.append_value(s),
        Some(Cow::Borrowed(s)) => {
            let view = chunks
                .iter_mut()
                .find_map(|c| c.contains(field_offset, field.len()).map(|coords| (c, coords)));
            match view {
                Some((chunk, (local_offset, len))) => {
                    let block = chunk.block_for(col, builder);
                    // SAFETY: `contains` confirmed `local_offset..local_offset+len`
                    // is in bounds for `block`, and the `Borrowed` case above
                    // means `decode_field` already validated this exact byte
                    // range as UTF-8.
                    unsafe { builder.append_view_unchecked(block, local_offset, len) };
                }
                // The field's bytes straddle two read chunks (rare: at most
                // one field per chunk boundary) or its chunk was already
                // evicted. Either way, a copy is unavoidable.
                None => builder.append_value(s),
            }
        }
    }
    Ok(())
}

/// Scan `source` end to end, assembling `Utf8View` `RecordBatch`es for every
/// row of every `COPY` block whose table matches `table` (qualified or
/// bare — see [`CopyHeader::matches`]). A table with zero rows produces no
/// batches.
///
/// The callback may return [`ControlFlow::Break`] to stop early.
pub async fn read_table<S, F>(
    source: &S,
    table: &str,
    scan_options: &ScanOptions,
    batch_options: &BatchOptions,
    mut on_batch: F,
) -> Result<()>
where
    S: ByteRangeSource,
    F: FnMut(RecordBatch) -> ControlFlow<()>,
{
    let size = source.size().await?;
    let mut scanner = CopyScanner::new();
    let mut buf: Vec<u8> = Vec::with_capacity(scan_options.chunk_size);
    let mut chunks: VecDeque<SourceChunk> = VecDeque::new();
    let mut read_pos = 0u64;

    // Set once a matching header with an explicit column list starts, or
    // once the first row of a matching headerless-column block arrives.
    let mut active: Option<(u64, RowBatcher)> = None;
    // A matching header with no column list, waiting on its first row to
    // learn the field count.
    let mut pending: Option<(CopyHeader, u64)> = None;

    loop {
        let want = scan_options.chunk_size.min((size - read_pos) as usize);
        if want > 0 {
            let bytes = source.read_range(read_pos, want).await?;
            chunks.push_back(SourceChunk {
                start: read_pos,
                buffer: Buffer::from(bytes.clone()),
                column_blocks: Vec::new(),
            });
            read_pos += bytes.len() as u64;
            buf.extend_from_slice(&bytes);
        }
        let eof = read_pos >= size;

        while let Some(event) = scanner.next_event(&buf, eof)? {
            match event {
                Event::CopyStart(start) if start.header.matches(table) => {
                    if start.header.columns.is_empty() {
                        pending = Some((start.header, start.header_offset));
                    } else {
                        let schema = schema_for(&start.header, start.header.columns.len());
                        active = Some((
                            start.header_offset,
                            RowBatcher::new(schema, batch_options.clone()),
                        ));
                    }
                }
                Event::CopyStart(_) => {}
                Event::Row(row) => {
                    if let Some((header, header_offset)) = pending.take() {
                        let field_count = memchr::memchr_iter(DELIMITER, row.raw).count() + 1;
                        let schema = schema_for(&header, field_count);
                        active =
                            Some((header_offset, RowBatcher::new(schema, batch_options.clone())));
                    }
                    if let Some((header_offset, batcher)) = active.as_mut() {
                        batcher.push_row(*header_offset, row.offset, row.raw, &mut chunks)?;
                        if batcher.should_flush() {
                            let batch = batcher.flush()?;
                            invalidate_block_cache(&mut chunks);
                            if on_batch(batch).is_break() {
                                return Ok(());
                            }
                        }
                    }
                }
                Event::CopyEnd(_) => {
                    pending = None;
                    if let Some((_, mut batcher)) = active.take()
                        && !batcher.is_empty()
                    {
                        let batch = batcher.flush()?;
                        invalidate_block_cache(&mut chunks);
                        if on_batch(batch).is_break() {
                            return Ok(());
                        }
                    }
                }
            }
        }

        let used = scanner.take_consumed();
        buf.drain(..used);

        // Everything before the scanner's new position has already had its
        // chance to be referenced by a zero-copy view (that happens
        // synchronously above, before we get here), so it's safe to drop.
        let floor = scanner.position();
        while chunks.front().is_some_and(|c| c.end() <= floor) {
            chunks.pop_front();
        }

        if eof {
            return Ok(());
        }
        if buf.len() > scan_options.max_line_bytes {
            return Err(Error::LineTooLong {
                offset: scanner.position(),
                limit: scan_options.max_line_bytes,
            });
        }
    }
}
