//! The backward footer walk: an `.xz` file's own indexes, turned into a
//! [`SeekTable`].
//!
//! Nothing here decodes anything. The walk reads each stream's footer, the
//! index it points back at, and the stream header it derives the position of,
//! and that is enough to place every block in the file.
//!
//! # The parse is a machine; the fetching is the driver's
//!
//! [`Walk`] is the parser. It hands out a [`Request`] naming a range of the
//! compressed file, takes those bytes back through [`Walk::supply`], and hands
//! out the next request until it has a table. It reads nothing, borrows
//! nothing and has no lifetime, so a caller whose transport is asynchronous
//! drives it across an `await` without this crate acquiring a runtime.
//!
//! [`walk`] is the synchronous driver, and [`Backfill`] is the cache above the
//! machine. That is where the read count lives: the machine emits minimal
//! ranges, and the straddling refill below is what turns the four a stream
//! costs into one read. `docs/design/decisions.md`, "D64".
//!
//! # The file's end is the machine's, not each driver's
//!
//! The size is a constructor argument, so the machine is the one layer that
//! holds it: it never hands out a range past it, and a supply that stops short
//! of the range it answers is the file being smaller than that number, which is
//! [`Error::Truncated`] at the offset the bytes ran out. A driver therefore
//! carries no bound of its own — [`Backfill`] holds no size at all — and the
//! panic is left to the two faults that name no offset in a file: a supply
//! beginning past the start of what was asked for, and a supply of a request
//! this walk is not waiting for.
//! `docs/design/decisions.md`, "D65".
//!
//! # One read per stream, because of where a header sits
//!
//! A stream's header is adjacent to its *predecessor's* padding, footer and
//! index:
//!
//! ```text
//! …[stream N-1 index][stream N-1 footer][padding][stream N header]…
//! ```
//!
//! So [`Backfill`] refills as a fixed 4 KiB block **ending at the end of the range
//! asked for**. Walking backward, that one policy straddles the boundary for
//! free: the read that fetches stream N's 12-byte header also carries the 4 KiB
//! below it, which is exactly stream N-1's tail. The walk is therefore one read
//! per stream plus one for the file's own tail, against the naive two per
//! stream — 31,151 reads rather than 62,301 on the real multistream corpus
//! file, beside the magic read at offset 0 that precedes all of them.
//! An index larger than the window falls back to a read sized to the index,
//! which is the same policy with a bigger `len`.
//!
//! # The first block's header rides on the stream header's read
//!
//! [`StreamEntry::first_block_dict_size`] is what a caller charges one concurrent
//! decode for, and it is one byte of the first block's header — which begins
//! exactly where the stream header ends. The machine asks for both in one
//! request, and the refill grows forward to cover the lookahead rather than
//! giving up backward reach for it, so the block header costs **no additional
//! read**. What it costs is bytes, at most [`crate::block::HEADER_SIZE_MAX`] of
//! them per stream.
//!
//! *Rejected: buying the lookahead out of the backward reach* — see
//! `docs/design/decisions.md`, "D6", and [`Request::lookahead`].
//!
//! # What a failure is called
//!
//! Three variants come out of the parse, and the split is R7's. ([`Error::Io`]
//! comes out of this module too, but out of the synchronous driver below the
//! machine — [`Backfill`] and [`walk`] are the only things here that touch a
//! source at all.)
//!
//! * [`Error::NotXz`] — the six magic bytes at offset 0 are wrong. Read once,
//!   before anything else, because it is the only thing separating a random
//!   file from a clipped download: both fail at the tail.
//! * [`Error::Truncated`] — the file claims to be `.xz` and the structure runs
//!   off its end. That includes a size that is not a multiple of four, a range
//!   that would pass the last byte, a supply that stops short of the range it
//!   answers, and **a tail with no footer magic where a footer must be**, which
//!   is what a `head -c` cut leaves behind.
//! * [`Error::IndexInconsistent`] — the structure is present and disagrees with
//!   itself: a footer or index or header CRC32 that does not match, reserved
//!   stream-flag bits, a header that is not where the index says the stream
//!   starts, header flags that differ from the footer's copy. The format calls
//!   these separate errors; the taxonomy deliberately does not, because a
//!   caller acts on all of them identically — the file is damaged in its index
//!   region — and the offset each carries says which one it was.
//!
//! `Error::NotXz` is decided *only* by the magic. Once past it, a walk failure
//! is one of the other two, so a damaged file is never reported as "not xz".
//!
//! *Rejected: drawing the `Truncated`/`IndexInconsistent` line at the footer's
//! CRC32 rather than at its magic* — `docs/design/decisions.md`, "D7". What
//! settles it is that `liblzma` partitions the same failure the same way: the
//! magic asks whether the structure is here, the CRC32 whether what is here is
//! damaged, and a second backend goes behind this taxonomy. See
//! `xz-invariants.md`, `I11` — which also records that this line can never be
//! settled differentially, since `xz --list` prints one string for every
//! damaged fixture in the corpus.
//!
//! # Only the tail of a file can be cut
//!
//! That line decides which *kind* of damage a failure is; where the walk found
//! it decides whether "damage" is the right word at all. A truncation removes a
//! suffix, so the two failures that mean "no stream ends here" — an absent
//! footer magic, and a boundary with less than a minimum stream's worth of file
//! beneath it — are a truncation **only at the tail-most stream**.
//!
//! The machine's first stream starts at the file's own end and is the one that
//! can meet a cut. Every later one starts at a boundary the walk *derived* —
//! from a footer it read, an index whose CRC32 matched, and a header found
//! where that index said one would be — so every byte from there to the end of
//! the file is known to be present, and the same two failures are the container
//! disagreeing with itself: [`Error::IndexInconsistent`].
//!
//! Without that condition a file whose *interior* is damaged is reported as
//! "ends at offset N" for an N that is nowhere near its end, which sends a
//! caller to re-fetch a file it already has whole.

use std::ops::Range;

use crate::error::{Error, Result};
use crate::source::CompressedSource;
use crate::table::{BlockEntry, Check, SeekTable, StreamEntry};

/// The straddling window, in bytes. Not configurable: see the module docs.
const WINDOW: u64 = 4096;

pub(crate) const STREAM_HEADER_SIZE: u64 = 12;
pub(crate) const STREAM_FOOTER_SIZE: u64 = 12;
/// Indicator, a one-byte record count, two bytes of padding and a CRC32.
pub(crate) const INDEX_SIZE_MIN: u64 = 8;
/// The smallest possible stream: a header, an empty index and a footer.
pub(crate) const STREAM_SIZE_MIN: u64 = STREAM_HEADER_SIZE + INDEX_SIZE_MIN + STREAM_FOOTER_SIZE;

const HEADER_MAGIC: [u8; 6] = [0xfd, b'7', b'z', b'X', b'Z', 0x00];
const FOOTER_MAGIC: [u8; 2] = *b"YZ";

/// `LZMA_VLI_BYTES_MAX`: seven bits a byte over 63 bits.
const VLI_BYTES_MAX: usize = 9;
/// The format's stated floor — a block is at least a header and a byte.
pub(crate) const UNPADDED_SIZE_MIN: u64 = 5;

/// One range of the compressed file the walk needs before it can go on.
///
/// Handed out by [`Walk::begin`] and by [`Walk::supply`], and handed back with
/// the bytes it names. It is `Copy`, holds no borrow and has no lifetime, so it
/// crosses an `await` as a value — which is what makes the machine drivable
/// from a future. There is no public constructor: a request is one the machine
/// itself produced, and that is what lets [`Walk::supply`] tell a stale request
/// from the pending one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request {
    start: u64,
    len: u64,
    /// How many of the trailing bytes are a forward lookahead — nonzero only
    /// for the stream-header request, whose tail is the first block's header.
    ///
    /// **Crate-private, and a fetch hint rather than part of the protocol.**
    /// The structure the machine is really at is `[start, end - lookahead)`, so
    /// a cache reaching backward from a request positions itself off that; the
    /// synchronous driver's does, which is what keeps its read exactly the
    /// window plus the lookahead. The public surface is the range alone —
    /// `docs/design/decisions.md`, "D64".
    lookahead: u64,
}

impl Request {
    /// The file-absolute range of bytes asked for.
    pub fn range(&self) -> Range<u64> {
        self.start..self.start + self.len
    }

    /// File-absolute offset of the first byte asked for.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// How many bytes are asked for.
    ///
    /// Zero only for a file too short to hold the six magic bytes, which is a
    /// file the first [`Walk::supply`] refuses as [`Error::NotXz`].
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Whether the request asks for no bytes at all.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// One past the last byte asked for.
    fn end(&self) -> u64 {
        self.start + self.len
    }
}

/// What the machine wants after a supply: more bytes, or nothing more.
#[derive(Debug)]
pub enum Step {
    /// Fetch this range and supply it.
    Need(Request),
    /// The walk is over, and this is the file's table.
    Done(SeekTable),
}

/// The footer walk as a state machine the caller drives.
///
/// The caller fetches; this parses. Nothing here reads a byte, opens anything
/// or knows what a transport is — which is the whole point, since a caller
/// cannot `await` inside [`CompressedSource::read_at`].
///
/// ```no_run
/// # fn fetch(_range: std::ops::Range<u64>) -> Vec<u8> { unimplemented!() }
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use xz_seek::{Step, Walk};
///
/// # let file_size = 0u64;
/// let (mut walk, mut request) = Walk::begin(file_size);
/// let table = loop {
///     let bytes = fetch(request.range());
///     match walk.supply(request, &bytes)? {
///         Step::Need(next) => request = next,
///         Step::Done(table) => break table,
///     }
/// };
/// # let _ = table;
/// # Ok(())
/// # }
/// ```
///
/// The file's size is a constructor argument rather than a request: the walk
/// starts at the tail, and reaching for that number through an I/O trait is
/// what this seam exists to remove.
///
/// The requests descend, each one's position computed from the bytes of the
/// one before it, and a whole walk is the magic, then per stream a footer, an
/// index, a header, and one padding probe per four bytes of stream padding
/// **plus one** — the probe that finds a non-zero word is what ends the run,
/// so a stream with no padding still costs one. [`Reader`](crate::Reader) and [`SeekTable::from_source`] are drivers
/// over this, and the cost the file itself dictates is unchanged by which
/// driver is used.
#[derive(Debug)]
pub struct Walk {
    size: u64,
    /// The request the caller was last handed, and which `supply` takes back.
    /// `None` once the walk has finished or failed.
    pending: Option<Request>,
    state: State,
    walked: Vec<Walked>,
    /// The end of the stream being walked, its own padding included.
    end: u64,
    /// Whether that end is the file's own: see the module docs, "Only the tail
    /// of a file can be cut".
    tail_most: bool,
}

/// What the pending request is for, and what the parse carries across it.
#[derive(Debug)]
enum State {
    Magic,
    Padding {
        padding: u64,
    },
    Footer {
        footer_end: u64,
        padding: u64,
    },
    Index {
        index_start: u64,
        check: Check,
        flags: [u8; 2],
        footer_end: u64,
        padding: u64,
    },
    Header {
        start: u64,
        check: Check,
        flags: [u8; 2],
        footer_end: u64,
        padding: u64,
        records: Vec<(u64, u64)>,
    },
    /// Finished or failed: `pending` is `None`, so no supply reaches this.
    Done,
}

impl Walk {
    /// Start a walk over a file of `file_size` bytes, and ask for the first
    /// range.
    ///
    /// The six magic bytes at offset 0 come first because they are the only
    /// thing that decides [`Error::NotXz`]; a file with no room for them fails
    /// there rather than at its tail.
    pub fn begin(file_size: u64) -> (Walk, Request) {
        let request = Request {
            start: 0,
            len: (HEADER_MAGIC.len() as u64).min(file_size),
            lookahead: 0,
        };
        let walk = Walk {
            size: file_size,
            pending: Some(request),
            state: State::Magic,
            walked: Vec::new(),
            end: file_size,
            tail_most: true,
        };
        (walk, request)
    }

    /// Supply bytes beginning at the start of `request`.
    ///
    /// [`Walk::supply_at`] with `at` defaulted to the request's own start, which
    /// is the whole difference between the two: containment is the one rule, so
    /// `bytes` running past the request's end is accepted and the remainder
    /// ignored, exactly as it is there.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if `bytes` is shorter than the request, and whatever
    /// the parse finds in the bytes it did get.
    ///
    /// # Panics
    ///
    /// If `request` is not the one this machine is waiting for — including one
    /// held from an earlier step, and one supplied after the walk ended. A
    /// driver-protocol fault is a caller contradicting itself one line after
    /// being told what to fetch, and it names no offset in a file, so it is not
    /// one of the taxonomy's variants: `docs/design/decisions.md`, "D64".
    ///
    /// In a debug build, also if this machine derives a range past the size
    /// `begin` was given, which no file reaches and which a release build
    /// refuses as [`Error::Truncated`] instead — `docs/design/decisions.md`,
    /// "D65".
    pub fn supply(&mut self, request: Request, bytes: &[u8]) -> Result<Step> {
        self.supply_at(request, request.start, bytes)
    }

    /// Supply bytes that begin at `at` and **contain** the requested range.
    ///
    /// This is the shape for a caller who fetched more than was asked for — a
    /// coalesced read, a speculative window, a whole file already in memory —
    /// and it is what the synchronous driver itself uses, handing the machine
    /// its whole cache window. Bytes outside the request are not retained: a
    /// cache here would be the fetch policy this seam hands out.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the supplied bytes stop short of the request's
    /// end, and whatever the parse finds in the bytes it did get.
    ///
    /// # Panics
    ///
    /// As [`Walk::supply`], and if `at` is past the request's own start. Only
    /// that half of containment is a driver contradicting itself; running out
    /// early is the file's own length, and the machine is what holds it —
    /// `docs/design/decisions.md`, "D65".
    pub fn supply_at(&mut self, request: Request, at: u64, bytes: &[u8]) -> Result<Step> {
        let requested = self.accept(request, at, bytes)?;
        self.step(requested)
    }

    /// Take the pending request back, and cut the bytes it asked for out of
    /// what the caller supplied.
    ///
    /// The two halves of containment are not the same fault. Bytes beginning
    /// *past* the request's start are a driver that sliced wrongly, which names
    /// no offset in a file. Bytes that begin at or below it and run out early
    /// are the file being smaller than the size `begin` was handed, which names
    /// one — and a driver told to fetch a range cannot distinguish the two from
    /// outside, which is why the machine does it.
    fn accept<'b>(&mut self, request: Request, at: u64, bytes: &'b [u8]) -> Result<&'b [u8]> {
        assert_eq!(
            self.pending,
            Some(request),
            "supplied a request this walk is not waiting for"
        );
        assert!(
            at <= request.start,
            "supplied bytes beginning at {at}, past the start of the requested {:?}",
            request.range()
        );
        self.pending = None;
        let end = at.saturating_add(bytes.len() as u64);
        if end < request.end() {
            return Err(Error::Truncated {
                compressed_offset: end,
            });
        }
        let lo = (request.start - at) as usize;
        Ok(&bytes[lo..lo + request.len as usize])
    }

    /// Parse what the pending request asked for, and decide the next one.
    fn step(&mut self, bytes: &[u8]) -> Result<Step> {
        match std::mem::replace(&mut self.state, State::Done) {
            State::Magic => {
                if bytes != HEADER_MAGIC {
                    return Err(Error::NotXz {
                        compressed_offset: 0,
                    });
                }
                // Streams and stream padding are both multiples of four, so the
                // file is.
                if !self.size.is_multiple_of(4) {
                    return Err(Error::Truncated {
                        compressed_offset: self.size,
                    });
                }
                self.stream()
            }
            State::Padding { padding } => {
                // Stream padding: null bytes, a multiple of four. A footer's
                // last four bytes are its flags and `YZ`, which are never all
                // zero, so this cannot walk into the stream it is looking for.
                if bytes != [0, 0, 0, 0] {
                    return self.footer(padding);
                }
                let padding = padding + 4;
                if self.end - padding >= 4 {
                    self.probe(padding)
                } else {
                    self.footer(padding)
                }
            }
            State::Footer {
                footer_end,
                padding,
            } => {
                let footer_start = footer_end - STREAM_FOOTER_SIZE;
                let f = bytes;
                // No footer magic here means there is no footer here: at the
                // tail of a file that claims to be xz, that is a cut; anywhere
                // else it is the index region contradicting itself.
                if f[10..12] != FOOTER_MAGIC {
                    return missing_end(self.tail_most, footer_start);
                }
                if crate::check::crc32(&f[4..10]) != le32(&f[0..4]) {
                    return inconsistent(footer_start);
                }
                let flags = [f[8], f[9]];
                let check = stream_flags(flags, footer_start + 8)?;
                let index_size = (le32(&f[4..8]) as u64 + 1) * 4;
                let Some(index_start) = footer_start.checked_sub(index_size) else {
                    return inconsistent(footer_start + 4);
                };
                if index_size < INDEX_SIZE_MIN {
                    return inconsistent(index_start);
                }
                self.state = State::Index {
                    index_start,
                    check,
                    flags,
                    footer_end,
                    padding,
                };
                self.need(Request {
                    start: index_start,
                    len: index_size,
                    lookahead: 0,
                })
            }
            State::Index {
                index_start,
                check,
                flags,
                footer_end,
                padding,
            } => {
                let records = index(bytes, index_start)?;

                let mut blocks_total = 0u64;
                for (unpadded, _) in &records {
                    blocks_total = match blocks_total.checked_add(round4(*unpadded)) {
                        Some(t) => t,
                        None => return inconsistent(index_start),
                    };
                }
                // A crafted index can claim blocks summing past the address
                // space, so the span is checked as well as subtracted:
                // `index-span-overflow.xz` is the file that does.
                let Some(start) = blocks_total
                    .checked_add(STREAM_HEADER_SIZE)
                    .and_then(|span| index_start.checked_sub(span))
                else {
                    return inconsistent(index_start);
                };

                // The header costs no extra read: the refill ends where this
                // range ends, so it carries the previous stream's tail with it.
                // The same range reaches forward over the first block's header,
                // which begins where the stream header ends.
                let lookahead = first_block_header_extent(&records);
                self.state = State::Header {
                    start,
                    check,
                    flags,
                    footer_end,
                    padding,
                    records,
                };
                self.need(Request {
                    start,
                    len: STREAM_HEADER_SIZE + lookahead,
                    lookahead,
                })
            }
            State::Header {
                start,
                check,
                flags,
                footer_end,
                padding,
                records,
            } => {
                let (h, first_block_header) = bytes.split_at(STREAM_HEADER_SIZE as usize);
                if h[0..6] != HEADER_MAGIC {
                    return inconsistent(start);
                }
                if crate::check::crc32(&h[6..8]) != le32(&h[8..12]) {
                    return inconsistent(start + 8);
                }
                // The format requires the two copies of the flags to be
                // identical.
                if h[6..8] != flags {
                    return inconsistent(start + 6);
                }

                self.walked.push(Walked {
                    start,
                    end: footer_end,
                    check,
                    padding,
                    first_block_dict_size: first_block_dict_size(
                        &records,
                        start,
                        check,
                        first_block_header,
                    ),
                    records,
                });
                if start == 0 {
                    let walked = std::mem::take(&mut self.walked);
                    return Ok(Step::Done(assemble(self.size, walked)?));
                }
                // Only the first stream starts at the file's own end, so only
                // it can meet a cut rather than a contradiction.
                self.end = start;
                self.tail_most = false;
                self.stream()
            }
            State::Done => unreachable!("a finished walk has no pending request"),
        }
    }

    /// Begin the stream whose padding ends at `self.end`.
    fn stream(&mut self) -> Result<Step> {
        if self.end >= 4 {
            self.probe(0)
        } else {
            self.footer(0)
        }
    }

    /// Probe the four bytes below `self.end - padding` for more stream padding.
    fn probe(&mut self, padding: u64) -> Result<Step> {
        self.state = State::Padding { padding };
        self.need(Request {
            start: self.end - padding - 4,
            len: 4,
            lookahead: 0,
        })
    }

    /// The padding is behind us: ask for the footer it ends above.
    fn footer(&mut self, padding: u64) -> Result<Step> {
        let footer_end = self.end - padding;
        if footer_end < STREAM_SIZE_MIN {
            return missing_end(self.tail_most, footer_end);
        }
        self.state = State::Footer {
            footer_end,
            padding,
        };
        self.need(Request {
            start: footer_end - STREAM_FOOTER_SIZE,
            len: STREAM_FOOTER_SIZE,
            lookahead: 0,
        })
    }

    /// Hand `request` out, unless it runs past the file.
    ///
    /// Every range the machine emits passes here, so the bound against the size
    /// is written once. It sits here rather than in each driver because the size
    /// is the machine's field: a driver checking it would be re-deriving, four
    /// times over, a limit the layer holding the number can state.
    ///
    /// **No file reaches it.** Every range descends from a boundary the parse
    /// has already placed inside the file, and the one that reaches *forward* is
    /// bounded by [`first_block_header_extent`], so this is the machine keeping
    /// a promise to its drivers rather than a path damage can take. Firing it is
    /// therefore this crate's own arithmetic, which is why the assertion names
    /// that and the release build still refuses the range:
    /// `docs/design/decisions.md`, "D65".
    fn need(&mut self, request: Request) -> Result<Step> {
        debug_assert!(
            request.end() <= self.size,
            "xz-seek asked for {:?}, past the {} bytes this walk was given: no \
             file reaches this, so it is a mis-derivation in xz-seek rather than \
             a short file, and worth reporting",
            request.range(),
            self.size
        );
        if request.end() > self.size {
            return Err(Error::Truncated {
                compressed_offset: self.size,
            });
        }
        self.pending = Some(request);
        Ok(Step::Need(request))
    }
}

/// Round up to the next multiple of four: block padding, index padding.
fn round4(v: u64) -> u64 {
    (v + 3) & !3
}

/// Shared with [`crate::block`]: the block header carries a CRC32 too.
pub(crate) fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Shared with [`crate::block`], which reaches the same verdict for the same
/// reason: the structure is present and disagrees with itself.
pub(crate) fn inconsistent<T>(compressed_offset: u64) -> Result<T> {
    Err(Error::IndexInconsistent { compressed_offset })
}

/// A structure that is missing where a stream must end: a cut at the tail-most
/// stream, and a self-contradiction anywhere else. See the module docs, "Only
/// the tail of a file can be cut".
fn missing_end<T>(tail_most: bool, compressed_offset: u64) -> Result<T> {
    if tail_most {
        Err(Error::Truncated { compressed_offset })
    } else {
        inconsistent(compressed_offset)
    }
}

/// A 4 KiB read, cached, that refills backward: the synchronous driver's cache.
///
/// The refill positions the window to **end** where the requested range ends.
/// That is the whole of the straddle: a backward walk asking for the 12 bytes at
/// a stream's start gets the 4 KiB below it in the same read, and that is where
/// the previous stream's index and footer live. A request larger than the
/// window — an index of more than a few hundred blocks — is one read sized to
/// it, which is the same policy with a bigger `len`.
///
/// [`Request::lookahead`] buys a *forward* lookahead on top of that — the
/// stream header's read is what also fetches the first block's header — and it
/// is bought with bytes rather than with backward reach: the window still
/// begins where a request without one would have put it, and grows by the
/// lookahead at its far end.
struct Backfill<'s, S: CompressedSource> {
    source: &'s S,
    start: u64,
    buf: Vec<u8>,
}

impl<'s, S: CompressedSource> Backfill<'s, S> {
    fn new(source: &'s S) -> Self {
        Backfill {
            source,
            start: 0,
            buf: Vec::new(),
        }
    }

    /// The window holding `request`, and the offset it starts at.
    ///
    /// The whole window goes back to the machine, which takes it as the
    /// superset it is. **The cache holds no size and bounds nothing against
    /// one**: the machine does not ask for a range past the file, and a source
    /// returning fewer bytes than its own `size()` promised is handed back
    /// short, for the machine to name — `docs/design/decisions.md`, "D65".
    fn window(&mut self, request: Request) -> Result<(u64, &[u8])> {
        let end = request.end();
        if request.start < self.start || end > self.start + self.buf.len() as u64 {
            self.fill(request)?;
        }
        Ok((self.start, &self.buf))
    }

    fn fill(&mut self, request: Request) -> Result<()> {
        let end = request.end();
        let want = request.len.max(WINDOW + request.lookahead);
        let start = end.saturating_sub(want);
        let len = (end - start) as usize;
        self.buf.clear();
        self.buf.resize(len, 0);
        let got = self
            .source
            .read_at(start, &mut self.buf)
            .map_err(|e| Error::io(start, e))?;
        // A source shorter than its own `size()` said yields a short window
        // rather than an error here: the machine names where the bytes ran out.
        self.buf.truncate(got);
        self.start = start;
        Ok(())
    }
}

/// One stream as the backward walk finds it, before the file is put in order.
#[derive(Debug)]
struct Walked {
    start: u64,
    /// One past the footer's last byte — the stream's end, padding excluded.
    end: u64,
    check: Check,
    padding: u64,
    /// What this stream's first block header declares, or `None` where it did
    /// not parse: see [`StreamEntry::first_block_dict_size`].
    first_block_dict_size: Option<u64>,
    /// `(unpadded_size, uncompressed_size)` per block, in stream order.
    records: Vec<(u64, u64)>,
}

/// Build a [`SeekTable`] by walking `source`'s stream footers backward.
///
/// The synchronous driver: [`Backfill`] answers what [`Walk`] asks for, and the
/// source's own `size()` is the one number the machine is handed rather than
/// asking for.
pub(crate) fn walk<S: CompressedSource>(source: &S) -> Result<SeekTable> {
    let size = source.size().map_err(|e| Error::io(0, e))?;
    let mut cache = Backfill::new(source);
    let (mut machine, mut request) = Walk::begin(size);
    loop {
        let (at, bytes) = cache.window(request)?;
        match machine.supply_at(request, at, bytes)? {
            Step::Need(next) => request = next,
            Step::Done(table) => return Ok(table),
        }
    }
}

/// How far past a stream header the first block's header can possibly run.
///
/// A block header is `(first byte + 1) * 4` bytes and never more than
/// [`crate::block::HEADER_SIZE_MAX`], and it cannot be longer than the block the
/// index sized. Every byte of it therefore lies inside the stream, which is what
/// makes the extended request safe: `start + 12 + extent` is at most the
/// stream's own index offset.
fn first_block_header_extent(records: &[(u64, u64)]) -> u64 {
    match records.first() {
        Some((unpadded, _)) => (*unpadded).min(crate::block::HEADER_SIZE_MAX),
        None => 0,
    }
}

/// The dictionary the first block declares, from bytes the header request
/// carried.
///
/// **A header that does not parse is `None` rather than a failed walk.** The
/// walk places blocks out of the *index*, and a damaged block header is not a
/// fault in the index region: refusing the file here would make a stream's first
/// damaged header cost a caller every other block in the file, where today it
/// costs that block alone — `crate::decode` re-reads and re-parses the same
/// header, so the fault is raised, with its offset, at the read that meets it.
///
/// **What it may not do is spell that absence as zero.** Zero is what a chain
/// naming no LZMA2 filter declares, and a stream with no blocks has no header to
/// read and a dictionary known to be zero all the same; both are `Some(0)`. The
/// field is the per-stream proxy for every block of the stream, so a stream
/// whose first header is damaged and whose second block declares a large
/// dictionary is one the reader really does allocate for — `corrupt-block-header.xz`
/// is that file — and charging it zero would understate it with nothing in the
/// table able to say so.
fn first_block_dict_size(
    records: &[(u64, u64)],
    start: u64,
    check: Check,
    header: &[u8],
) -> Option<u64> {
    let Some(&(unpadded_size, uncompressed_size)) = records.first() else {
        return Some(0);
    };
    // `uncompressed_offset` is the one field the parse does not read; the walk
    // does not know the stream's uncompressed base until `assemble` runs.
    let entry = BlockEntry {
        compressed_offset: start + STREAM_HEADER_SIZE,
        uncompressed_offset: 0,
        unpadded_size,
        uncompressed_size,
    };
    crate::block::parse(header, &entry, check)
        .ok()
        .map(|h| h.dict_size)
}

/// The two stream-flag bytes, which the header and the footer both carry.
///
/// The first byte is reserved and must be zero; the second's high nibble is
/// reserved too. The low nibble is the check id, and an id nothing implements is
/// recorded rather than refused — see [`Check::Unsupported`].
fn stream_flags(flags: [u8; 2], at: u64) -> Result<Check> {
    if flags[0] != 0 || flags[1] & 0xf0 != 0 {
        return inconsistent(at);
    }
    Ok(Check::from_id(flags[1]))
}

/// The stream index: an indicator, a record count, the records, padding, CRC32.
///
/// Parsed whole, out of the bytes the index request asked for. A cursor
/// stepping forward through a window that refills backward was the alternative,
/// and it would refill on nearly every record.
fn index(idx: &[u8], start: u64) -> Result<Vec<(u64, u64)>> {
    let crc_at = idx.len() - 4;
    if crate::check::crc32(&idx[..crc_at]) != le32(&idx[crc_at..]) {
        return inconsistent(start);
    }
    if idx[0] != 0x00 {
        return inconsistent(start);
    }

    let mut p = 1;
    let count = vli(&idx[..crc_at], &mut p, start)?;
    let mut records = Vec::new();
    for _ in 0..count {
        let unpadded = vli(&idx[..crc_at], &mut p, start)?;
        let uncompressed = vli(&idx[..crc_at], &mut p, start)?;
        if unpadded < UNPADDED_SIZE_MIN {
            return inconsistent(start + p as u64);
        }
        records.push((unpadded, uncompressed));
    }

    // Index padding: 0-3 null bytes, which is what makes the whole index a
    // multiple of four.
    let pad = &idx[p..crc_at];
    if pad.len() > 3 || pad.iter().any(|b| *b != 0) {
        return inconsistent(start + p as u64);
    }
    Ok(records)
}

/// One variable-length integer: seven bits a byte, little end first.
///
/// A terminating `0x00` anywhere but the first byte is a non-minimal encoding,
/// which the format forbids — `vli_decoder.c` returns `LZMA_DATA_ERROR` for it,
/// and accepting it here would make two byte strings mean one index.
///
/// Shared with [`crate::block`]: a block header's declared sizes, filter ids and
/// property lengths are the same encoding.
pub(crate) fn vli(buf: &[u8], p: &mut usize, at: u64) -> Result<u64> {
    let mut value = 0u64;
    for i in 0..VLI_BYTES_MAX {
        let Some(byte) = buf.get(*p).copied() else {
            return inconsistent(at + *p as u64);
        };
        *p += 1;
        value |= ((byte & 0x7f) as u64) << (i * 7);
        if byte & 0x80 == 0 {
            if byte == 0x00 && i > 0 {
                return inconsistent(at + *p as u64);
            }
            return Ok(value);
        }
    }
    inconsistent(at + *p as u64)
}

/// Put the backward walk's streams in file order and place every block.
fn assemble(size: u64, walked: Vec<Walked>) -> Result<SeekTable> {
    let mut streams = Vec::with_capacity(walked.len());
    let mut blocks = Vec::new();
    let mut uncompressed_offset = 0u64;

    for s in walked.into_iter().rev() {
        let first_block = blocks.len();
        let mut c = s.start + STREAM_HEADER_SIZE;
        let mut u = uncompressed_offset;
        for (unpadded, uncompressed) in &s.records {
            blocks.push(BlockEntry {
                compressed_offset: c,
                uncompressed_offset: u,
                unpadded_size: *unpadded,
                uncompressed_size: *uncompressed,
            });
            c += round4(*unpadded);
            u = match u.checked_add(*uncompressed) {
                Some(u) => u,
                None => return inconsistent(s.start),
            };
        }
        streams.push(StreamEntry {
            compressed_offset: s.start,
            uncompressed_offset,
            compressed_size: s.end - s.start,
            uncompressed_size: u - uncompressed_offset,
            check: s.check,
            padding: s.padding,
            first_block,
            block_count: s.records.len(),
            first_block_dict_size: s.first_block_dict_size,
        });
        uncompressed_offset = u;
    }

    Ok(SeekTable {
        compressed_file_size: size,
        streams,
        blocks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A plain request for `len` bytes at `offset`, as the machine emits one.
    fn req(start: u64, len: u64) -> Request {
        Request {
            start,
            len,
            lookahead: 0,
        }
    }

    /// A source that counts the reads made through it, so that the window's
    /// caching is a property a test can state rather than an intention.
    struct Counting {
        bytes: Vec<u8>,
        reads: Cell<usize>,
        last_len: Cell<usize>,
    }

    impl Counting {
        fn new(len: usize) -> Counting {
            Counting {
                bytes: (0..len).map(|i| (i % 251) as u8).collect(),
                reads: Cell::new(0),
                last_len: Cell::new(0),
            }
        }
    }

    impl CompressedSource for Counting {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
            self.reads.set(self.reads.get() + 1);
            self.last_len.set(buf.len());
            let slice: &[u8] = &self.bytes;
            slice.read_at(offset, buf)
        }

        fn size(&self) -> std::io::Result<u64> {
            Ok(self.bytes.len() as u64)
        }
    }

    /// The bytes of `request` out of the window the driver would fetch for it.
    fn bytes<'w, S: CompressedSource>(
        w: &'w mut Backfill<'_, S>,
        request: Request,
    ) -> Result<&'w [u8]> {
        let (at, window) = w.window(request)?;
        let lo = (request.start - at) as usize;
        Ok(&window[lo..lo + request.len as usize])
    }

    #[test]
    fn the_window_refills_to_end_where_the_request_ends() {
        let src = Counting::new(100_000);
        let mut w = Backfill::new(&src);

        // The straddle: asking for twelve bytes brings the 4084 below them.
        assert_eq!(bytes(&mut w, req(50_000, 12)).unwrap().len(), 12);
        assert_eq!(src.reads.get(), 1);
        assert_eq!(src.last_len.get(), WINDOW as usize);
        assert_eq!(w.start, 50_012 - WINDOW);

        // Everything already inside it is free.
        for offset in (50_012 - WINDOW..50_012).step_by(4) {
            assert_eq!(
                bytes(&mut w, req(offset, 4)).unwrap(),
                &src.bytes[offset as usize..][..4]
            );
        }
        assert_eq!(src.reads.get(), 1);

        // A range bigger than the window is one read sized to it.
        assert_eq!(bytes(&mut w, req(1_000, 9_000)).unwrap().len(), 9_000);
        assert_eq!(src.reads.get(), 2);
        assert_eq!(src.last_len.get(), 9_000);
    }

    /// The lookahead is bought with bytes: the window still starts where it
    /// would have, and the read grows at its far end.
    ///
    /// That is the whole reason the first block's header is free — the tail this
    /// read carries for the *next* stream is untouched, so the read count is
    /// what it was.
    #[test]
    fn a_lookahead_grows_the_read_and_leaves_the_backward_reach_alone() {
        let src = Counting::new(100_000);
        let mut w = Backfill::new(&src);

        let request = Request {
            start: 50_000,
            len: 12 + 1_024,
            lookahead: 1_024,
        };
        let got = bytes(&mut w, request).unwrap();
        assert_eq!(got.len(), 12 + 1_024);
        assert_eq!(got, &src.bytes[50_000..][..12 + 1_024]);
        assert_eq!(src.reads.get(), 1);
        // One read, WINDOW + lookahead long, and starting exactly where a
        // plain twelve-byte request would have put it.
        assert_eq!(src.last_len.get(), WINDOW as usize + 1_024);
        assert_eq!(w.start, 50_012 - WINDOW);

        // So the previous stream's whole tail is still cached.
        assert_eq!(src.reads.get(), 1);
        assert_eq!(bytes(&mut w, req(50_012 - WINDOW, 4)).unwrap().len(), 4);
        assert_eq!(src.reads.get(), 1);
    }

    #[test]
    fn the_window_clamps_at_the_start_of_the_file_and_hands_a_short_source_back_short() {
        let src = Counting::new(1_000);
        let mut w = Backfill::new(&src);
        assert_eq!(bytes(&mut w, req(0, 12)).unwrap(), &src.bytes[..12]);
        // Clamped at the start of the file: the window still ends where the
        // request ends, so there is simply less of it.
        assert_eq!(w.start, 0);
        assert_eq!(src.last_len.get(), 12);

        // Past the last byte the source has, the cache refuses nothing: the
        // window is what there was, and naming it is the machine's.
        let (at, window) = w.window(req(996, 8)).expect("the cache bounds nothing");
        assert_eq!(at + window.len() as u64, 1_000);
    }

    /// The machine crosses an `await` as a value, which is the whole reason it
    /// exists — and nothing in either type's *text* would notice a field
    /// arriving that broke it.
    #[test]
    fn the_machine_and_its_requests_are_send_and_hold_no_borrow() {
        fn assert_send_and_owned<T: Send + 'static>() {}
        assert_send_and_owned::<Walk>();
        assert_send_and_owned::<Request>();
        assert_send_and_owned::<Step>();
    }

    /// A request held from an earlier step is the one misuse the protocol
    /// cannot design out, and it is a panic rather than another variant of the
    /// taxonomy: it names no offset in a file, in either coordinate space.
    #[test]
    #[should_panic(expected = "not waiting for")]
    fn a_stale_request_is_a_panic() {
        let bytes = synthetic_stream(2, 100, 4_096);
        let (mut machine, first) = Walk::begin(bytes.len() as u64);
        let Step::Need(second) = machine
            .supply(
                first,
                &bytes[first.range().start as usize..first.range().end as usize],
            )
            .expect("the magic is the file's")
        else {
            panic!("a one-stream file is not walked by its magic alone")
        };
        let _ = second;
        let _ = machine.supply(first, &bytes[..6]);
    }

    /// Supplying more than was asked for is the driver's own shape — every walk
    /// in this crate goes through it — and beginning *past* the request is the
    /// half of containment a driver can only get wrong by contradicting itself.
    #[test]
    #[should_panic(expected = "past the start of")]
    fn bytes_beginning_past_the_request_are_a_panic() {
        let (mut machine, first) = Walk::begin(4_096);
        let _ = machine.supply_at(first, 2, &[0u8; 8]);
    }

    /// A supply that runs out early is the file being smaller than the size the
    /// machine was handed: [`Error::Truncated`] where the bytes ran out, and not
    /// a panic, because it names an offset in a file and a driver cannot tell it
    /// from its own slicing.
    #[test]
    fn a_supply_that_runs_out_early_is_a_truncation() {
        let bytes = synthetic_stream(1, 100, 4_096);
        let (mut machine, first) = Walk::begin(bytes.len() as u64);
        let Step::Need(probe) = machine
            .supply(first, &bytes[..first.len as usize])
            .expect("the magic is the file's")
        else {
            panic!("a one-stream file is not walked by its magic alone")
        };

        // The padding probe asks for the file's last four bytes; answer it with
        // three, as a source shorter than its own `size()` would.
        let at = probe.start() as usize;
        assert!(matches!(
            machine.supply_at(probe, at as u64, &bytes[at..at + 3]),
            Err(Error::Truncated { compressed_offset }) if compressed_offset == at as u64 + 3
        ));
    }

    /// The machine bounds every range it emits against the size it was given, so
    /// no driver holds that bound — and nothing a *file* can contain reaches the
    /// check, every range descending from a boundary already placed inside the
    /// file. The two tests below provoke it by shrinking the size out from under
    /// a walk in flight, which is the state a mis-derivation would leave behind;
    /// they differ only in the profile, the assertion being the diagnosis and the
    /// refusal what a release build does in its place.
    fn a_walk_whose_size_shrank() -> (Walk, Request, Vec<u8>) {
        let bytes = synthetic_stream(2, 100, 4_096);
        let (mut machine, first) = Walk::begin(bytes.len() as u64);
        machine.size = 8;
        (machine, first, bytes)
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "mis-derivation in xz-seek")]
    fn the_size_bound_asserts_in_debug() {
        let (mut machine, first, bytes) = a_walk_whose_size_shrank();
        let _ = machine.supply(first, &bytes[..first.len as usize]);
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn the_size_bound_refuses_the_range_in_release() {
        let (mut machine, first, bytes) = a_walk_whose_size_shrank();
        assert!(matches!(
            machine.supply(first, &bytes[..first.len as usize]),
            Err(Error::Truncated {
                compressed_offset: 8
            })
        ));
    }

    /// A source whose `size()` over-reports reaches the same verdict through the
    /// synchronous driver, at the last byte it actually had.
    #[test]
    fn a_source_shorter_than_it_claims_is_truncated_at_its_last_byte() {
        struct Lying {
            bytes: Vec<u8>,
            claimed: u64,
        }
        impl CompressedSource for Lying {
            fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::io::Result<usize> {
                let slice: &[u8] = &self.bytes;
                slice.read_at(offset, buf)
            }
            fn size(&self) -> std::io::Result<u64> {
                Ok(self.claimed)
            }
        }

        let bytes = synthetic_stream(2, 100, 4_096);
        let real = bytes.len() as u64;
        let src = Lying {
            bytes,
            claimed: real + 4,
        };
        assert!(matches!(
            walk(&src),
            Err(Error::Truncated { compressed_offset }) if compressed_offset == real
        ));
    }

    /// The two derivations the walk makes from bytes it has read, checked
    /// against values written out by hand. The differential evidence for the
    /// walk as a whole is `tests/walk.rs`, against the `--robot` oracle.
    #[test]
    fn a_variable_length_integer_is_seven_bits_a_byte_and_must_be_minimal() {
        let mut p = 0;
        assert_eq!(vli(&[0x00], &mut p, 0).unwrap(), 0);
        assert_eq!(p, 1);

        let mut p = 0;
        assert_eq!(vli(&[0x7f], &mut p, 0).unwrap(), 127);

        let mut p = 0;
        assert_eq!(vli(&[0x80, 0x01], &mut p, 0).unwrap(), 128);
        assert_eq!(p, 2);

        let mut p = 0;
        assert_eq!(
            vli(&[0xff, 0xff, 0xff, 0xff, 0x07], &mut p, 0).unwrap(),
            (1u64 << 31) - 1
        );

        // Non-minimal: a continuation byte followed by a terminating zero.
        let mut p = 0;
        assert!(vli(&[0x80, 0x00], &mut p, 0).is_err());
        // Ten bytes is one too many.
        let mut p = 0;
        assert!(vli(&[0x80; 10], &mut p, 0).is_err());
        // Ran out of buffer.
        let mut p = 0;
        assert!(vli(&[0x80], &mut p, 0).is_err());
    }

    #[test]
    fn reserved_stream_flag_bits_are_a_damaged_file_and_a_reserved_check_is_not() {
        assert_eq!(stream_flags([0x00, 0x04], 0).unwrap(), Check::Crc64);
        assert_eq!(
            stream_flags([0x00, 0x05], 0).unwrap(),
            Check::Unsupported(5)
        );
        assert!(stream_flags([0x01, 0x04], 0).is_err());
        assert!(stream_flags([0x00, 0x14], 0).is_err());
    }

    #[test]
    fn round4_is_the_block_padding_the_index_leaves_out() {
        assert_eq!(round4(0), 0);
        assert_eq!(round4(5), 8);
        assert_eq!(round4(12), 12);
        assert_eq!(round4(13), 16);
    }

    fn put_vli(out: &mut Vec<u8>, mut v: u64) {
        while v >= 0x80 {
            out.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        out.push(v as u8);
    }

    /// One synthetic stream: a real header, index and footer around filler.
    ///
    /// The walk never looks inside a block, so the blocks can be filler — which
    /// is what makes it possible to hand-build a stream with an index larger
    /// than the window without an encoder. `xz` would reject the filler, so this
    /// stays a unit test rather than becoming a fixture.
    fn synthetic_stream(blocks: u64, unpadded: u64, uncompressed: u64) -> Vec<u8> {
        let flags = [0x00u8, 0x04];
        let mut out = Vec::new();
        out.extend_from_slice(&HEADER_MAGIC);
        out.extend_from_slice(&flags);
        out.extend_from_slice(&crate::check::crc32(&flags).to_le_bytes());
        out.resize(out.len() + (blocks * round4(unpadded)) as usize, 0xaa);

        let mut index = vec![0x00];
        put_vli(&mut index, blocks);
        for _ in 0..blocks {
            put_vli(&mut index, unpadded);
            put_vli(&mut index, uncompressed);
        }
        index.resize(round4(index.len() as u64 + 4) as usize - 4, 0x00);
        let crc = crate::check::crc32(&index);
        index.extend_from_slice(&crc.to_le_bytes());
        let backward = (index.len() / 4 - 1) as u32;
        out.extend_from_slice(&index);

        let mut footer_body = backward.to_le_bytes().to_vec();
        footer_body.extend_from_slice(&flags);
        out.extend_from_slice(&crate::check::crc32(&footer_body).to_le_bytes());
        out.extend_from_slice(&footer_body);
        out.extend_from_slice(&FOOTER_MAGIC);
        out
    }

    /// The window's fallback, end to end.
    ///
    /// No fixture reaches it: the biggest index in the corpus is a few dozen
    /// bytes, because a 4 KiB index needs on the order of 1,500 blocks. The real
    /// multiblock corpus file has ~5,700 and lands here on every walk, so the
    /// path is not exotic — it is simply too large to generate into `fixtures/`.
    #[test]
    fn an_index_larger_than_the_window_is_read_whole_and_still_tiles() {
        let one = synthetic_stream(1_500, 100, 4_096);
        let mut bytes = one.clone();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // stream padding
        bytes.extend_from_slice(&one);
        let src = Counting {
            bytes,
            reads: Cell::new(0),
            last_len: Cell::new(0),
        };

        let t = walk(&src).expect("the synthetic file walks");
        assert_eq!(t.stream_count(), 2);
        assert_eq!(t.block_count(), 3_000);
        assert_eq!(t.uncompressed_size(), 3_000 * 4_096);
        assert_eq!(t.compressed_file_size, (one.len() * 2 + 4) as u64);
        assert_eq!(t.streams[0].padding, 4);
        assert_eq!(t.streams[1].padding, 0);
        assert_eq!(t.streams[0].compressed_size, one.len() as u64);
        assert_eq!(t.streams[1].first_block, 1_500);

        // The blocks tile each stream from its header onward.
        for (i, b) in t.blocks.iter().enumerate() {
            let s = &t.streams[i / 1_500];
            let n = (i % 1_500) as u64;
            assert_eq!(b.compressed_offset, s.compressed_offset + 12 + n * 100);
            assert_eq!(b.uncompressed_offset, s.uncompressed_offset + n * 4_096);
            assert_eq!(b.unpadded_size, 100);
        }

        // The magic, two tail reads and two sized index reads: still no more
        // than one read a stream beyond the ones the window cannot avoid.
        assert!(src.reads.get() <= 1 + 2 * (t.stream_count() + 1));

        // The blocks here are filler, so no first block header parses. That is
        // `None` rather than a failed walk: the index region is intact and the
        // file's geometry is exactly what it says it is, but no dictionary was
        // read, and only `None` can say so — zero is a dictionary a header can
        // declare.
        assert_eq!(t.streams[0].first_block_dict_size, None);
        assert_eq!(t.streams[1].first_block_dict_size, None);
    }

    fn walk_bytes(bytes: Vec<u8>) -> Result<SeekTable> {
        let src: &[u8] = &bytes;
        walk(&src)
    }

    /// The tail-most stream is the only one a cut can reach.
    ///
    /// No fixture reaches these: the corpus damages the tail, which is what both
    /// a `head -c` cut and a flipped footer bit are, so the two synthetic files
    /// here are the only place the inner boundary is exercised. Building a
    /// fixture for it would mean writing a multi-stream file whose *inner*
    /// footer is wrong, which no recipe over `xz`'s own output produces.
    #[test]
    fn a_missing_footer_below_a_derived_boundary_is_not_a_truncation() {
        let one = synthetic_stream(2, 100, 4_096);
        let mut bytes = one.clone();
        bytes.extend_from_slice(&one);

        // The tail stream walks, derives its start, and the walk then looks for
        // a footer immediately below it. Break only that one's magic.
        let at = one.len() - 2;
        bytes[at..at + 2].copy_from_slice(b"XX");

        assert!(
            matches!(
                walk_bytes(bytes),
                Err(Error::IndexInconsistent {
                    compressed_offset
                }) if compressed_offset == one.len() as u64 - STREAM_FOOTER_SIZE
            ),
            "the interior of a file that reaches its own end is not cut short"
        );

        // The same damage at the file's own end *is* a cut: one rule, two
        // answers, and the only difference is which boundary it was found at.
        let mut tail = one.clone();
        let at = tail.len() - 2;
        tail[at..at + 2].copy_from_slice(b"XX");
        assert!(matches!(walk_bytes(tail), Err(Error::Truncated { .. })));
    }

    #[test]
    fn a_derived_start_with_no_room_for_a_stream_is_not_a_truncation() {
        // Eight bytes of junk carrying the file's magic, then a whole stream.
        // The tail stream's derived start is 8, so the walk continues below it
        // and finds less than a minimum stream's worth of file there.
        let mut bytes = HEADER_MAGIC.to_vec();
        bytes.extend_from_slice(&[0xaa, 0xaa]);
        bytes.extend_from_slice(&synthetic_stream(2, 100, 4_096));

        assert!(matches!(
            walk_bytes(bytes),
            Err(Error::IndexInconsistent {
                compressed_offset: 8
            })
        ));

        // A file that is *only* those eight bytes has no derived boundary below
        // it, so the same floor is the truncation it was written for.
        let mut stub = HEADER_MAGIC.to_vec();
        stub.extend_from_slice(&[0xaa, 0xaa]);
        assert!(matches!(
            walk_bytes(stub),
            Err(Error::Truncated {
                compressed_offset: 8
            })
        ));
    }
}
