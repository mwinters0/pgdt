//! A local BGZF file's index, read at open off its members' headers and
//! trailers with nothing decoded (`gz_seek::Walk`) — the place the `.xz`
//! footer walk has, and announced as it is, being the same wait before any
//! scan (`docs/design/decisions.md`, "D64").
//!
//! **The walk is how a BGZF file's index is found, nothing else**: what it
//! builds is the checkpoint list a first pass over the file builds, a unit of
//! whole members at least [`CHECKPOINT_SPACING`] long, every one a member start
//! (`docs/design/decisions.md`, "D116"). It reads every part of the file, a
//! member being at most 64 KiB, which locally buys a parallel first scan; a
//! fetched file is read by a first pass instead, downloaded once
//! (`docs/design/roadmap-P15-gzip.md`, "Settled in the grilling"). A file whose
//! first member is BGZF's and whose chain then breaks — a later member not
//! BGZF's, a length running past the file — is a gzip file of several members
//! or a damaged one, so it is handed to a first pass, which reads the first
//! and refuses the second as it would without the walk.

use std::num::NonZeroU64;
use std::ops::ControlFlow;

use pgdt_core::Result;

use super::super::Positioned;
use super::{CHECKPOINT_SPACING, GZIP, GzipIndex, INPUT_CHUNK};

/// Whether `leading`, a file's first bytes, open with a BGZF member's header:
/// gzip's magic, `FEXTRA` and the `BC` subfield first in it, which is how
/// recognition tells a file whose index is walked from one whose index a
/// first pass builds.
pub(crate) fn is_bgzf(leading: &[u8]) -> bool {
    gz_seek::bgzf_member_len(leading).is_some()
}

/// How many bytes one read of the walk asks for at least: the input chunk a
/// decode pulls. A member is at most 64 KiB, so the walk touches every part of
/// the file whatever it reads; the chunk sets only how many reads that takes.
const WALK_CHUNK: u64 = INPUT_CHUNK;

/// Walk the local BGZF `file`, `stored_size` bytes, for its index, saying so
/// before and after as `named`: `None`, said too, where its chain breaks
/// and a first pass must read it.
///
/// # Errors
///
/// A read of the file failing.
pub(crate) fn walk_file(
    named: &str,
    file: &std::fs::File,
    stored_size: u64,
) -> Result<Option<GzipIndex>> {
    walk_file_spaced(named, file, stored_size, CHECKPOINT_SPACING)
}

/// [`walk_file`] coalescing into units of at least `spacing`.
fn walk_file_spaced(
    named: &str,
    file: &std::fs::File,
    stored_size: u64,
    spacing: NonZeroU64,
) -> Result<Option<GzipIndex>> {
    let mut walk = Walking::begin(named, stored_size, spacing);
    let mut buf = Vec::new();
    loop {
        let (offset, len) = walk.next_read();
        buf.resize(len, 0);
        let got = Positioned::read_at(file, offset, &mut buf)?;
        if let ControlFlow::Break(walked) = walk.supply(offset, &buf[..got]) {
            return Ok(walked);
        }
    }
}

/// One walk, its status lines and the reads it asks for.
struct Walking<'a> {
    walk: gz_seek::Walk,
    named: &'a str,
    stored_size: u64,
}

impl<'a> Walking<'a> {
    fn begin(named: &'a str, stored_size: u64, spacing: NonZeroU64) -> Self {
        tracing::info!(source = %named, codec = %GZIP, "BGZF member walk started");
        Self { walk: gz_seek::Walk::new(stored_size, spacing), named, stored_size }
    }

    /// The next read: the walk's request, read on to [`WALK_CHUNK`] within
    /// the file.
    fn next_read(&self) -> (u64, usize) {
        let request = self.walk.request();
        let asked = request.offset + request.len as u64;
        let end = (request.offset + WALK_CHUNK).min(self.stored_size).max(asked);
        (request.offset, usize::try_from(end - request.offset).unwrap_or(usize::MAX))
    }

    /// The bytes at `offset` handed to the walk: the index once it is done,
    /// `None` where the chain broke, each said; or the next read.
    fn supply(&mut self, offset: u64, bytes: &[u8]) -> ControlFlow<Option<GzipIndex>> {
        match self.walk.supply(offset, bytes) {
            Ok(gz_seek::WalkStep::Need(_)) => ControlFlow::Continue(()),
            Ok(gz_seek::WalkStep::Done(index)) => {
                let index = GzipIndex { index, windows: None };
                tracing::info!(
                    source = %self.named,
                    codec = %GZIP,
                    units = index.unit_count(),
                    members = index.member_count(),
                    "BGZF member walk complete",
                );
                ControlFlow::Break(Some(index))
            }
            Err(cause) => {
                tracing::warn!(
                    source = %self.named,
                    codec = %GZIP,
                    %cause,
                    "BGZF member walk stopped: the file opens with a BGZF member and its chain \
                     of members does not hold, so a first pass reads it as any gzip file",
                );
                ControlFlow::Break(None)
            }
        }
    }
}

#[cfg(test)]
mod tests;
