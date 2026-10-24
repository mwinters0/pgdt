//! A deflate decoder of this crate's own, for the one decode the zlib API
//! cannot make: from a block boundary found in the middle of a member, with
//! the 32 KiB of history before it unknown.
//!
//! **What it is for is speculation, never the plaintext's authority.** A
//! parallel first pass ([`Parallel`](crate::Parallel)) decodes each chunk of
//! the file from a block boundary it guessed, before the chunk ahead of it is
//! decoded, so a back-reference into the history before the boundary cannot
//! be resolved yet: this decoder writes a **marker** in its place — a 16-bit
//! value naming which byte of that history it copies — and the markers are
//! resolved once the history is known. Once 32 KiB have been written with no
//! marker among them nothing later can reach one, and the decode goes on in
//! bytes. A chunk whose guess was wrong, or whose decode or resolution fails
//! anywhere, is discarded and its bytes are decoded through the zlib API, so
//! every refusal a caller sees is the engine's.
//!
//! **It refuses what zlib refuses**: the same over-subscribed and incomplete
//! codes, the same missing end-of-block code, the same invalid symbols and
//! distances, so a guess it accepts is a stream the engine accepts. What it
//! hands out is checked as the engine's is, by the CRC-32s every unit and
//! member carries (`docs/design/decisions.md`, "D117").

mod huffman;
mod inflate;
mod input;
mod sink;

pub(crate) use self::inflate::{Inflater, Stop};
pub(crate) use self::input::Input;
pub(crate) use self::sink::{MARKER, Marked, Plain, Sink};

#[cfg(test)]
mod tests;
