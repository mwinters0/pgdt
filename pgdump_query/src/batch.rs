//! Row/batch assembly: turns rows inside a `COPY` block into `Utf8View`
//! Arrow `RecordBatch`es.
//!
//! Per `docs/design/roadmap-phase7-scan-performance.md`, a field that needs no
//! unescaping is appended as a zero-copy view into the Arrow `Buffer` backing
//! the read chunk it came from, rather than copied into the builder's own
//! storage — retrofitting that later would be expensive, so it's built in now
//! even though the rest of the performance work (roadmap Phase 7) is not. Only
//! fields that need unescaping, or whose bytes straddle two read chunks, take
//! a copying path.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::ControlFlow;
use std::sync::Arc;

use arrow::array::builder::StringViewBuilder;
use arrow::array::{ArrayRef, RecordBatch};
use arrow::buffer::Buffer;
use arrow::datatypes::{DataType, Field, Schema, SchemaRef};

use crate::cache::CacheMode;
use crate::copy::{CopyHeader, DELIMITER, decode_field};
use crate::io::ByteRangeSource;
use crate::predicate::Predicate;
use crate::resolve::SchemaMode;
use crate::scan::ScanOptions;
use crate::{Error, Result};

/// Tuning knobs for batch assembly.
#[derive(Debug, Clone)]
pub struct BatchOptions {
    /// Rows per batch. A batch is flushed once it reaches this many rows.
    pub max_rows: usize,
    /// Optional cap on a batch's total field-byte count. Whichever of this
    /// or `max_rows` is hit first flushes the batch.
    pub max_bytes: Option<usize>,
    /// Whether to resolve column types against the dump's DDL — see
    /// `docs/design/roadmap-phase2-typed-columns.md`, "Output model". Every
    /// `RecordBatch` this build produces is still all-`Utf8View` regardless
    /// (Phase 2.4 builds the decoders); this only controls what
    /// `TableStream::resolved_schema`/`read_table`'s returned
    /// [`crate::resolve::ResolvedSchema`] reports.
    pub schema_mode: SchemaMode,
    /// Selects which database's table to query when the name alone is
    /// ambiguous — matched against `DatabaseMetadata::name`
    /// (`docs/design/roadmap-phase2-typed-columns.md`, "One target per
    /// query"). `None` is the common case: a single-database dump, or a
    /// cross-schema ambiguity a qualified name already resolves on its own.
    pub database: Option<String>,
}

impl Default for BatchOptions {
    fn default() -> Self {
        Self { max_rows: 8192, max_bytes: None, schema_mode: SchemaMode::default(), database: None }
    }
}

/// A read chunk retained only long enough for zero-copy views to be taken
/// into it. Dropped once the scanner has moved past it for good.
pub(crate) struct SourceChunk {
    /// Absolute file offset of `buffer[0]`.
    pub(crate) start: u64,
    pub(crate) buffer: Buffer,
    /// Cached `StringViewBuilder::append_block` index per column, filled in
    /// the first time a column takes a view into this chunk. Grown lazily
    /// rather than sized up front, since more than one schema (from
    /// sequential or same-name-different-schema blocks) can reference the
    /// same chunk.
    pub(crate) column_blocks: Vec<Option<u32>>,
}

impl SourceChunk {
    pub(crate) fn end(&self) -> u64 {
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
pub(crate) fn invalidate_block_cache(chunks: &mut VecDeque<SourceChunk>) {
    for chunk in chunks.iter_mut() {
        chunk.column_blocks.clear();
    }
}

/// Build the schema for a `COPY` header's columns. A header with no
/// explicit column list means "all columns, in table order" (see
/// `CopyHeader::columns`) — Phase 1 has no DDL parsing to name them from, so
/// placeholder names are used instead, sized to `field_count` (the first
/// row's field count).
pub(crate) fn schema_for(header: &CopyHeader, field_count: usize) -> SchemaRef {
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
pub(crate) struct RowBatcher {
    schema: SchemaRef,
    columns: Vec<StringViewBuilder>,
    rows_in_batch: usize,
    bytes_in_batch: usize,
    options: BatchOptions,
}

impl RowBatcher {
    pub(crate) fn new(schema: SchemaRef, options: BatchOptions) -> Self {
        let columns = (0..schema.fields().len()).map(|_| StringViewBuilder::new()).collect();
        Self { schema, columns, rows_in_batch: 0, bytes_in_batch: 0, options }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows_in_batch == 0
    }

    /// Columns in this block's schema — the field count a resumed stream
    /// needs to rebuild the same schema without re-reading the header.
    pub(crate) fn field_count(&self) -> usize {
        self.schema.fields().len()
    }

    pub(crate) fn should_flush(&self) -> bool {
        self.rows_in_batch >= self.options.max_rows
            || self.options.max_bytes.is_some_and(|max| self.bytes_in_batch >= max)
    }

    /// Append one raw (still-escaped) COPY TEXT data row.
    pub(crate) fn push_row(
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

    pub(crate) fn flush(&mut self) -> Result<RecordBatch> {
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
/// Push-mode entry point (`roadmap-phase1-mvp.md`, "Streaming API"):
/// internally drains the pull-mode [`crate::stream::table_stream`], so the two
/// share one scan loop. The callback may return [`ControlFlow::Break`] to stop
/// early, in which case the returned token resumes from just past the last
/// batch delivered to it — see [`crate::stream::TableStream::resume_token`].
/// `predicate` applies a post-parse row filter — see `table_stream`'s docs.
/// `cache` controls structure-cache consulting — see `table_stream`'s docs.
pub async fn read_table<S, F>(
    source: &S,
    table: &str,
    scan_options: &ScanOptions,
    batch_options: &BatchOptions,
    predicate: Option<Predicate>,
    cache: CacheMode,
    mut on_batch: F,
) -> Result<(crate::resolve::ResolvedSchema, Option<crate::stream::ResumeToken>)>
where
    S: ByteRangeSource,
    F: FnMut(RecordBatch) -> ControlFlow<()>,
{
    use futures::StreamExt;

    let mut stream = crate::stream::table_stream(
        source,
        table,
        scan_options.clone(),
        batch_options.clone(),
        predicate,
        None,
        cache,
    );
    while let Some(batch) = stream.next().await.transpose()? {
        if on_batch(batch).is_break() {
            return Ok((stream.resolved_schema(), Some(stream.resume_token())));
        }
    }
    Ok((stream.resolved_schema(), None))
}
