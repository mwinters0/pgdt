//! A BGZF chunk's guess read off its members' headers and trailers, with
//! nothing decoded.
//!
//! **A BGZF member states where the next begins and how much plaintext it
//! holds** — its header's `BSIZE`, its trailer's `ISIZE` — and begins with no
//! history, so the guess a speculation would decode a chunk to reach is read
//! off the chain instead: from the first BGZF header at or past the chunk's
//! first byte to the first member start at or past its end, where a decoded
//! speculation stops too unless a block inside that member ends first
//! (`super`), its length the `ISIZE`s between and
//! its history at the stop none. Still a guess: the frontier takes the decode
//! it lets begin only standing at its start, every trailer it holds matching
//! (`docs/design/decisions.md`, "D117").
//!
//! **Each member is read by its own header**, as the placement rule decides
//! each by its own (`docs/design/decisions.md`, "D116"): the chain ends at a
//! member start whose header is a plain gzip member's, which states no
//! length. **Every stated length is borne out**, the member reaching the
//! chunk's end included: one is taken only where a BGZF header, a plain
//! member's magic or the file's end follows it. The chain is cut at the last
//! member start it reached wherever the bytes after a member begin no member,
//! an `ISIZE` passes BGZF's most, or a member runs past the file's end; where
//! that start is its first, it was no header, and the search goes on past it,
//! so no guess begins at a lone header the bytes do not bear out. Plaintext
//! that would pass the limit cuts the chain there too, and so does a member
//! past the events a guess may span, three a member — its start, header and
//! end, all the engine's decode of the chunk records of it — the guess marked
//! cut for the pass to read on from (`super`). A chunk holding no BGZF header is
//! searched as any file's chunk is, once the scan for one has read it to its
//! end; a continuation looks for one only where it begins.

use crate::bgzf::{BGZF_HEADER_LEN, BGZF_MAX_PLAINTEXT, MIN_MEMBER, bgzf_member_len};
use crate::member::GZIP_MAGIC;
use crate::native::Input;

/// The events the engine's decode of a chained member records: its start,
/// its header and its end (`docs/design/decisions.md`, "D123"), a span
/// holding no other member (`KD154`).
const EVENTS_A_MEMBER: usize = 3;

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
    /// Whether a chain may begin only at `from`, a continuation's start.
    pinned: bool,
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
    /// Whether the limit cut it.
    pub(super) cut: bool,
}

/// What a search for the chain's first header came to.
enum Found {
    Chain(Chain),
    /// It needs the bytes from [`Input::end`] on.
    Input,
    Nothing,
}

/// What follows a member, bearing its stated length out.
enum Follows {
    /// A BGZF member, stating this length.
    Bgzf(u64),
    /// A plain gzip member, stating none.
    Plain,
    /// The file's end.
    End,
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
    /// The members from the first to `at`.
    members: usize,
}

impl Members {
    pub(super) fn new(from: u64) -> Members {
        Members { from, search: from, pinned: false, chain: None }
    }

    /// A read of the chain beginning at byte `at` and nowhere else: a
    /// continuation's, which searches its chunk as any file's from `at`
    /// where no BGZF header begins there.
    pub(super) fn at(at: u64) -> Members {
        Members { pinned: true, ..Members::new(at) }
    }

    /// Read on through `input`, held from the chunk's first byte, to `until`,
    /// the next chunk's first bit, holding at most `limit` bytes of
    /// plaintext and spanning at most `events` events.
    pub(super) fn read(&mut self, input: &Input, until: u64, limit: usize, events: usize) -> Read {
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
            if input.end() < (next + BGZF_HEADER_LEN as u64).min(file_len) {
                return Read::Input;
            }
            // Its stated length is borne out only where a member, or the
            // file's end, follows it.
            let follows = if next == file_len {
                Follows::End
            } else {
                let bytes = input.from(next);
                match member_len(bytes) {
                    Some(len) => Follows::Bgzf(len),
                    None if bytes.starts_with(&GZIP_MAGIC) => Follows::Plain,
                    None => match self.cut(chain) {
                        Some(read) => return read,
                        None => continue,
                    },
                }
            };
            let trailer = input.from(next - 4);
            let isize = u32::from_le_bytes(trailer[..4].try_into().expect("four bytes"));
            let (plaintext, members) = (chain.plaintext + u64::from(isize), chain.members + 1);
            if isize > BGZF_MAX_PLAINTEXT {
                match self.cut(chain) {
                    Some(read) => return read,
                    None => continue,
                }
            }
            if plaintext > limit as u64 || EVENTS_A_MEMBER * members > events {
                if chain.at == chain.first {
                    return Read::Done(None);
                }
                return Read::Done(Some(Walked { cut: true, ..walked(chain, Some(chain.at)) }));
            }
            let reached = Chain { at: next, plaintext, members, ..chain };
            match follows {
                Follows::End => return Read::Done(Some(walked(reached, None))),
                Follows::Bgzf(len) if 8 * next < until => {
                    self.chain = Some(Chain { len, ..reached });
                }
                // A plain member states no length: the chain ends at its
                // start, as it does at the first member start past the chunk.
                Follows::Bgzf(_) | Follows::Plain => {
                    return Read::Done(Some(walked(reached, Some(next))));
                }
            }
        }
    }

    /// The first BGZF header from the search on, before byte `until`, or
    /// only at `from` where the read is pinned there.
    fn find(&mut self, input: &Input, until: u64) -> Found {
        let until = if self.pinned { until.min(self.from + 1) } else { until };
        while self.search < until {
            let held = input.from(self.search);
            let room = usize::try_from(until - self.search).unwrap_or(usize::MAX);
            let looked = &held[..held.len().min(room)];
            let Some(skip) = position_of(looked, GZIP_MAGIC[0]) else {
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
                return Found::Chain(Chain { first: at, at, len, plaintext: 0, members: 0 });
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

/// Where `byte` first is in `bytes`, looked for a chunk at a time, so the
/// compiler compares a chunk's bytes at once: a chunk of compressed bytes
/// holding a gzip magic only about once in its 256 bytes.
pub(super) fn position_of(bytes: &[u8], byte: u8) -> Option<usize> {
    const CHUNK: usize = 32;
    let (chunks, rest) = bytes.as_chunks::<CHUNK>();
    for (i, chunk) in chunks.iter().enumerate() {
        if chunk.iter().fold(false, |any, &b| any | (b == byte)) {
            return chunk.iter().position(|&b| b == byte).map(|j| i * CHUNK + j);
        }
    }
    rest.iter().position(|&b| b == byte).map(|j| bytes.len() - rest.len() + j)
}

/// The length a BGZF header at the start of `bytes` states, where it is one
/// and states one a member can have.
fn member_len(bytes: &[u8]) -> Option<u64> {
    bgzf_member_len(bytes).filter(|&len| len >= MIN_MEMBER)
}

/// The chain from its first member to `stop`, the member start it reached.
fn walked(chain: Chain, stop: Option<u64>) -> Walked {
    Walked { first: chain.first, stop, plaintext: chain.plaintext, cut: false }
}
