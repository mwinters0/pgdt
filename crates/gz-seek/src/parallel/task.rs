//! The work a parallel pass hands its caller, to run on whatever thread it
//! has: a chunk's speculation, or a chunk's decode through the engine.

use crate::chunk::{ChunkDecode, ChunkDecoded};
use crate::speculate::{Speculated, Speculation};

/// Which of a pass's tasks this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId {
    chunk: u64,
    kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Kind {
    Speculation,
    Decode { generation: u64 },
}

impl TaskId {
    pub(crate) fn speculation(chunk: u64) -> TaskId {
        TaskId { chunk, kind: Kind::Speculation }
    }

    pub(crate) fn decode(chunk: u64, generation: u64) -> TaskId {
        TaskId { chunk, kind: Kind::Decode { generation } }
    }

    /// The chunk of the file the task is about.
    pub fn chunk(&self) -> u64 {
        self.chunk
    }

    /// A decode's generation; `None` for a speculation.
    pub(crate) fn generation(&self) -> Option<u64> {
        match self.kind {
            Kind::Speculation => None,
            Kind::Decode { generation } => Some(generation),
        }
    }
}

/// One task of a parallel pass, fed by its caller: `Send`, holding no borrow,
/// so it runs on whatever thread the caller has.
///
/// A **speculation** finds where its chunk may begin and decodes it with the
/// crate's own decoder, holding about a chunk of compressed bytes and at most
/// the pass's plaintext limit, twice over while it writes markers — or, over a
/// chunk holding a BGZF header, reads its guess off the member chain, holding
/// the compressed bytes alone. A
/// **decode** decodes a chunk through the engine from where a speculation
/// found it begins, holding its 32 KiB window, an engine
/// ([`DECODER_BYTES`](crate::DECODER_BYTES)) and room for at most that
/// speculation's plaintext.
pub struct ChunkTask(Work);

enum Work {
    Speculation(Speculation),
    Decode(ChunkDecode),
}

impl ChunkTask {
    pub(crate) fn speculation(task: Speculation) -> ChunkTask {
        ChunkTask(Work::Speculation(task))
    }

    pub(crate) fn decode(task: ChunkDecode) -> ChunkTask {
        ChunkTask(Work::Decode(task))
    }

    /// Which task this is.
    pub fn id(&self) -> TaskId {
        match &self.0 {
            Work::Speculation(task) => TaskId::speculation(task.chunk()),
            Work::Decode(task) => TaskId::decode(task.chunk(), task.generation()),
        }
    }

    /// The compressed offset of the next byte the task wants, `None` once it
    /// is done.
    pub fn wants(&self) -> Option<u64> {
        match &self.0 {
            Work::Speculation(task) => task.wants(),
            Work::Decode(task) => task.wants(),
        }
    }

    /// Take the file's bytes from [`ChunkTask::wants`] on, as many as the
    /// caller has, and go as far as they go.
    pub fn supply(&mut self, bytes: &[u8]) {
        match &mut self.0 {
            Work::Speculation(task) => task.supply(bytes),
            Work::Decode(task) => task.supply(bytes),
        }
    }

    /// What the task came to, for [`Parallel::done`](crate::Parallel::done);
    /// one not yet done hands back nothing.
    pub fn finish(self) -> ChunkDone {
        ChunkDone(match self.0 {
            Work::Speculation(task) => Finished::Speculated(task.finish()),
            Work::Decode(task) => Finished::Decoded(task.finish()),
        })
    }
}

/// What a [`ChunkTask`] came to.
pub struct ChunkDone(pub(crate) Finished);

pub(crate) enum Finished {
    Speculated(Speculated),
    Decoded(ChunkDecoded),
}

impl ChunkDone {
    /// Which task this came from.
    pub fn id(&self) -> TaskId {
        match &self.0 {
            Finished::Speculated(done) => TaskId::speculation(done.chunk),
            Finished::Decoded(done) => TaskId::decode(done.chunk, done.generation),
        }
    }

    /// The bytes it holds until the pass takes or drops it.
    pub fn held(&self) -> usize {
        match &self.0 {
            Finished::Speculated(done) => done.found.as_ref().map_or(0, |guess| guess.held()),
            Finished::Decoded(done) => done.held(),
        }
    }
}
