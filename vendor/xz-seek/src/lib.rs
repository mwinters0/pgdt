//! A positioned read over an `.xz` file: an uncompressed byte offset in, the
//! bytes there out.
//!
//! **There is no [`std::io::Seek`] implementation, and there will not be.** The
//! primitive here is a positioned read — see [`Reader::read_at`] — because a
//! cursor plus a `Seek` impl is the idiom this crate exists to replace: it
//! turns "give me the bytes at X" into two calls whose meaning depends on
//! everything that came before them. The name says "seek" because that is the
//! verb people search for.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut reader = xz_seek::Reader::new(std::fs::File::open("dump.xz")?)?;
//! let mut buf = [0u8; 4096];
//! let n = reader.read_at(1_000_000_000, &mut buf)?;
//! # let _ = n;
//! # Ok(())
//! # }
//! ```
//!
//! [`Reader::read_at`] fills `buf` completely; a short return means the
//! uncompressed stream ended, and means nothing else.
//!
//! The [`SeekTable`] a reader built is a plain value: [`Reader::index`] hands
//! it out, the off-by-default `serde` feature serializes it, and
//! [`Builder::open_with_table`] takes it back without walking the file again.
//!
//! **Pre-1.0, and differentially checked.** Every structured `(offset, len)`
//! over the fixture corpus and an exhaustive sweep over the smallest fixture run
//! in two orders against `xz -dc`, the damaged fixtures are asserted against the
//! error taxonomy, and a detached pass has confirmed the whole of it over a
//! 40 GB real file. The design record is `docs/design/roadmap.md` and
//! `docs/design/architecture.md`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

// A build with no backend decodes nothing, and the first sign of it would
// otherwise be an unresolved name at the block-payload seam. It is a mistake in
// the build configuration, so it is a build error: see
// `docs/design/architecture.md`, "A build with no backend is a compile error".
#[cfg(not(any(feature = "liblzma", feature = "xz4rust")))]
compile_error!(
    "xz-seek needs a decoder backend: enable the `liblzma` feature (which is the \
     default) or the `xz4rust` feature"
);

mod backend;
mod block;
mod check;
mod decode;
mod error;
mod reader;
mod source;
mod table;
mod walk;

pub use backend::Backend;
pub use check::implementation as check_implementation;
pub use error::{Error, Result};
pub use reader::{Builder, Reader, Verify};
pub use source::CompressedSource;
pub use table::{BlockEntry, Check, SeekTable, StreamEntry};
