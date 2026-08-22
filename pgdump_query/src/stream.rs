//! Pull-mode streaming API (`docs/design/mvp.md`, "Streaming API").
//!
//! [`table_stream`] is the primitive: an async `Stream<Item = Result<RecordBatch>>`
//! built directly on [`CopyScanner`]/[`RowBatcher`], the same machinery
//! [`crate::batch::read_table`] (push mode) now drives internally rather than
//! duplicating. [`ResumeToken`] lets a caller stop consuming partway through
//! and pick back up later in the same process — it holds no public fields
//! (`mvp.md` is explicit that it must stay opaque), so its representation is
//! free to change without an API break.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use arrow::buffer::Buffer;
use async_stream::try_stream;
use futures::Stream;

use crate::batch::{BatchOptions, RowBatcher, SourceChunk, invalidate_block_cache, schema_for};
use crate::copy::{CopyHeader, DELIMITER};
use crate::io::ByteRangeSource;
use crate::scan::{CopyScanner, Event, ScanOptions};
use crate::{Error, Result};

/// Opaque cursor into a [`table_stream`]/[`crate::batch::read_table`]
/// consumption, sufficient to resume from just past the last batch a caller
/// accepted. Valid only within the process that produced it — persisting it
/// across a restart is out of scope for Phase 1 (`mvp.md`).
#[derive(Debug, Clone)]
pub struct ResumeToken {
    offset: u64,
    rows_emitted: u64,
    /// Reserved for the structural cache's generation stamp. The cache
    /// doesn't exist yet (`docs/status/STATUS.md`, "Not started"), so this
    /// is always 0 in Phase 1; carrying the field now avoids a later
    /// breaking change to this already-opaque type.
    #[allow(dead_code)]
    generation: u64,
    in_copy: Option<InCopyResume>,
}

#[derive(Debug, Clone)]
struct InCopyResume {
    header: CopyHeader,
    header_offset: u64,
    rows_in_block: u64,
    field_count: usize,
}

impl ResumeToken {
    fn start() -> Self {
        Self { offset: 0, rows_emitted: 0, generation: 0, in_copy: None }
    }
}

/// A pull-mode stream of `Utf8View` `RecordBatch`es for one table query.
/// Construct with [`table_stream`].
pub struct TableStream<'a> {
    inner: Pin<Box<dyn Stream<Item = Result<RecordBatch>> + Send + 'a>>,
    position: Arc<Mutex<ResumeToken>>,
}

impl<'a> Stream for TableStream<'a> {
    type Item = Result<RecordBatch>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl<'a> TableStream<'a> {
    /// A token that resumes this stream from just past the last batch
    /// [`futures::StreamExt::next`] returned (or from the start, if nothing
    /// has been polled yet).
    pub fn resume_token(&self) -> ResumeToken {
        self.position.lock().unwrap().clone()
    }
}

/// Reconstruct the in-progress block state a [`ResumeToken`] captured, if
/// any: the scanner's row counter (so a later `CopyEnd` reports the block's
/// true total, not just rows-since-resume) and a fresh [`RowBatcher`] built
/// from the same schema the original block used.
fn resume_state(
    token: &ResumeToken,
    batch_options: &BatchOptions,
) -> (CopyScanner, Option<(u64, CopyHeader, RowBatcher)>) {
    let scanner = CopyScanner::resume(
        token.offset,
        token.in_copy.as_ref().map(|ic| (ic.header_offset, ic.rows_in_block)),
    );
    let active = token.in_copy.as_ref().map(|ic| {
        let schema = schema_for(&ic.header, ic.field_count);
        (ic.header_offset, ic.header.clone(), RowBatcher::new(schema, batch_options.clone()))
    });
    (scanner, active)
}

fn snapshot(
    scanner: &CopyScanner,
    active: &Option<(u64, CopyHeader, RowBatcher)>,
    rows_emitted: u64,
) -> ResumeToken {
    let in_copy = active.as_ref().map(|(header_offset, header, batcher)| InCopyResume {
        header: header.clone(),
        header_offset: *header_offset,
        rows_in_block: scanner.in_copy_rows().unwrap_or(0),
        field_count: batcher.field_count(),
    });
    ResumeToken { offset: scanner.position(), rows_emitted, generation: 0, in_copy }
}

/// Pull-mode entry point: stream `Utf8View` `RecordBatch`es for every row of
/// every `COPY` block whose table matches `table` (qualified or bare — see
/// [`CopyHeader::matches`]). A table with zero rows yields no batches.
///
/// `resume` continues a previous consumption from a [`ResumeToken`] it
/// produced; `None` starts from the beginning of `source`.
pub fn table_stream<'a, S>(
    source: &'a S,
    table: &str,
    scan_options: ScanOptions,
    batch_options: BatchOptions,
    resume: Option<ResumeToken>,
) -> TableStream<'a>
where
    S: ByteRangeSource + 'a,
{
    let table = table.to_string();
    let start_token = resume.clone().unwrap_or_else(ResumeToken::start);
    let position = Arc::new(Mutex::new(start_token));
    let position_for_stream = Arc::clone(&position);

    let inner = try_stream! {
        let size = source.size().await?;
        let mut buf: Vec<u8> = Vec::with_capacity(scan_options.chunk_size);
        let mut chunks: VecDeque<SourceChunk> = VecDeque::new();

        let (mut scanner, mut read_pos, mut active, mut rows_emitted) = match resume {
            None => (CopyScanner::new(), 0u64, None, 0u64),
            Some(token) => {
                let offset = token.offset;
                let rows_emitted = token.rows_emitted;
                let (scanner, active) = resume_state(&token, &batch_options);
                (scanner, offset, active, rows_emitted)
            }
        };
        // A matching header with no column list, waiting on its first row to
        // learn the field count. Never non-empty across a resume point: a
        // stream only yields right after a flush, and by then any pending
        // headerless block has already seen its first row (see `active`).
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
                    Event::CopyStart(start) if start.header.matches(&table) => {
                        if start.header.columns.is_empty() {
                            pending = Some((start.header, start.header_offset));
                        } else {
                            let schema = schema_for(&start.header, start.header.columns.len());
                            active = Some((
                                start.header_offset,
                                start.header,
                                RowBatcher::new(schema, batch_options.clone()),
                            ));
                        }
                    }
                    Event::CopyStart(_) => {}
                    Event::Row(row) => {
                        if let Some((header, header_offset)) = pending.take() {
                            let field_count = memchr::memchr_iter(DELIMITER, row.raw).count() + 1;
                            let schema = schema_for(&header, field_count);
                            active = Some((
                                header_offset,
                                header,
                                RowBatcher::new(schema, batch_options.clone()),
                            ));
                        }
                        if let Some((header_offset, _, batcher)) = active.as_mut() {
                            batcher.push_row(*header_offset, row.offset, row.raw, &mut chunks)?;
                            if batcher.should_flush() {
                                let batch = batcher.flush()?;
                                invalidate_block_cache(&mut chunks);
                                rows_emitted += batch.num_rows() as u64;
                                *position_for_stream.lock().unwrap() =
                                    snapshot(&scanner, &active, rows_emitted);
                                yield batch;
                            }
                        }
                    }
                    Event::CopyEnd(_) => {
                        pending = None;
                        if let Some((_, _, mut batcher)) = active.take()
                            && !batcher.is_empty()
                        {
                            let batch = batcher.flush()?;
                            invalidate_block_cache(&mut chunks);
                            rows_emitted += batch.num_rows() as u64;
                            *position_for_stream.lock().unwrap() =
                                snapshot(&scanner, &active, rows_emitted);
                            yield batch;
                        }
                    }
                }
            }

            let used = scanner.take_consumed();
            buf.drain(..used);

            // Everything before the scanner's new position has already had
            // its chance to be referenced by a zero-copy view (that happens
            // synchronously above, before we get here), so it's safe to drop.
            let floor = scanner.position();
            while chunks.front().is_some_and(|c| c.end() <= floor) {
                chunks.pop_front();
            }

            if eof {
                return;
            }
            if buf.len() > scan_options.max_line_bytes {
                Err(Error::LineTooLong {
                    offset: scanner.position(),
                    limit: scan_options.max_line_bytes,
                })?;
            }
        }
    };

    TableStream { inner: Box::pin(inner), position }
}

/// Blocking [`Iterator`] wrapper over a [`TableStream`], for sync callers
/// (the CLI) with no ambient `tokio` runtime — nesting this inside one
/// (e.g. a `#[tokio::main]` function) panics, same as any other
/// `Runtime::block_on` call.
pub struct BlockingTableIter<'a> {
    stream: TableStream<'a>,
    rt: tokio::runtime::Runtime,
}

impl<'a> BlockingTableIter<'a> {
    pub fn new(stream: TableStream<'a>) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread().build()?;
        Ok(Self { stream, rt })
    }

    /// A token that resumes from just past the last batch [`Iterator::next`]
    /// returned.
    pub fn resume_token(&self) -> ResumeToken {
        self.stream.resume_token()
    }
}

impl<'a> Iterator for BlockingTableIter<'a> {
    type Item = Result<RecordBatch>;

    fn next(&mut self) -> Option<Self::Item> {
        use futures::StreamExt;
        self.rt.block_on(self.stream.next())
    }
}
