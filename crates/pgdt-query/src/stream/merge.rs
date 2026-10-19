//! One stream in file order over a plan's sub-streams: the k-way merge on
//! each batch's source offset, and the earliest failing piece's error
//! (`docs/design/decisions.md`, "D51" and "D52").

use std::sync::atomic::Ordering;

use arrow::array::RecordBatch;
use async_stream::try_stream;
use futures::StreamExt;

use pgdt_core::Error;

use super::note::EarlyStop;
use super::resume::ResumeToken;
use super::table::{StreamShared, TableStream};

/// `streams`, a plan's sub-streams in partition order, merged into one stream
/// yielding their batches in file order, publishing to `shared`: each
/// batch's source offset, a token stamped `fingerprint` — the whole query's,
/// which no sub-stream's is — resuming just past it, and the early stops and
/// dynamic-filter counts of every sub-stream together.
///
/// **One batch per sub-stream is the whole bound**: each yields in file order
/// and they are themselves in file order, so emitting the held batch that
/// begins earliest re-assembles the serial order while never holding more
/// than one batch each. The same two facts make it serial past the first
/// round, `pgdt query`'s merge reading the same way (`KD57`).
///
/// **The error is the lowest-indexed failed sub-stream's**, raised once every
/// sub-stream before it has drained and every batch it or a later one holds
/// is discarded, so an unchanged file names the same row on every run
/// (`docs/design/decisions.md`, "D52").
pub(super) fn in_file_order(
    mut streams: Vec<TableStream<'static>>,
    shared: StreamShared,
    fingerprint: u64,
) -> TableStream<'static> {
    let shared_for_stream = shared.clone();
    let inner = try_stream! {
        let mut slots: Vec<Slot> = streams.iter().map(|_| Slot::Empty).collect();
        let mut failed: Option<(usize, Error)> = None;
        let mut rows_emitted = 0u64;
        loop {
            // Everything at or after a failing sub-stream is dead, and
            // discarding what those slots hold is the load-bearing half: a
            // batch left there would be emitted once the ones before it
            // drained — rows past the error a serial replay never reached.
            let live = failed.as_ref().map_or(slots.len(), |(index, _)| *index);
            for slot in slots.iter_mut().skip(live) {
                *slot = Slot::Done;
            }
            // Refill every empty slot at once: every sub-stream in the first
            // round, and after it only the one just emitted from.
            let fills = streams
                .iter_mut()
                .zip(slots.iter_mut())
                .enumerate()
                .filter(|(index, (_, slot))| *index < live && matches!(slot, Slot::Empty))
                .map(|(index, (stream, slot))| async move {
                    match stream.next().await {
                        Some(Ok(batch)) => {
                            let (offset, token) =
                                (stream.batch_source_offset(), stream.resume_token());
                            *slot = Slot::Held { offset, token, batch };
                            None
                        }
                        Some(Err(err)) => {
                            *slot = Slot::Done;
                            Some((index, err))
                        }
                        None => {
                            *slot = Slot::Done;
                            None
                        }
                    }
                })
                .collect::<Vec<_>>();
            // `join_all` answers in argument order, which is partition order,
            // which is file order: its first failure is this round's lowest.
            let round = futures::future::join_all(fills).await.into_iter().flatten().next();
            // Round again rather than emitting, so the loop head marks dead
            // what this failure killed. It ends: a failure lowers `live`.
            if let Some(first) = round {
                failed = Some(first);
                continue;
            }
            let Some(next) = slots
                .iter()
                .enumerate()
                .filter_map(|(index, slot)| match slot {
                    Slot::Held { offset, .. } => Some((*offset, index)),
                    _ => None,
                })
                .min()
                .map(|(_, index)| index)
            else {
                break;
            };
            let Slot::Held { offset, token, batch } =
                std::mem::replace(&mut slots[next], Slot::Empty)
            else {
                unreachable!("the slot the merge picked is the one it just read")
            };
            rows_emitted += batch.num_rows() as u64;
            *shared_for_stream.batch_offset.lock().unwrap() = offset;
            *shared_for_stream.position.lock().unwrap() =
                ResumeToken { rows_emitted, query_fingerprint: fingerprint, ..token };
            publish(&shared_for_stream, &streams);
            yield batch;
        }
        publish(&shared_for_stream, &streams);
        if let Some((_, err)) = failed {
            Err(err)?;
        }
    };
    shared.into_stream(Box::pin(inner))
}

/// One sub-stream's place in the merge: at most one batch, held with the
/// offset it begins at — the merge key — and the token its sub-stream
/// reported for it.
enum Slot {
    /// Nothing held: polled in the next round.
    Empty,
    /// A batch waiting its turn.
    Held { offset: u64, token: ResumeToken, batch: RecordBatch },
    /// Drained, stopped at an error, or at or after one: never polled again.
    Done,
}

/// What the merged stream reports of its sub-streams together, as of now:
/// each block's early stop once, its unread bytes summed across the
/// sub-streams that read pieces of it, and the dynamic filter's counts summed.
fn publish(shared: &StreamShared, streams: &[TableStream<'static>]) {
    let mut stops: Vec<EarlyStop> = Vec::new();
    for stop in streams.iter().flat_map(TableStream::early_stops) {
        match stops.iter_mut().find(|held| held.header_offset == stop.header_offset) {
            Some(held) => {
                if let Some(unread) = stop.unread_bytes {
                    *held.unread_bytes.get_or_insert(0) += unread;
                }
            }
            None => stops.push(stop),
        }
    }
    stops.sort_by_key(|stop| stop.header_offset);
    *shared.early_stops.lock().unwrap() = stops;
    let groups = streams.iter().map(TableStream::dynamic_filter_pruned_groups).sum();
    let rows = streams.iter().map(TableStream::dynamic_filter_pruned_rows).sum();
    shared.dynamic_pruned.store(groups, Ordering::Relaxed);
    shared.dynamic_rows.store(rows, Ordering::Relaxed);
}
