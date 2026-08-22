//! Pull-mode streaming API (`docs/design/mvp.md`, "Streaming API").
//!
//! [`table_stream`] is the primitive: an async `Stream<Item = Result<RecordBatch>>`
//! built directly on [`CopyScanner`]/[`RowBatcher`], the same machinery
//! [`crate::batch::read_table`] (push mode) now drives internally rather than
//! duplicating. [`ResumeToken`] lets a caller stop consuming partway through
//! and pick back up later in the same process — it holds no public fields
//! (`mvp.md` is explicit that it must stay opaque), so its representation is
//! free to change without an API break.
//!
//! **Cache-consulting** (`mvp.md`, "Index / structure cache"): a
//! [`CacheMode::Enabled`] cache is consulted up front and turned into a
//! sequence of [`Segment`]s — one [`Segment::Known`] per already-cached
//! block matching the query table (replayed for its rows, at zero I/O cost
//! for every non-matching block in between) plus one trailing
//! [`Segment::Live`] covering whatever's past the cache's watermark, which
//! is scanned as today, with every block it finds (matching or not) handed
//! to a [`Recorder`] and persisted back after each one completes.

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use arrow::array::RecordBatch;
use arrow::buffer::Buffer;
use async_stream::try_stream;
use futures::Stream;

use crate::batch::{BatchOptions, RowBatcher, SourceChunk, invalidate_block_cache, schema_for};
use crate::cache::CacheMode;
use crate::copy::{CopyHeader, DELIMITER};
use crate::index::{CopyBlock, DumpIndex};
use crate::io::ByteRangeSource;
use crate::predicate::Predicate;
use crate::scan::{CopyScanner, CopyStart, Event, ScanOptions};
use crate::{Error, Result};

/// State for a `COPY` block whose table matches the query: the batcher
/// accumulating its rows, and — when a [`Predicate`] was given — the column
/// index it was resolved to against this block's own schema (schemas can
/// differ block-to-block, e.g. a headerless block's placeholder names).
type Active = (u64, CopyHeader, RowBatcher, Option<usize>);

/// Resolve `predicate`'s column name against `schema`, once per block. `Ok(None)`
/// when there is no predicate to apply.
fn resolve_predicate_index(
    predicate: Option<&Predicate>,
    schema: &arrow::datatypes::SchemaRef,
    header_offset: u64,
) -> Result<Option<usize>> {
    let Some(predicate) = predicate else { return Ok(None) };
    schema
        .fields()
        .iter()
        .position(|f| f.name() == &predicate.column)
        .map(Some)
        .ok_or(Error::UnknownPredicateColumn { header_offset, column: predicate.column.clone() })
}

/// One piece of a [`table_stream`] scan: either replaying a block the cache
/// already knew about, or walking unscanned territory live. See the module
/// docs.
enum Segment {
    /// `[start, end)`: an already-cached block matching the query table.
    Known { start: u64, end: u64 },
    /// `[start, size)`: unscanned territory, discovered as it's walked.
    Live { start: u64 },
}

/// Accumulates newly-discovered blocks during a [`Segment::Live`] scan and
/// persists them back to the cache. Seeded from whatever the cache already
/// had, so a persisted index stays complete rather than shrinking to just
/// this call's discoveries.
struct Recorder {
    cache: CacheMode,
    index: DumpIndex,
}

impl Recorder {
    /// Record `block`, ignoring it if a block at the same `header_offset` is
    /// already present. A resumed stream's live segment can re-walk bytes
    /// the base index already covers (see `docs/design/mvp.md`'s "Known
    /// gaps") — without this guard that would duplicate an entry rather
    /// than just redundantly re-read some bytes.
    fn record(&mut self, block: CopyBlock) {
        if !self.index.blocks.iter().any(|b| b.header_offset == block.header_offset) {
            self.index.blocks.push(block);
        }
    }

    /// `end.end_offset` is always a safe, resumable watermark — the scanner
    /// is back in its `Outside` state there — so every persisted
    /// `scanned_through` is valid for a later query to replay from.
    fn persist(&mut self, scanned_through: u64) -> Result<()> {
        self.index.scanned_through = self.index.scanned_through.max(scanned_through);
        self.cache.save(&self.index)
    }
}

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
    predicate: Option<&Predicate>,
) -> Result<(CopyScanner, Option<Active>)> {
    let scanner = CopyScanner::resume(
        token.offset,
        token.in_copy.as_ref().map(|ic| (ic.header_offset, ic.rows_in_block)),
    );
    let active = token
        .in_copy
        .as_ref()
        .map(|ic| {
            let schema = schema_for(&ic.header, ic.field_count);
            let predicate_index = resolve_predicate_index(predicate, &schema, ic.header_offset)?;
            Ok::<_, Error>((
                ic.header_offset,
                ic.header.clone(),
                RowBatcher::new(schema, batch_options.clone()),
                predicate_index,
            ))
        })
        .transpose()?;
    Ok((scanner, active))
}

fn snapshot(scanner: &CopyScanner, active: &Option<Active>, rows_emitted: u64) -> ResumeToken {
    let in_copy = active.as_ref().map(|(header_offset, header, batcher, _)| InCopyResume {
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
///
/// `predicate` applies `docs/design/mvp.md`'s post-parse row filter
/// (`docs/design/mvp.md`, "Predicate filtering"): `None` yields every row, as
/// before; `Some` drops any row whose named column doesn't satisfy it, after
/// that row has been fully unescaped. Referencing a column absent from a
/// matching block's own schema is `Error::UnknownPredicateColumn`.
///
/// `cache` controls structure-cache consulting (`docs/design/mvp.md`,
/// "Index / structure cache"): `CacheMode::Disabled` is pure streaming with
/// no side effects; `CacheMode::Enabled` replays already-cached blocks
/// matching `table` at zero I/O cost for everything in between, then scans
/// live from the cache's watermark, persisting each newly-discovered block
/// (matching or not) as it completes — so a later query against the same
/// dump gets progressively cheaper. `resume` takes priority over cache
/// replay: it always starts a single live segment at the token's offset (see
/// `docs/design/mvp.md`'s "Known gaps" for what that costs in the rare case
/// of resuming from inside a would-be replay).
pub fn table_stream<'a, S>(
    source: &'a S,
    table: &str,
    scan_options: ScanOptions,
    batch_options: BatchOptions,
    predicate: Option<Predicate>,
    resume: Option<ResumeToken>,
    cache: CacheMode,
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

        let base_index = cache.load()?.unwrap_or_default();
        let segments: Vec<Segment> = match &resume {
            Some(token) => vec![Segment::Live { start: token.offset }],
            None => {
                let mut segs: Vec<Segment> = base_index
                    .blocks_for(&table)
                    .map(|b| Segment::Known { start: b.header_offset, end: b.end_offset })
                    .collect();
                segs.push(Segment::Live { start: base_index.scanned_through });
                segs
            }
        };
        let mut recorder: Option<Recorder> = (!matches!(cache, CacheMode::Disabled))
            .then(|| Recorder { cache: cache.clone(), index: base_index });

        // Only the first segment can start mid-block (a resumed stream); a
        // fresh `CopyScanner` for it is prebuilt here so `resume_state`'s
        // logic isn't duplicated below.
        let (mut active, mut first_scanner) = match &resume {
            Some(token) => {
                let (scanner, active) = resume_state(token, &batch_options, predicate.as_ref())?;
                (active, Some(scanner))
            }
            None => (None, None),
        };
        // A matching header with no column list, waiting on its first row to
        // learn the field count. Never non-empty across a resume point: a
        // stream only yields right after a flush, and by then any pending
        // headerless block has already seen its first row (see `active`).
        let mut pending: Option<(CopyHeader, u64)> = None;
        let mut rows_emitted = resume.as_ref().map_or(0, |t| t.rows_emitted);

        for (segment_index, segment) in segments.into_iter().enumerate() {
            let is_live = matches!(segment, Segment::Live { .. });
            let (seg_start, seg_end) = match &segment {
                Segment::Known { start, end } => (*start, *end),
                Segment::Live { start } => (*start, size),
            };

            let mut scanner = if segment_index == 0 {
                first_scanner.take().unwrap_or_else(|| CopyScanner::resume(seg_start, None))
            } else {
                CopyScanner::resume(seg_start, None)
            };
            let mut read_pos = seg_start;
            let mut buf: Vec<u8> = Vec::with_capacity(scan_options.chunk_size);
            let mut chunks: VecDeque<SourceChunk> = VecDeque::new();
            // The most recent `CopyStart` in this (live) segment, regardless
            // of table match — mirrors `build_index`'s `pending`, tracked
            // separately from the table-matching `pending`/`active` above
            // since a live segment must record every block, not just ones
            // the query cares about.
            let mut block_start: Option<CopyStart> = None;

            loop {
                let want = scan_options.chunk_size.min((seg_end - read_pos) as usize);
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
                let eof = read_pos >= seg_end;

                while let Some(event) = scanner.next_event(&buf, eof)? {
                    match event {
                        Event::CopyStart(start) => {
                            if is_live && recorder.is_some() {
                                block_start = Some(start.clone());
                            }
                            if start.header.matches(&table) {
                                if start.header.columns.is_empty() {
                                    pending = Some((start.header, start.header_offset));
                                } else {
                                    let schema =
                                        schema_for(&start.header, start.header.columns.len());
                                    let predicate_index = resolve_predicate_index(
                                        predicate.as_ref(),
                                        &schema,
                                        start.header_offset,
                                    )?;
                                    active = Some((
                                        start.header_offset,
                                        start.header,
                                        RowBatcher::new(schema, batch_options.clone()),
                                        predicate_index,
                                    ));
                                }
                            }
                        }
                        Event::Row(row) => {
                            if let Some((header, header_offset)) = pending.take() {
                                let field_count =
                                    memchr::memchr_iter(DELIMITER, row.raw).count() + 1;
                                let schema = schema_for(&header, field_count);
                                let predicate_index = resolve_predicate_index(
                                    predicate.as_ref(),
                                    &schema,
                                    header_offset,
                                )?;
                                active = Some((
                                    header_offset,
                                    header,
                                    RowBatcher::new(schema, batch_options.clone()),
                                    predicate_index,
                                ));
                            }
                            if let Some((header_offset, _, batcher, predicate_index)) =
                                active.as_mut()
                            {
                                let keep = match (predicate.as_ref(), *predicate_index) {
                                    (Some(pred), Some(col)) => pred.matches(row.raw, col)?,
                                    _ => true,
                                };
                                if keep {
                                    batcher.push_row(
                                        *header_offset,
                                        row.offset,
                                        row.raw,
                                        &mut chunks,
                                    )?;
                                }
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
                        Event::CopyEnd(end) => {
                            pending = None;
                            if let Some((_, _, mut batcher, _)) = active.take()
                                && !batcher.is_empty()
                            {
                                let batch = batcher.flush()?;
                                invalidate_block_cache(&mut chunks);
                                rows_emitted += batch.num_rows() as u64;
                                *position_for_stream.lock().unwrap() =
                                    snapshot(&scanner, &active, rows_emitted);
                                yield batch;
                            }
                            if let (Some(start), Some(rec)) =
                                (block_start.take(), recorder.as_mut())
                            {
                                rec.record(CopyBlock {
                                    header: start.header,
                                    header_offset: start.header_offset,
                                    data_offset: start.data_offset,
                                    terminator_offset: end.terminator_offset,
                                    end_offset: end.end_offset,
                                    row_count: end.row_count,
                                    sparse_index: None,
                                    column_stats: None,
                                });
                                rec.persist(end.end_offset)?;
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
                    break;
                }
                if buf.len() > scan_options.max_line_bytes {
                    Err(Error::LineTooLong {
                        offset: scanner.position(),
                        limit: scan_options.max_line_bytes,
                    })?;
                }
            }

            if is_live && let Some(rec) = recorder.as_mut() {
                rec.persist(size)?;
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
