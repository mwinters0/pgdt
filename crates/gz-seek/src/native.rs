//! A deflate decoder of this crate's own, for the one decode the zlib API
//! cannot make: from a block boundary found in the middle of a member, with
//! the 32 KiB of history before it unknown.
//!
//! **What it is for is speculation.** A
//! parallel first pass ([`Parallel`](crate::Parallel)) decodes each chunk of
//! the file from a block boundary it guessed, before the chunk ahead of it is
//! decoded — a BGZF chunk, read off its members' headers, excepted — so a back-reference into the history before the boundary cannot
//! be resolved yet: this decoder writes a **marker** in its place — a 16-bit
//! value naming which byte of that history it copies — and the markers are
//! resolved once the history is known. Once 32 KiB have been written with no
//! marker among them nothing later can reach one, and the decode goes on in
//! bytes.
//!
//! **Its plaintext is never handed out.** What a speculation keeps is where
//! its chunk begins and stops and the history at the stop, still marked; the
//! pass resolves that into the window the next chunk begins with, and the
//! engine decodes every chunk from it, so a fault here costs a chunk's
//! parallelism, never a byte (`docs/design/decisions.md`, "D117").
//!
//! **It also walks a fixed block** without producing a byte, to find where a
//! decode may resume inside one ([`Walk`]): what a checkpoint inside a fixed
//! block is placed by.
//!
//! **It refuses what zlib refuses**, by zlib's message but where a
//! code-length code holds no code (`huffman.rs`): the same over-subscribed
//! and incomplete codes, the same missing end-of-block code, the same invalid symbols and
//! distances — but a back-reference reaching before a speculation's start,
//! which a marker stands for whatever the member's real history, and only the
//! engine's resume with that history refuses.

mod huffman;
mod inflate;
mod input;
mod sink;
mod walk;

pub(crate) use self::inflate::{Inflater, Stop};
pub(crate) use self::input::Input;
pub(crate) use self::sink::{MARKER, Marked, Plain, Sink};
pub(crate) use self::walk::{Walk, Walked};

#[cfg(test)]
mod tests;
