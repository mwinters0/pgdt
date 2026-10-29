//! gzip's framing around each member's deflate data (RFC 1952): the header,
//! read a byte at a time so that a header split across any two inputs reads as
//! one, and the trailer's CRC-32 and `ISIZE`.

use crate::error::{Error, Result};

/// gzip's magic and the deflate method byte that follows it.
pub const GZIP_MAGIC: [u8; 3] = [0x1f, 0x8b, 0x08];

/// A member's fixed header, before any optional field.
const FIXED_LEN: usize = 10;

const FHCRC: u8 = 0x02;
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const RESERVED: u8 = 0xe0;

/// The header bytes kept: as many as BGZF recognition reads, the fixed
/// header and `XLEN` among them, which every field after is read by.
pub(crate) const KEPT: usize = 18;

/// Where a header read has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Fixed,
    ExtraLength,
    Extra {
        left: usize,
    },
    Name,
    Comment,
    /// The header CRC, its first byte once read.
    HeaderCrc {
        first: Option<u8>,
    },
    Done,
}

/// One member header, read incrementally.
///
/// It holds its first [`KEPT`] bytes and the CRC-32 of every byte before the
/// header CRC, running, never a name or comment, which no length bounds.
#[derive(Debug)]
pub(crate) struct Header {
    /// The member's compressed offset, for the errors.
    offset: u64,
    /// How many bytes have been read.
    len: u64,
    /// The first [`KEPT`] of them.
    head: [u8; KEPT],
    /// The CRC-32 of every byte before the header CRC's own two read so far:
    /// what the header CRC covers (RFC 1952, 2.3.1).
    crc: u32,
    field: Field,
}

impl Header {
    pub(crate) fn new(offset: u64) -> Header {
        Header { offset, len: 0, head: [0; KEPT], crc: 0, field: Field::Fixed }
    }

    /// Read from `input` until the header ends or the input does, answering
    /// how many bytes it took.
    pub(crate) fn read(&mut self, input: &[u8]) -> Result<usize> {
        let mut taken = 0;
        // Where the bytes the running CRC-32 has yet to take begin, until the
        // header CRC is reached.
        let mut uncovered = Some(0);
        while self.field != Field::Done && taken < input.len() {
            if let (Field::HeaderCrc { .. }, Some(from)) = (self.field, uncovered) {
                self.crc = crate::zlib::crc32(self.crc, &input[from..taken]);
                uncovered = None;
            }
            self.push(input[taken])?;
            taken += 1;
        }
        if let Some(from) = uncovered {
            self.crc = crate::zlib::crc32(self.crc, &input[from..taken]);
        }
        Ok(taken)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.field == Field::Done
    }

    /// The header's first bytes, as many as BGZF recognition reads: the
    /// whole header where it is shorter, once [`Header::is_done`].
    pub(crate) fn head(&self) -> &[u8] {
        &self.head[..usize::try_from(self.len).map_or(KEPT, |len| len.min(KEPT))]
    }

    fn push(&mut self, byte: u8) -> Result<()> {
        if let Some(slot) = usize::try_from(self.len).ok().and_then(|at| self.head.get_mut(at)) {
            *slot = byte;
        }
        self.len += 1;
        let n = self.len;
        match self.field {
            Field::Fixed => {
                let at = (n - 1) as usize;
                if at < GZIP_MAGIC.len() && byte != GZIP_MAGIC[at] {
                    return Err(if at + 1 == GZIP_MAGIC.len() {
                        self.refused("compression method is not deflate")
                    } else {
                        Error::NotGzip { offset: self.offset }
                    });
                }
                if n == 4 && byte & RESERVED != 0 {
                    return Err(self.refused("reserved flag bits are set"));
                }
                if n == FIXED_LEN as u64 {
                    self.field = self.after(Field::Fixed);
                }
            }
            Field::ExtraLength => {
                if n == FIXED_LEN as u64 + 2 {
                    let left = usize::from(u16::from_le_bytes([self.head[FIXED_LEN], byte]));
                    self.field = if left == 0 {
                        self.after(Field::Extra { left })
                    } else {
                        Field::Extra { left }
                    };
                }
            }
            Field::Extra { left } => {
                self.field = if left == 1 {
                    self.after(Field::Extra { left })
                } else {
                    Field::Extra { left: left - 1 }
                };
            }
            Field::Name | Field::Comment => {
                if byte == 0 {
                    self.field = self.after(self.field);
                }
            }
            Field::HeaderCrc { first: None } => self.field = Field::HeaderCrc { first: Some(byte) },
            Field::HeaderCrc { first: Some(first) } => {
                // The low half of the CRC-32 of every header byte before it.
                if u16::from_le_bytes([first, byte]) != self.crc as u16 {
                    return Err(self.refused("header CRC does not match"));
                }
                self.field = Field::Done;
            }
            Field::Done => unreachable!("a finished header takes no byte"),
        }
        Ok(())
    }

    /// The field after `field`, skipping the optional ones the flags leave out.
    fn after(&self, field: Field) -> Field {
        let flags = self.head[3];
        let order = [
            (Field::ExtraLength, FEXTRA),
            (Field::Name, FNAME),
            (Field::Comment, FCOMMENT),
            (Field::HeaderCrc { first: None }, FHCRC),
        ];
        let from = match field {
            Field::Fixed => 0,
            Field::Extra { .. } => 1,
            Field::Name => 2,
            Field::Comment => 3,
            _ => unreachable!("only a field that ends moves on"),
        };
        order[from..]
            .iter()
            .find(|&&(_, flag)| flags & flag != 0)
            .map_or(Field::Done, |&(next, _)| next)
    }

    fn refused(&self, reason: &'static str) -> Error {
        Error::Header { offset: self.offset, reason }
    }
}

/// A member's trailer: the CRC-32 of its plaintext, and that plaintext's
/// length modulo 2³².
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Trailer {
    pub(crate) crc: u32,
    pub(crate) isize: u32,
}

impl Trailer {
    pub(crate) const LEN: usize = 8;

    pub(crate) fn parse(bytes: &[u8; Self::LEN]) -> Trailer {
        Trailer {
            crc: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            isize: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        }
    }
}

#[cfg(test)]
mod tests;
