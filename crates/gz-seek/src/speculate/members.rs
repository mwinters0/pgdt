//! A BGZF chunk's guess read off its members' headers and trailers, with
//! nothing decoded.
//!
//! **A BGZF member states where the next begins and how much plaintext it
//! holds** — its header's `BSIZE`, its trailer's `ISIZE` — and begins with no
//! history, so the guess a speculation would decode a chunk to reach is read
//! off the chain instead: from the first BGZF header at or past the chunk's
//! first byte to the first member start at or past its end, the stop a decoded
//! speculation reaches there (`super`), its length the `ISIZE`s between and
//! its history at the stop none. Still a guess: the frontier takes the decode
//! it lets begin only standing at its start, every trailer it holds matching
//! (`docs/design/decisions.md`, "D117").
//!
//! **Each member is read by its own header**, as the placement rule decides
//! each by its own (`docs/design/decisions.md`, "D116"): the chain ends at a
//! member start whose header is a plain gzip member's, which states no
//! length, and is cut at the last member start it reached wherever the bytes there
//! begin no member, an `ISIZE` passes BGZF's most, a member runs past the
//! file's end, or the plaintext would pass the limit. A first header whose
//! stated length lands inside the chunk on no member start was no header, and
//! the search goes on past it; the length of the member reaching the chunk's
//! end, the first's included, is taken as stated, the bytes after it unread. A
//! chunk holding no BGZF header is searched as any file's chunk is, once the
//! scan for one has read it to its end.

use crate::bgzf::{BGZF_HEADER_LEN, BGZF_MAX_PLAINTEXT, MIN_MEMBER, bgzf_member_len};
use crate::member::GZIP_MAGIC;
use crate::native::Input;

/// What a read of the chain came to.
pub(super) enum Read {
    /// It needs the bytes from [`Input::end`] on.
    Input,
    /// The chunk's chain, or nothing where its first member alone passes the
    /// limit.
    Done(Option<Walked>),
    /// No BGZF chain begins in the chunk.
    Search,
}

/// A read of one chunk's member chain.
pub(super) struct Members {
    /// The chunk's first byte, where a search of the chunk as any file's
    /// begins.
    pub(super) from: u64,
    /// The next byte searched for a BGZF header, while no chain is begun.
    search: u64,
    chain: Option<Chain>,
}

/// A chunk's chain as read: the member start it begins at, the member start
/// it stops at — `None` at the file's end — and the plaintext between, in
/// bytes. Its history at the stop is none, a member beginning with none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Walked {
    pub(super) first: u64,
    pub(super) stop: Option<u64>,
    pub(super) plaintext: u64,
}

/// What a search for the chain's first header came to.
enum Found {
    Chain(Chain),
    /// It needs the bytes from [`Input::end`] on.
    Input,
    Nothing,
}

/// The chain from its first member to a member start reached.
#[derive(Clone, Copy)]
struct Chain {
    first: u64,
    /// The member start reached, its header a BGZF one stating `len`.
    at: u64,
    len: u64,
    /// The plaintext from the first member's start to `at`'s.
    plaintext: u64,
}

impl Members {
    pub(super) fn new(from: u64) -> Members {
        Members { from, search: from, chain: None }
    }

    /// Read on through `input`, held from the chunk's first byte, to `until`,
    /// the next chunk's first bit, holding at most `limit` bytes of
    /// plaintext.
    pub(super) fn read(&mut self, input: &Input, until: u64, limit: usize) -> Read {
        let file_len = input.file_len();
        loop {
            let Some(chain) = self.chain else {
                match self.find(input, until / 8) {
                    Found::Chain(chain) => self.chain = Some(chain),
                    Found::Input => return Read::Input,
                    Found::Nothing => return Read::Search,
                }
                continue;
            };
            let next = chain.at + chain.len;
            if next > file_len {
                match self.cut(chain) {
                    Some(read) => return read,
                    None => continue,
                }
            }
            // The member's `ISIZE`, and the header after it, inside the input.
            let wanted = if 8 * next >= until { next } else { next + BGZF_HEADER_LEN as u64 };
            if input.end() < wanted.min(file_len) {
                return Read::Input;
            }
            let trailer = input.from(next - 4);
            let isize = u32::from_le_bytes(trailer[..4].try_into().expect("four bytes"));
            let plaintext = chain.plaintext + u64::from(isize);
            if isize > BGZF_MAX_PLAINTEXT || plaintext > limit as u64 {
                if chain.at == chain.first && isize <= BGZF_MAX_PLAINTEXT {
                    return Read::Done(None);
                }
                match self.cut(chain) {
                    Some(read) => return read,
                    None => continue,
                }
            }
            let reached = Chain { at: next, plaintext, ..chain };
            if next == file_len {
                return Read::Done(Some(walked(reached, None)));
            }
            if 8 * next >= until {
                return Read::Done(Some(walked(reached, Some(next))));
            }
            let bytes = input.from(next);
            if let Some(len) = member_len(bytes) {
                self.chain = Some(Chain { len, ..reached });
            } else if bytes.starts_with(&GZIP_MAGIC) {
                // A plain member states no length: the chain ends at its start.
                return Read::Done(Some(walked(reached, Some(next))));
            } else {
                match self.cut(chain) {
                    Some(read) => return read,
                    None => continue,
                }
            }
        }
    }

    /// The first BGZF header from the search on, before byte `until`.
    fn find(&mut self, input: &Input, until: u64) -> Found {
        while self.search < until {
            let held = input.from(self.search);
            let room = usize::try_from(until - self.search).unwrap_or(usize::MAX);
            let looked = &held[..held.len().min(room)];
            let Some(skip) = looked.iter().position(|&b| b == GZIP_MAGIC[0]) else {
                self.search += looked.len() as u64;
                if self.search < until {
                    return if input.is_complete() { Found::Nothing } else { Found::Input };
                }
                continue;
            };
            self.search += skip as u64;
            let bytes = &held[skip..];
            if bytes.len() < BGZF_HEADER_LEN && !input.is_complete() {
                return Found::Input;
            }
            if let Some(len) = member_len(bytes) {
                let at = self.search;
                return Found::Chain(Chain { first: at, at, len, plaintext: 0 });
            }
            self.search += 1;
        }
        Found::Nothing
    }

    /// The chain cut at the last member start it reached: the guess to there,
    /// or, where that is its first, no chain, the search going on past it.
    fn cut(&mut self, chain: Chain) -> Option<Read> {
        if chain.at > chain.first {
            return Some(Read::Done(Some(walked(chain, Some(chain.at)))));
        }
        self.chain = None;
        self.search = chain.first + 1;
        None
    }
}

/// The length a BGZF header at the start of `bytes` states, where it is one
/// and states one a member can have.
fn member_len(bytes: &[u8]) -> Option<u64> {
    bgzf_member_len(bytes).filter(|&len| len >= MIN_MEMBER)
}

/// The chain from its first member to `stop`, the member start it reached.
fn walked(chain: Chain, stop: Option<u64>) -> Walked {
    Walked { first: chain.first, stop, plaintext: chain.plaintext }
}
