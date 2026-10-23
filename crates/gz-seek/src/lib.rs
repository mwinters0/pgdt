//! Random access into gzip files: decode from a checkpoint without decoding
//! from the file's start.
//!
//! A gzip file is one or more members, each a header, a deflate stream and a
//! trailer checking it. A deflate stream can be resumed at any of its block
//! boundaries given the bit it begins at and the 32 KiB of plaintext before it,
//! so an index of such places — [`Index`], a list of [`Checkpoint`]s — makes
//! every span between two of them, a **unit**, decodable on its own.
//!
//! **Two ways to an index.** A single member states nothing about its own
//! interior, so its index is a by-product of decoding it once: [`Forward`] is
//! that pass, placing a checkpoint at the first block boundary at or after each
//! multiple of a spacing the caller chooses, and at every member start, and
//! handing each checkpoint's window out as it goes. A **BGZF** file states every
//! member's length in its header, so [`Walk`] reads its index off the headers
//! and trailers without decoding a byte, coalescing members into units of at
//! least the spacing.
//!
//! **Nothing here reads a file.** Every type is driven by bytes the caller
//! supplies — [`Forward::read`] and [`UnitRead::read`] take the next compressed
//! bytes and space for plaintext, [`Walk`] hands out a [`Request`] and takes
//! the bytes back — and none starts a thread, holds a borrow across calls, or
//! has a lifetime, so a caller decodes on whatever thread and over whatever
//! transport it has.
//!
//! **Every byte handed out is checked.** The forward pass compares each
//! member's CRC-32 and `ISIZE` and records each unit's own CRC-32 as it goes; a
//! unit decode compares the CRC-32 its index recorded, and the trailer of every
//! member it holds whole, before it reports itself finished; and a unit that
//! ends anywhere but where its index says is refused.
//!
//! **The plaintext's size is open until a pass reaches the end** ([`Size`]): a
//! single member's length modulo 2³² is all its trailer says, so only a decode
//! learns it.
//!
//! ```
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::io::Write;
//! use std::num::NonZeroU64;
//!
//! let plaintext: Vec<u8> = (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect();
//! let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
//! encoder.write_all(&plaintext)?;
//! let file = encoder.finish()?;
//!
//! // The first pass: the plaintext, the index, and each checkpoint's window.
//! let mut pass = gz_seek::Forward::new(NonZeroU64::new(128 * 1024).unwrap())?;
//! let (mut taken, mut out, mut windows) = (0, vec![0u8; 64 * 1024], Vec::new());
//! loop {
//!     let step = pass.read(&file[taken..], &mut out)?;
//!     taken += step.progress.consumed;
//!     if step.checkpoint.is_some() {
//!         windows.push(pass.window().to_vec());
//!     }
//!     if step.progress == Default::default() && step.checkpoint.is_none() {
//!         break;
//!     }
//! }
//! let index = pass.finish()?;
//!
//! // Any unit, decoded alone.
//! let unit = index.unit(index.unit_count() - 1).unwrap();
//! let range = unit.compressed_range();
//! let mut decoded = vec![0u8; (unit.plaintext_range().end - unit.plaintext_range().start) as usize];
//! unit.decode(&windows[unit.unit()], &file[range.start as usize..range.end as usize], &mut decoded)?;
//! assert_eq!(decoded, plaintext[unit.plaintext_range().start as usize..]);
//! # Ok(())
//! # }
//! ```
//!
//! The inflater is reached through the zlib API in one module that holds every
//! `unsafe` this crate has; the API above is safe. The engine behind it is a
//! feature's — zlib-rs by default, the one that ships, zlib-ng or stock zlib
//! otherwise (`docs/design/decisions.md`, "D112") — and [`engine`] names the
//! one a build links.
// upstream: UF4
#![deny(unsafe_code)]
#![warn(missing_docs)]

mod bgzf;
mod engine;
mod error;
mod forward;
mod index;
mod member;
#[cfg(test)]
mod test_support;
mod unit;
mod zlib;

pub use bgzf::{BGZF_HEADER_LEN, BGZF_MAX_PLAINTEXT, Request, Walk, WalkStep, bgzf_member_len};
pub use error::{Error, Result};
pub use forward::{Forward, ForwardStep};
pub use index::{Checkpoint, End, Index, Size, Start, UnitTask};
pub use member::GZIP_MAGIC;
pub use unit::{Progress, UnitRead};
pub use zlib::{DECODER_BYTES, WINDOW_SIZE, engine};
