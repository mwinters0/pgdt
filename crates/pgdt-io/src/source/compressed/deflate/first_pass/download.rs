//! The one download a fetched first pass reads: the object's compressed bytes
//! fetched a piece at a time, each piece once, and held for the frontier and
//! the tasks ahead of it until the frontier has passed it.
//!
//! **Each piece is fetched once.** A speculation reads its chunk, the engine's
//! decode of the chunk reads it again, and the frontier reads every byte no
//! decode it took covers (`gz_seek::ChunkTask::wants`); over a transport each
//! of those reads would be a ranged GET of its own. Here each is a slice of
//! the piece the first of them fetched, a second reader of a piece in flight
//! waiting for that fetch rather than making its own.
//!
//! **What is held is bounded by a room, in pieces, from the frontier's on.**
//! A piece the frontier has passed is dropped, every task reading it being of
//! no further use, and a task asking for one past the room is refused, so it
//! hands back nothing and the frontier meets those bytes itself; the frontier
//! is never refused. The tasks ahead are handed out no further than the room
//! reaches ([`Download::last_chunk`]), so a task is refused only where its
//! chunk's speculation runs past the piece after it.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use tokio::sync::OnceCell;

use pgdt_core::Result;

use crate::source::ByteRangeSource;

/// The fetched object's pieces, fetched once each through the transport.
pub(super) struct Download {
    raw: Arc<dyn ByteRangeSource>,
    /// A piece's length: the chunk length, so each chunk is one piece.
    piece: u64,
    stored_size: u64,
    held: Mutex<Held>,
}

/// The pieces held, and where they may lie.
struct Held {
    /// The first piece that may be held: the one the frontier stands in.
    base: u64,
    /// How many pieces from `base` on may be held.
    room: u64,
    /// Each piece fetched or being fetched, by its index.
    pieces: BTreeMap<u64, Arc<OnceCell<Bytes>>>,
}

impl Download {
    /// The download of `raw`, `stored_size` bytes long, in pieces of `piece`
    /// bytes, with room for the frontier's two.
    pub(super) fn new(raw: Arc<dyn ByteRangeSource>, piece: NonZeroU64, stored_size: u64) -> Self {
        let held = Held { base: 0, room: 2, pieces: BTreeMap::new() };
        Download { raw, piece: piece.get(), stored_size, held: Mutex::new(held) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Hold up to `pieces` from the frontier's on, at least its own two —
    /// the piece it stands in and the one its next read may begin.
    pub(super) fn set_room(&self, pieces: u64) {
        self.lock().room = pieces.max(2);
    }

    /// The last chunk a speculation may be handed out on: one whose next
    /// chunk's piece, where its search stops, is still inside the room.
    pub(super) fn last_chunk(&self) -> u64 {
        let held = self.lock();
        held.base.saturating_add(held.room).saturating_sub(2)
    }

    /// The frontier stands at compressed offset `offset`: drop every piece
    /// before the one it stands in.
    pub(super) fn release_before(&self, offset: u64) {
        let mut held = self.lock();
        let base = held.base.max(offset / self.piece);
        held.base = base;
        held.pieces = held.pieces.split_off(&base);
    }

    /// Drop every piece and refuse every read: the pass has handed over.
    pub(super) fn clear(&self) {
        let mut held = self.lock();
        held.pieces.clear();
        held.base = u64::MAX;
        held.room = 0;
    }

    /// A task's read: the bytes from `at` to its piece's end, empty at the
    /// object's end, or `None` where that piece lies outside the room.
    pub(super) async fn read(&self, at: u64) -> Result<Option<Bytes>> {
        let index = at / self.piece;
        let cell = {
            let mut held = self.lock();
            if index < held.base || index - held.base >= held.room {
                return Ok(None);
            }
            Arc::clone(held.pieces.entry(index).or_default())
        };
        self.sliced(&cell, index, at).await.map(Some)
    }

    /// The frontier's read: [`Download::read`] from the piece it stands in,
    /// which is never refused — a read behind what is held being fetched
    /// without holding it.
    pub(super) async fn read_frontier(&self, at: u64) -> Result<Bytes> {
        self.release_before(at);
        match self.read(at).await? {
            Some(bytes) => Ok(bytes),
            None => {
                let index = at / self.piece;
                self.sliced(&OnceCell::new(), index, at).await
            }
        }
    }

    /// The bytes from `at` to the end of piece `index`, fetching it into
    /// `cell` where nothing has.
    async fn sliced(&self, cell: &OnceCell<Bytes>, index: u64, at: u64) -> Result<Bytes> {
        let from = index * self.piece;
        if from >= self.stored_size {
            return Ok(Bytes::new());
        }
        let len = self.piece.min(self.stored_size - from);
        let piece = cell
            .get_or_try_init(|| self.raw.read_range(from, usize::try_from(len).unwrap_or(0)))
            .await?;
        Ok(piece.slice(usize::try_from(at - from).unwrap_or(piece.len()).min(piece.len())..))
    }

    /// How many pieces are held, fetched or being fetched.
    #[cfg(test)]
    pub(super) fn held(&self) -> usize {
        self.lock().pieces.len()
    }
}

#[cfg(test)]
mod tests;
