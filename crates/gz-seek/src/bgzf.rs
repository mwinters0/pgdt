//! BGZF: a gzip file whose members state their own lengths, walked into an
//! index without decoding anything.
//!
//! Every BGZF member carries a `BC` extra subfield giving its compressed size
//! (`BSIZE`, the member's length less one) and holds at most 64 KiB of
//! plaintext, which its trailer's `ISIZE` states. So the chain of members is
//! read off their headers and trailers alone: each member's 18-byte header
//! says where the next begins, and the four bytes before that say how much
//! plaintext it held. The plaintext's length is then exact, each `ISIZE`
//! being whole rather than modulo 2³².
//!
//! **Members are coalesced into units** of at least the spacing, each unit
//! beginning at a member start: a 64 KiB unit would make a caller's block
//! cache and partition grain absurd. A unit of whole members needs no CRC-32
//! of its own, its members' trailers checking it as it is decoded. A
//! [`Forward`](crate::Forward) pass over the file places the same checkpoints,
//! so a caller with no reason to walk — one whose file arrives once, in
//! order — builds the walk's index by decoding it.
//!
//! # The walk is a machine; the fetching is the caller's
//!
//! [`Walk`] hands out a [`Request`] for a range of the file and takes the
//! bytes back through [`Walk::supply`], holding no borrow, so a caller whose
//! transport is asynchronous drives it across an `await`. A supply may run
//! past what was asked: the walk reads as many members as the bytes hold, so
//! a driver reading ahead pays one read for many members.

use std::num::NonZeroU64;

use crate::error::{Error, Result};
use crate::index::{Checkpoint, End, Index};
use crate::member::GZIP_MAGIC;

/// The bytes a BGZF member's header occupies, its `BC` subfield included:
/// what recognition reads.
pub const BGZF_HEADER_LEN: usize = 18;

/// The most plaintext one BGZF member holds.
pub const BGZF_MAX_PLAINTEXT: u32 = 64 * 1024;

/// A member's trailer and the smallest header: no BGZF member is shorter.
pub(crate) const MIN_MEMBER: u64 = BGZF_HEADER_LEN as u64 + 8;

/// The length of the BGZF member whose header begins `header`, `None` where
/// the bytes are not a BGZF header: gzip's magic, `FEXTRA` set, and a `BC`
/// subfield of two bytes first in the extra field, where BGZF puts it.
pub fn bgzf_member_len(header: &[u8]) -> Option<u64> {
    let header: &[u8; BGZF_HEADER_LEN] = header.get(..BGZF_HEADER_LEN)?.try_into().ok()?;
    let fextra = header[3] & 0x04 != 0;
    let xlen = u16::from_le_bytes([header[10], header[11]]);
    let subfield = header[12..16] == [b'B', b'C', 2, 0];
    if header[..3] != GZIP_MAGIC || !fextra || xlen < 6 || !subfield {
        return None;
    }
    Some(u64::from(u16::from_le_bytes([header[16], header[17]])) + 1)
}

/// A range of the file the walk needs next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// Compressed offset.
    pub offset: u64,
    /// How many bytes, at least.
    pub len: usize,
}

/// What a supply left the walk needing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalkStep {
    /// More of the file.
    Need(Request),
    /// The walk is done: the file's index.
    Done(Index),
}

/// A walk over a BGZF file's member chain.
#[derive(Debug, Clone)]
pub struct Walk {
    size: u64,
    spacing: NonZeroU64,
    /// Where the next member begins.
    pos: u64,
    plaintext: u64,
    /// Whether the member before `pos` has yet to have its `ISIZE` read.
    owed: bool,
    checkpoints: Vec<Checkpoint>,
    /// Members read so far, each a unit's or one coalesced into it.
    members: u64,
    /// Plaintext of the unit being built, so far.
    unit_plaintext: u64,
}

impl Walk {
    /// A walk over a file of `size` bytes, coalescing its members into units
    /// of at least `spacing` plaintext bytes.
    pub fn new(size: u64, spacing: NonZeroU64) -> Walk {
        Walk {
            size,
            spacing,
            pos: 0,
            plaintext: 0,
            owed: false,
            checkpoints: Vec::new(),
            members: 0,
            unit_plaintext: 0,
        }
    }

    /// The range the walk needs next.
    pub fn request(&self) -> Request {
        let start = if self.owed { self.pos - 4 } else { self.pos };
        let end = (self.pos + BGZF_HEADER_LEN as u64).min(self.size).max(self.pos);
        Request { offset: start, len: (end - start) as usize }
    }

    /// The file's bytes from `offset`, at least the [`Walk::request`]'s.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] where the chain runs past the file's size, or a
    /// supply stops short of the request — the file being shorter than the
    /// size the walk was given; [`Error::NotGzip`] where a member should
    /// begin and gzip's magic does not; [`Error::NotBgzf`] for a gzip member
    /// that is not BGZF's.
    ///
    /// # Panics
    ///
    /// If `offset` is past the request's start: the caller answering a
    /// request the walk did not make.
    pub fn supply(&mut self, offset: u64, bytes: &[u8]) -> Result<WalkStep> {
        let request = self.request();
        assert!(
            offset <= request.offset,
            "gz-seek: a supply at {offset} answers a request at {}",
            request.offset
        );
        let held = offset + bytes.len() as u64;
        if held < request.offset + request.len as u64 {
            return Err(Error::Truncated { offset: held });
        }
        if self.size == 0 {
            return Err(Error::Truncated { offset: 0 });
        }
        loop {
            if self.owed {
                let at = (self.pos - 4 - offset) as usize;
                let isize = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"));
                if isize > BGZF_MAX_PLAINTEXT {
                    let member = self.checkpoints.last().map_or(0, |c| c.bit / 8);
                    return Err(Error::NotBgzf { offset: member });
                }
                self.plaintext += u64::from(isize);
                self.unit_plaintext += u64::from(isize);
                self.owed = false;
            }
            if self.pos == self.size {
                let end = End { plaintext: self.plaintext, compressed: self.size };
                return Ok(WalkStep::Done(Index::built(
                    self.spacing,
                    std::mem::take(&mut self.checkpoints),
                    self.members,
                    Some(end),
                )));
            }
            if self.pos + BGZF_HEADER_LEN as u64 > self.size {
                return Err(Error::Truncated { offset: self.size });
            }
            let at = (self.pos - offset) as usize;
            let header = &bytes[at..at + BGZF_HEADER_LEN];
            let len = bgzf_member_len(header).ok_or(if header[..2] == GZIP_MAGIC[..2] {
                Error::NotBgzf { offset: self.pos }
            } else {
                Error::NotGzip { offset: self.pos }
            })?;
            if len < MIN_MEMBER {
                return Err(Error::NotBgzf { offset: self.pos });
            }
            if self.pos + len > self.size {
                return Err(Error::Truncated { offset: self.size });
            }
            if self.checkpoints.is_empty() || self.unit_plaintext >= self.spacing.get() {
                self.checkpoints.push(Checkpoint::member(self.plaintext, self.pos));
                self.unit_plaintext = 0;
            }
            self.pos += len;
            self.members += 1;
            self.owed = true;
            let next = self.request();
            if held < next.offset + next.len as u64 {
                return Ok(WalkStep::Need(next));
            }
        }
    }
}

#[cfg(test)]
mod tests;
