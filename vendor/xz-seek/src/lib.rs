//! Random access into `.xz` files: read at any uncompressed byte offset without
//! decoding from zero.
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
//! [`Reader::index_shared`] and an `Arc<SeekTable>` passed back share the
//! reader's own table rather than copying it.
//!
//! **The walk that builds it fetches nothing.** [`Walk`] is the footer parse as
//! a state machine: it hands out a [`Request`] for a range of the compressed
//! file and takes the bytes back, holding no borrow and no lifetime, so a
//! caller whose transport is asynchronous drives it from a future.
//! [`SeekTable::from_source`] is the synchronous driver over it, for a caller
//! who has a file.
//!
//! **Nothing that answers from the table needs a source.** [`Layout`] is the
//! table, the memory limit, the verification state and the backend as one
//! value: the file's shape, the work one block is, what a decode holds and what
//! a range would cost, all of it arithmetic. [`Builder::layout`] builds one
//! from a persisted table and the file's size, [`Reader::layout`] hands out the
//! one a reader that walked is using, and a `Reader` forwards every one of
//! those queries so a caller who has a file need not hold both. The
//! configuration itself is readable there too: [`Layout::verify`] and
//! [`Layout::backend`], off the handle alone.
//!
//! **Blocks are independent, and [`Reader::block_task`] hands one out as work.**
//! A [`BlockTask`] is `Copy` and `Send`, says which compressed bytes the block
//! needs, and decodes the whole of it into a buffer you own — verified before
//! the call returns. Fetch those bytes however you like, wrap them in a
//! [`Window`], and decode on whatever thread you please; nothing on that path
//! starts a thread of its own. A caller whose buffer budget is smaller than a
//! block takes the same block in pieces instead — [`BlockTask::begin`] hands
//! back a [`BlockRead`], which reads forward into a buffer of any size out of
//! the one window and compares the check when the caller completes it.
//!
//! **[`Reader::read_range`] is the other way at that**, for a caller who wants
//! the bytes rather than the scheduling: name a range of the uncompressed
//! stream, however large, and fill your own buffer from the handle until a short
//! return says the range ended. Every byte it delivers had its block's check
//! compared first, and above one worker the blocks are fetched and decoded on
//! threads the handle owns and joins when you drop it. It is bounded by a worker
//! count and a byte budget ([`Bulk`]), whichever binds first, and what those two
//! admit over a given range — the workers, and the bytes they will hold — is
//! [`Reader::plan_range`], answered before a byte is read.
//!
//! **Pre-1.0, and differentially checked.** Every structured `(offset, len)`
//! over the fixture corpus and an exhaustive sweep over the smallest fixture run
//! in two orders against `xz -dc`, the damaged fixtures are asserted against the
//! error taxonomy, and a detached pass has confirmed the whole of it over a
//! 40 GB real file. The design record is `docs/design/roadmap.md` and
//! `docs/design/decisions.md`.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

// A build with no backend decodes nothing, and the first sign of it would
// otherwise be an unresolved name at the block-payload seam. It is a mistake in
// the build configuration, so it is a build error: see
// `docs/design/decisions.md`, "D51".
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
mod layout;
mod plan;
mod pool;
mod range;
mod reader;
mod source;
mod table;
mod task;
mod walk;
mod window;

pub use backend::Backend;
pub use check::implementation as check_implementation;
pub use error::{Error, Result};
pub use layout::Layout;
pub use plan::{Bulk, RangePlan};
pub use range::RangeRead;
pub use reader::{Builder, Reader, Verify};
pub use source::CompressedSource;
pub use table::{BlockEntry, Check, SeekTable, StreamEntry};
pub use task::{BlockRead, BlockTask};
pub use walk::{Request, Step, Walk};
pub use window::Window;
