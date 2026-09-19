//! The error taxonomy.
//!
//! One `#[non_exhaustive]` enum of thirteen variants: R7's six, plus
//! [`Error::Io`], [`Error::InvalidTable`], [`Error::UnsupportedCheck`],
//! [`Error::BlockDataError`], [`Error::BackendUnavailable`],
//! [`Error::BufferTooSmall`] and [`Error::OffsetPastBlockEnd`]. The rationale
//! for each addition is in `docs/design/roadmap.md`, "Errors name the case and
//! the offset"; the enum's shape is `docs/design/decisions.md`, "D28".
//!
//! **Every variant that is about a file carries the byte offset at which the
//! fault was detected, in the compressed file's coordinate space** — one space,
//! so no variant has to be read to find out which one it meant. Two variants
//! carry an uncompressed range beside it, because "bytes X..Y of your dump are
//! bad" is the sentence a downstream reports.
//!
//! **A variant about the *build* or the caller's own call rather than about the
//! file reports no position**, and [`Error::compressed_offset`] returns `None`
//! for it. There are three — [`Error::BackendUnavailable`], the build does not
//! carry the decoder the caller named; [`Error::BufferTooSmall`], the buffer
//! offered to a whole-block decode is shorter than the block; and
//! [`Error::OffsetPastBlockEnd`], a handle asked to advance beyond the block it
//! holds — and the rule is
//! stated rather than the count because the match below is what enforces it: it
//! is exhaustive with named arms, so a further variant fails to compile until
//! somebody decides which side it falls on. A fabricated `compressed_offset: 0`
//! would ask nobody that. The rejected alternatives are
//! `docs/design/decisions.md`, "D29".
//!
//! # When a fault is "unsupported" rather than "damaged"
//!
//! Bytes that are well-formed behind a matching CRC32 but name something this
//! version of the format does not assign are not damage: a reserved filter id, a
//! reserved check id, reserved stream- or block-flag bits, an unassigned LZMA2
//! dictionary code. The format's CRC32s exist precisely to separate that case
//! from corruption, and a decoder that supported the thing would want to say
//! "unsupported".
//!
//! **The rule is that a variant names the unsupported thing only when it can
//! carry the thing's identifier.** [`Error::UnsupportedFilter`] carries a
//! `filter_id` and [`Error::UnsupportedCheck`] a `check_id`, so both are
//! actionable: a caller learns *which* filter or check to go and get. Nothing in
//! the taxonomy can carry a flag bit or a dictionary code, and inventing a
//! variant that could was rejected — those are
//! [`Error::IndexInconsistent`], whose offset points at the byte, so the
//! information is not lost and only the name is. That rule is about faults in a
//! file, and **that half of the taxonomy is closed at ten**: every variant added
//! since names something that is not a fault in a file — a build that cannot
//! serve the request, a call that cannot be served.
//!
//! **A variant joins that other half only if the caller can still place the
//! fault**: either it is raised before any byte of the file is read, or it names
//! a position in the *uncompressed* coordinate space. That is the bar, rather
//! than "it arrived with an entry point that could fail that way", which admits
//! anything: every new entry point can invent a failure. It is what keeps
//! [`Error::compressed_offset`]'s `None` honest — a fault found mid-read that
//! named no position at all would leave a caller who has already been handed
//! bytes with no way to place it. [`Error::BackendUnavailable`] and
//! [`Error::BufferTooSmall`] satisfy the first clause, the backend resolved in
//! the constructor and the buffer measured before the block header, and **each
//! is held to it by a test that drives its entry point over a source answering
//! nothing**, so a check moved below a read is a failure rather than a silent
//! demotion: `tests/taxonomy.rs` for the backend, `src/task.rs`'s short-buffer
//! test for the buffer. [`Error::OffsetPastBlockEnd`] satisfies the second: it
//! arrives mid-block carrying the block the target missed, which places it in
//! the uncompressed space. The clause is about naming a position, not about
//! answering [`Error::uncompressed_range`], which stays the damage variants'.
//!
//! **This partition is the crate's, not `liblzma`'s**, and agreement with the
//! reference decoder is not what decides it: `liblzma` answers `LZMA_DATA_ERROR`
//! for a reserved filter id where this crate answers
//! [`Error::UnsupportedFilter`], and [`Error::UnsupportedCheck`] is a case `xz`
//! cannot produce at all. What the differential harness holds this crate to is
//! the *bytes* a read returns, not the reference implementation's choice of
//! error class.

use core::fmt;
use core::ops::Range;

use crate::backend::Backend;

/// This crate's result type.
pub type Result<T> = core::result::Result<T, Error>;

/// Everything that can go wrong reading an `.xz` file positionally.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The source does not hold an `.xz` file: the six magic bytes at offset 0
    /// are not `\xfd7zXZ\x00`.
    ///
    /// The magic is read once by a reader that walks, and it is what separates
    /// this from [`Error::Truncated`]: both a clipped download and a random
    /// file fail at the tail, and only the magic says which one the caller has.
    /// A reader built from a persisted table reads no byte at all, so this
    /// variant is unreachable for it.
    NotXz {
        /// Always 0 — the magic is at the start of the file.
        compressed_offset: u64,
    },

    /// The file claims to be `.xz` and ends before its structure does.
    ///
    /// A truncation removes a suffix, so this is raised only where the parse is
    /// working at the file's own end. The same damage found below a boundary the
    /// walk already derived is [`Error::IndexInconsistent`] — see `src/walk.rs`,
    /// "Only the tail of a file can be cut".
    Truncated {
        /// Where the parse ran off the end.
        compressed_offset: u64,
    },

    /// A file's index region is present and disagrees with itself.
    ///
    /// R7 names the narrow case — a stream index that does not match the blocks
    /// it describes — and this variant covers it: a block header that is not
    /// where the index says one is, or a decoded block whose unpadded or
    /// uncompressed size is not the one the index recorded. It covers the rest
    /// of the container's self-contradictions too, because a caller acts on all
    /// of them identically and the offset says which it was: a footer, index or
    /// stream-header CRC32 that fails, reserved stream-flag bits, a non-minimal
    /// variable-length integer, an unpadded size below the format's floor,
    /// index padding that is not null, a derived offset outside the file, a
    /// stream header whose flags differ from its footer's copy, and a stream
    /// that does not end where an earlier stream's header said it would.
    ///
    /// What separates it from [`Error::Truncated`] is whether the structure is
    /// *there*, and whether the parse is at the file's own end: an absent
    /// stream-footer magic at the tail is a file that was cut, the same absence
    /// at a derived inner boundary is a file that contradicts itself, and a
    /// present footer that fails its own CRC32 is damage wherever it is found.
    /// See `src/walk.rs`, "What a failure is called" and "Only the tail of a
    /// file can be cut".
    ///
    /// Its mirror for a caller's *persisted* table is [`Error::InvalidTable`],
    /// and the two are kept apart so that a downstream is never told to re-walk
    /// a file over its own stale cache.
    IndexInconsistent {
        /// Where the disagreement was found.
        compressed_offset: u64,
    },

    /// A block's integrity check did not match its decoded bytes.
    ///
    /// Verification completes at the block's end, so on a forward walk this
    /// arrives from a later call than the one that returned the bytes it covers.
    BlockCheckFailed {
        /// The block's own compressed offset — the first byte of its header.
        compressed_offset: u64,
        /// The whole block's uncompressed range.
        uncompressed_range: Range<u64>,
    },

    /// A block's filter chain names a filter this crate cannot build.
    UnsupportedFilter {
        /// The block header the chain was read from.
        compressed_offset: u64,
        /// The filter id, as the header's variable-length integer decoded it.
        filter_id: u64,
    },

    /// A block's declared dictionary is larger than the configured limit.
    ///
    /// Stated in **dictionary bytes**, which is the only part of the footprint
    /// the file dictates, and enforced from this crate's own header parse before
    /// any backend object is constructed.
    MemoryLimitExceeded {
        /// The block header the dictionary size was read from.
        compressed_offset: u64,
        /// What the block declares, in bytes.
        required: u64,
        /// What the reader was configured to allow, in bytes.
        limit: u64,
    },

    /// The caller's byte source failed.
    ///
    /// A failure of the caller's own byte source is not the file's fault and is
    /// never reported as corruption.
    Io {
        /// The offset the failing read was addressed at.
        compressed_offset: u64,
        /// The source's error, carried losslessly — `io::Error::other` is how an
        /// HTTP or object-store failure reaches here intact.
        source: std::io::Error,
    },

    /// A table handed back to the re-injecting constructor is not internally
    /// consistent, or does not describe this source.
    ///
    /// The same fault as [`Error::IndexInconsistent`] found at a different
    /// moment — from the caller's persisted copy rather than from the file.
    InvalidTable {
        /// The offset in the table's own coordinate space at which it stopped
        /// making sense.
        compressed_offset: u64,
    },

    /// A stream declares one of the format's reserved check ids.
    ///
    /// `xz` decodes such a file with a warning rather than refusing, so this is
    /// not a parse failure: the walk records
    /// [`Check::Unsupported`](crate::Check::Unsupported) and the shape stays
    /// inspectable. The error is raised only when a block in such a stream is
    /// decoded under full verification, where returning the bytes would hand a
    /// caller who asked for verification a block that was never verified.
    UnsupportedCheck {
        /// The block being decoded when the unsupported check came due.
        compressed_offset: u64,
        /// The reserved id the stream's flags carried, `0..=0x0f`.
        check_id: u8,
    },

    /// A block payload did not decode to a complete block.
    ///
    /// It means exactly that, and deliberately not "the range coder derailed":
    /// each backend partitions the failure its own way, and what both must
    /// satisfy is that the block's declared uncompressed size did not arrive.
    ///
    /// `uncompressed_range` is the **whole block's**, which overstates the
    /// damage on purpose — everything after it is unrecoverable and everything
    /// before it is unverifiable, because the check spans the block and can no
    /// longer be completed.
    ///
    /// A filter chain this crate maps wrongly derails a good file and lands
    /// here too, so one of our own bugs can arrive wearing the file's name. That
    /// is accepted: the block header's CRC32 is verified before the chain is
    /// built, so the bytes the chain came from are known good, and a
    /// mis-mapping fails over the whole fixture corpus rather than at one file.
    BlockDataError {
        /// `block header offset + header size + input consumed`, always inside
        /// the payload. Under `liblzma` that is the byte the decoder stopped
        /// at; `xz4rust` discards a failed call's progress, so under it this
        /// is the first byte of the chunk the decode failed in.
        compressed_offset: u64,
        /// The whole block's uncompressed range.
        uncompressed_range: Range<u64>,
    },

    /// The buffer handed to [`BlockTask::decode_into`](crate::BlockTask::decode_into)
    /// is smaller than the block it was asked to hold.
    ///
    /// That path fills the caller's buffer with a whole block's output or
    /// nothing at all — a partial fill would be indistinguishable from a block
    /// that decoded short, on the one path whose contract is that a returned
    /// buffer holds a verified block. The seek table already knows the exact
    /// length, and [`BlockTask::uncompressed_len`](crate::BlockTask::uncompressed_len)
    /// hands it over, so this is a caller who did not ask.
    ///
    /// It is about the call rather than about the file — the file is fine, and
    /// nothing in it has been read — so
    /// [`Error::compressed_offset`] is `None`, the same side of the line
    /// [`Error::BackendUnavailable`] falls on.
    BufferTooSmall {
        /// The block's uncompressed size, in bytes, which is what the buffer
        /// must be at least.
        required: u64,
        /// The buffer the caller offered, in bytes.
        given: u64,
    },

    /// The caller named a decoder backend this build does not carry.
    ///
    /// [`Backend`]'s variants exist whatever features are on, so a caller's
    /// `match` compiles in every configuration and a request for an absent one
    /// is a question asked at run time. This is the run-time answer, raised by
    /// [`Builder::open`](crate::Builder::open) and
    /// [`Builder::open_with_table`](crate::Builder::open_with_table) **before
    /// any byte is read**, so a caller who asked for the wrong build learns
    /// before paying for a footer walk.
    ///
    /// It is not the file's fault and names no place in it, so
    /// [`Error::compressed_offset`] is `None` — the rule for every variant
    /// about the build rather than the file, and this is the only one. A build
    /// carrying *no* backend is a different thing entirely and is a compile
    /// error.
    BackendUnavailable {
        /// The backend that was asked for. [`Backend::feature`] is the Cargo
        /// feature that would compile it.
        backend: Backend,
    },

    /// [`BlockRead::advance_to`](crate::BlockRead::advance_to) was asked for an
    /// offset past the end of the block its handle holds.
    ///
    /// A handle is one block's, and the next block's bytes are the next
    /// [`BlockTask`](crate::BlockTask)'s, so there is nothing to advance to. It
    /// is a variant rather than a panic because the bound it crossed is an
    /// **index claim**: the block's end comes from the table, so a target past
    /// it may be the right answer and the *table* wrong, and a value that
    /// reached us from a table a caller persisted is data, however wrong, which
    /// earns a refusal (`docs/design/roadmap.md`, "What panics, and what joins
    /// the error taxonomy"). Advancing *backward* crosses the other kind of
    /// bound — [`BlockRead::position`](crate::BlockRead::position) is this
    /// decode's own count of what it has produced — so no state of the file
    /// makes such a target right, and it still panics.
    ///
    /// It names no compressed offset: the arithmetic that produced the target
    /// never touched the compressed space, and
    /// [`BlockTask::compressed_range`](crate::BlockTask::compressed_range) has
    /// the block's compressed extent for a caller that wants it. It does not
    /// answer [`Error::uncompressed_range`] either, which stays the two damage
    /// variants' accessor; the block it holds is in `block`, which says *which*
    /// entry of a persisted table disagreed with the file.
    OffsetPastBlockEnd {
        /// The file-absolute uncompressed offset the handle was asked for.
        uncompressed_offset: u64,
        /// The block the handle holds, whose end the offset is past.
        block: Range<u64>,
    },
}

impl Error {
    /// The offset at which the fault was detected, in the compressed file's
    /// coordinate space.
    ///
    /// Every variant about a file has one, which is why this is a method and
    /// not a match the caller writes. A variant about the build or the caller's
    /// own call returns `None` — [`Error::BackendUnavailable`],
    /// [`Error::BufferTooSmall`] and [`Error::OffsetPastBlockEnd`], the first
    /// two raised before a byte of the file is read and the third naming its
    /// position in the uncompressed space instead, which is the rule the module
    /// docs state for admitting a fourth.
    ///
    /// **The match is exhaustive with named arms on purpose.** It is the one
    /// place a new variant is obliged to say whether it names a byte in a file,
    /// and it does not compile until it has.
    pub fn compressed_offset(&self) -> Option<u64> {
        match self {
            Error::NotXz { compressed_offset }
            | Error::Truncated { compressed_offset }
            | Error::IndexInconsistent { compressed_offset }
            | Error::BlockCheckFailed {
                compressed_offset, ..
            }
            | Error::UnsupportedFilter {
                compressed_offset, ..
            }
            | Error::MemoryLimitExceeded {
                compressed_offset, ..
            }
            | Error::Io {
                compressed_offset, ..
            }
            | Error::InvalidTable { compressed_offset }
            | Error::UnsupportedCheck {
                compressed_offset, ..
            }
            | Error::BlockDataError {
                compressed_offset, ..
            } => Some(*compressed_offset),
            Error::BufferTooSmall { .. }
            | Error::BackendUnavailable { .. }
            | Error::OffsetPastBlockEnd { .. } => None,
        }
    }

    /// The uncompressed range the fault ruins, where the variant names one.
    ///
    /// Damage only: [`Error::OffsetPastBlockEnd`] carries a block that is not
    /// ruined, and keeps it in a field of its own instead
    /// (`docs/design/decisions.md`, "D68").
    pub fn uncompressed_range(&self) -> Option<Range<u64>> {
        match self {
            Error::BlockCheckFailed {
                uncompressed_range, ..
            }
            | Error::BlockDataError {
                uncompressed_range, ..
            } => Some(uncompressed_range.clone()),
            _ => None,
        }
    }

    /// Wrap a source failure at a known offset.
    pub fn io(compressed_offset: u64, source: std::io::Error) -> Error {
        Error::Io {
            compressed_offset,
            source,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The variants with no compressed offset are written first and return,
        // so `at` is unconditionally there for every arm below.
        match self {
            Error::BackendUnavailable { backend } => {
                return write!(
                    f,
                    "the {backend} backend is not compiled into this build: enable \
                     the `{}` feature",
                    backend.feature()
                );
            }
            Error::BufferTooSmall { required, given } => {
                return write!(
                    f,
                    "the buffer is too small for the block: {required} bytes are \
                     needed and {given} were offered"
                );
            }
            Error::OffsetPastBlockEnd {
                uncompressed_offset,
                block,
            } => {
                return write!(
                    f,
                    "uncompressed offset {uncompressed_offset} is past the end of the \
                     block covering {}..{}",
                    block.start, block.end
                );
            }
            _ => {}
        }
        let at = self
            .compressed_offset()
            .expect("only the three variants above have no compressed offset");
        match self {
            Error::NotXz { .. } => write!(f, "not an xz file: bad magic at offset {at}"),
            Error::Truncated { .. } => write!(f, "truncated xz file: ends at offset {at}"),
            Error::IndexInconsistent { .. } => {
                write!(f, "stream index disagrees with the blocks, at offset {at}")
            }
            Error::BlockCheckFailed {
                uncompressed_range, ..
            } => write!(
                f,
                "block integrity check failed: block at offset {at} covering uncompressed {}..{}",
                uncompressed_range.start, uncompressed_range.end
            ),
            Error::UnsupportedFilter { filter_id, .. } => {
                write!(
                    f,
                    "unsupported filter {filter_id:#x} in block at offset {at}"
                )
            }
            Error::MemoryLimitExceeded {
                required, limit, ..
            } => write!(
                f,
                "memory limit exceeded: block at offset {at} declares a \
                 {required}-byte dictionary, limit is {limit} bytes"
            ),
            Error::Io { source, .. } => write!(f, "source read failed at offset {at}: {source}"),
            Error::InvalidTable { .. } => {
                write!(f, "the supplied seek table is invalid, at offset {at}")
            }
            Error::UnsupportedCheck { check_id, .. } => write!(
                f,
                "unsupported integrity check {check_id:#x} on the stream holding \
                 the block at offset {at}"
            ),
            Error::BlockDataError {
                uncompressed_range, ..
            } => write!(
                f,
                "block payload did not decode to a complete block: stopped at offset {at}, \
                 block covers uncompressed {}..{}",
                uncompressed_range.start, uncompressed_range.end
            ),
            // Handled above, where their lack of a compressed offset is what
            // selects them.
            Error::BackendUnavailable { .. }
            | Error::BufferTooSmall { .. }
            | Error::OffsetPastBlockEnd { .. } => unreachable!(),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_variant() -> Vec<Error> {
        vec![
            Error::NotXz {
                compressed_offset: 0,
            },
            Error::Truncated {
                compressed_offset: 1,
            },
            Error::IndexInconsistent {
                compressed_offset: 2,
            },
            Error::BlockCheckFailed {
                compressed_offset: 3,
                uncompressed_range: 0..64,
            },
            Error::UnsupportedFilter {
                compressed_offset: 4,
                filter_id: 0x0a,
            },
            Error::MemoryLimitExceeded {
                compressed_offset: 5,
                required: 1 << 26,
                limit: 1 << 20,
            },
            Error::io(6, std::io::Error::other("the object store said no")),
            Error::InvalidTable {
                compressed_offset: 7,
            },
            Error::UnsupportedCheck {
                compressed_offset: 8,
                check_id: 5,
            },
            Error::BlockDataError {
                compressed_offset: 9,
                uncompressed_range: 64..128,
            },
        ]
    }

    #[test]
    fn every_variant_about_a_file_carries_its_offset_and_says_it() {
        let all = every_variant();
        assert_eq!(all.len(), 10, "R7's six plus the four this crate adds");
        for (i, e) in all.iter().enumerate() {
            assert_eq!(e.compressed_offset(), Some(i as u64));
            let text = e.to_string();
            assert!(
                text.contains(&i.to_string()),
                "{text:?} does not name its offset"
            );
        }
    }

    /// The variants that are not about a file have no compressed offset — and
    /// each names what a caller would act on instead.
    #[test]
    fn the_buffer_variant_has_no_offset_and_names_both_lengths() {
        let e = Error::BufferTooSmall {
            required: 65_536,
            given: 4_096,
        };
        assert_eq!(e.compressed_offset(), None);
        assert_eq!(e.uncompressed_range(), None);
        let text = e.to_string();
        assert!(text.contains("65536") && text.contains("4096"), "{text:?}");
        assert!(
            !text.contains("offset"),
            "{text:?} talks about a position it does not have"
        );
    }

    /// The build variant is about the build, not the file, so it has no
    /// offset — and it names the feature that would fix it instead.
    #[test]
    fn the_backend_variant_has_no_offset_and_names_its_feature() {
        for backend in [Backend::Liblzma, Backend::Xz4rust] {
            let e = Error::BackendUnavailable { backend };
            assert_eq!(e.compressed_offset(), None);
            assert_eq!(e.uncompressed_range(), None);
            let text = e.to_string();
            assert!(text.contains(backend.feature()), "{text:?}");
            assert!(
                !text.contains("offset"),
                "{text:?} talks about a position it does not have"
            );
        }
    }

    /// The third variant with no compressed offset, and the only one of the
    /// three that names a position at all — in the *uncompressed* space, which
    /// is the bar the module docs state for admitting it. It places itself
    /// through its own field, not through an accessor: `uncompressed_range()`
    /// is the damage variants', and answering it here would make one accessor
    /// mean both "bytes this ruins" and "a block that is fine".
    #[test]
    fn the_past_the_block_variant_answers_neither_accessor() {
        let e = Error::OffsetPastBlockEnd {
            uncompressed_offset: 4_096,
            block: 1_024..2_048,
        };
        assert_eq!(e.compressed_offset(), None);
        assert_eq!(e.uncompressed_range(), None);
        let text = e.to_string();
        assert!(
            text.contains("4096") && text.contains("1024") && text.contains("2048"),
            "{text:?}"
        );
    }

    #[test]
    fn only_the_two_range_variants_name_an_uncompressed_range() {
        let mut all = every_variant();
        all.push(Error::OffsetPastBlockEnd {
            uncompressed_offset: 256,
            block: 128..192,
        });
        let ranged: Vec<_> = all.iter().filter_map(|e| e.uncompressed_range()).collect();
        assert_eq!(ranged, vec![0..64, 64..128]);
    }

    #[test]
    fn only_io_has_a_source() {
        use std::error::Error as _;
        let sourced = every_variant()
            .iter()
            .filter(|e| e.source().is_some())
            .count();
        assert_eq!(sourced, 1);
    }
}
