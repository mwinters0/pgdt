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

/// Where a header read has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Fixed,
    ExtraLength,
    Extra { left: usize },
    Name,
    Comment,
    HeaderCrc,
    Done,
}

/// One member header, read incrementally.
///
/// Deficiency register: `deficiency: KD150` — it holds every byte it reads,
/// for the header CRC alone, so a name or comment, which no length bounds,
/// is held whole by the engine and by a speculation; and a speculation, whose
/// compressed bound is checked only in deflate data, holds its input from the
/// header's start on beside it. **(b) owned by 15.18.4**.
#[derive(Debug)]
pub(crate) struct Header {
    /// The member's compressed offset, for the errors.
    offset: u64,
    /// Every byte read so far: the header CRC covers all of them before its
    /// own two.
    bytes: Vec<u8>,
    field: Field,
    /// Where the header CRC begins, once the flags say there is one.
    crc_at: usize,
}

impl Header {
    pub(crate) fn new(offset: u64) -> Header {
        Header { offset, bytes: Vec::with_capacity(FIXED_LEN), field: Field::Fixed, crc_at: 0 }
    }

    /// Read from `input` until the header ends or the input does, answering
    /// how many bytes it took.
    pub(crate) fn read(&mut self, input: &[u8]) -> Result<usize> {
        let mut taken = 0;
        while self.field != Field::Done && taken < input.len() {
            self.push(input[taken])?;
            taken += 1;
        }
        Ok(taken)
    }

    pub(crate) fn is_done(&self) -> bool {
        self.field == Field::Done
    }

    /// Every byte of the header read so far: the whole header once
    /// [`Header::is_done`].
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    fn push(&mut self, byte: u8) -> Result<()> {
        self.bytes.push(byte);
        let n = self.bytes.len();
        match self.field {
            Field::Fixed => {
                if n <= GZIP_MAGIC.len() && byte != GZIP_MAGIC[n - 1] {
                    return Err(if n == GZIP_MAGIC.len() {
                        self.refused("compression method is not deflate")
                    } else {
                        Error::NotGzip { offset: self.offset }
                    });
                }
                if n == 4 && byte & RESERVED != 0 {
                    return Err(self.refused("reserved flag bits are set"));
                }
                if n == FIXED_LEN {
                    self.field = self.after(Field::Fixed);
                }
            }
            Field::ExtraLength => {
                if n == FIXED_LEN + 2 {
                    let left = usize::from(u16::from_le_bytes([self.bytes[n - 2], byte]));
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
            Field::HeaderCrc => {
                if n == self.crc_at + 2 {
                    let stored = u16::from_le_bytes([self.bytes[self.crc_at], byte]);
                    // The header CRC is the low half of the CRC-32 of every
                    // header byte before it (RFC 1952, 2.3.1).
                    let computed = crate::zlib::crc32(0, &self.bytes[..self.crc_at]) as u16;
                    if stored != computed {
                        return Err(self.refused("header CRC does not match"));
                    }
                    self.field = Field::Done;
                }
            }
            Field::Done => unreachable!("a finished header takes no byte"),
        }
        Ok(())
    }

    /// The field after `field`, skipping the optional ones the flags leave out.
    fn after(&mut self, field: Field) -> Field {
        let flags = self.bytes[3];
        let order = [
            (Field::ExtraLength, FEXTRA),
            (Field::Name, FNAME),
            (Field::Comment, FCOMMENT),
            (Field::HeaderCrc, FHCRC),
        ];
        let from = match field {
            Field::Fixed => 0,
            Field::Extra { .. } => 1,
            Field::Name => 2,
            Field::Comment => 3,
            _ => unreachable!("only a field that ends moves on"),
        };
        for &(next, flag) in &order[from..] {
            if flags & flag != 0 {
                if next == Field::HeaderCrc {
                    self.crc_at = self.bytes.len();
                }
                return next;
            }
        }
        Field::Done
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
