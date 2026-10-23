//! What a decode, a walk or an index refuses, each failure in the file
//! carrying the offset — compressed or plaintext, as its field says — that it
//! names.

use std::fmt;

/// This crate's result.
pub type Result<T> = std::result::Result<T, Error>;

/// Every refusal this crate makes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The bytes at `offset`, where a member must begin, are not gzip's
    /// magic: at offset zero the file is not gzip, past it the file carries
    /// something after its last member.
    NotGzip {
        /// Compressed offset.
        offset: u64,
    },
    /// The member header at `offset` is gzip's and cannot be read: a method
    /// other than deflate, a reserved flag, or a header CRC that does not
    /// match.
    Header {
        /// Compressed offset of the member.
        offset: u64,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The deflate data is invalid; `offset` is where the failing step's input
    /// began.
    Deflate {
        /// Compressed offset.
        offset: u64,
        /// The inflater's message.
        message: String,
    },
    /// A member's trailer CRC-32 does not match its plaintext.
    MemberCheck {
        /// Compressed offset of the member.
        offset: u64,
        /// The trailer's value.
        stored: u32,
        /// What the plaintext gives.
        computed: u32,
    },
    /// A member's trailer `ISIZE` does not match its plaintext's length,
    /// modulo 2³².
    MemberLength {
        /// Compressed offset of the member.
        offset: u64,
        /// The trailer's value.
        stored: u32,
        /// The plaintext's length, modulo 2³².
        computed: u32,
    },
    /// A unit's plaintext does not match the CRC-32 the index recorded for it.
    UnitCheck {
        /// Plaintext offset of the unit.
        plaintext: u64,
        /// The index's value.
        stored: u32,
        /// What the decode gives.
        computed: u32,
    },
    /// The input ended inside a member or a unit's compressed range, or held
    /// no member at all.
    Truncated {
        /// Compressed offset the bytes ran out at.
        offset: u64,
    },
    /// A unit decoded to something other than what its index says: a
    /// different length, or an end at a different place in the file.
    IndexMismatch {
        /// Plaintext offset the disagreement was found at.
        plaintext: u64,
        /// What disagreed.
        reason: &'static str,
    },
    /// The window handed to a resume is not the length its checkpoint states.
    WindowLength {
        /// The checkpoint's.
        expected: usize,
        /// The caller's.
        given: usize,
    },
    /// An index's parts do not describe a gzip file this crate could have
    /// built them from.
    InvalidIndex {
        /// What is wrong with them.
        reason: &'static str,
    },
    /// A member is not a BGZF block: a header not deflate's, no `BC` subfield
    /// where BGZF puts it, a `BSIZE` below the smallest member, or a plaintext
    /// larger than a BGZF block holds.
    NotBgzf {
        /// Compressed offset of the member, or for an oversized plaintext of
        /// the first member of the unit holding it.
        offset: u64,
    },
    /// An output buffer that is not the length the call needs.
    BufferSize {
        /// What the call needs.
        expected: usize,
        /// What it was given.
        given: usize,
    },
    /// The inflater failed in a way its input does not explain: its return
    /// code.
    Backend {
        /// zlib's return code.
        code: i32,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotGzip { offset: 0 } => write!(f, "not a gzip file"),
            Error::NotGzip { offset } => {
                write!(f, "bytes at offset {offset} are not a gzip member")
            }
            Error::Header { offset, reason } => {
                write!(f, "gzip member at offset {offset}: {reason}")
            }
            Error::Deflate { offset, message } => {
                write!(f, "invalid deflate data near offset {offset}: {message}")
            }
            Error::MemberCheck { offset, stored, computed } => write!(
                f,
                "gzip member at offset {offset}: CRC-32 {computed:08x} does not match its \
                 trailer's {stored:08x}"
            ),
            Error::MemberLength { offset, stored, computed } => write!(
                f,
                "gzip member at offset {offset}: length {computed} (mod 2^32) does not match \
                 its trailer's {stored}"
            ),
            Error::UnitCheck { plaintext, stored, computed } => write!(
                f,
                "unit at plaintext offset {plaintext}: CRC-32 {computed:08x} does not match \
                 the index's {stored:08x}"
            ),
            Error::Truncated { offset } => write!(f, "gzip data ends early, at offset {offset}"),
            Error::IndexMismatch { plaintext, reason } => {
                write!(
                    f,
                    "the file disagrees with its index at plaintext offset {plaintext}: {reason}"
                )
            }
            Error::WindowLength { expected, given } => {
                write!(f, "a window of {given} bytes where the checkpoint states {expected}")
            }
            Error::InvalidIndex { reason } => write!(f, "invalid gzip index: {reason}"),
            Error::NotBgzf { offset } => write!(f, "gzip member at offset {offset} is not BGZF"),
            Error::BufferSize { expected, given } => {
                write!(f, "a buffer of {given} bytes where {expected} are needed")
            }
            Error::Backend { code } => write!(f, "the inflater failed with code {code}"),
        }
    }
}

impl std::error::Error for Error {}
