//! On-disk structure cache: a serialized [`DumpIndex`](pgdt_model::DumpIndex), colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/decisions.md`, "The compressed source and the cache").
//!
//! **A cache that cannot be used is refused, not scanned over**
//! (`docs/design/decisions.md`, "D20"): one that is another file's, another
//! build's, or damaged is almost always a user's accident — the wrong path, or
//! a file that changed — so an open and the two query entry points answer
//! [`Error::CacheUnusable`](pgdt_core::Error::CacheUnusable) before the dump
//! is read past its magic, naming which of those it is ([`Unusable`]). [`CacheMode::with_overwrite_unusable`] opts into
//! starting cold and replacing it instead, for a source whose file is replaced
//! under a stable name; a file that is not a pgdt cache at all is refused
//! whatever is asked. Only a missing cache starts cold unasked. Writing is not
//! best-effort either: [`CacheMode::save`] propagates I/O failures rather than
//! silently degrading every future run of the same command back to a full
//! scan.
//!
//! **The dump file's identity is checked, not assumed**
//! (`docs/design/decisions.md`, "D21"). Every cache records the source's
//! *stored* size and modification signal as observed at save time — bytes on
//! the device, not the addressable (possibly decompressed) length — and, for a
//! source that was fetched from somewhere, **where it came from** and the
//! server's entity tag
//! (`docs/design/decisions.md`, "D87"); a load
//! re-observes the live source and compares. A stored-size mismatch means
//! every byte offset in the cache could be wrong, so the cache is unusable.
//! Neither weak signal invalidates it by default: they are surfaced on
//! [`LoadedCache`] as a [`WeakIdentity`] and an [`OriginMatch`], and a load
//! through [`CacheMode`] turns each into a warning on the loaded index,
//! recomputed on every load and never persisted. A caller that asked for
//! [`StrictIdentity::time`] or [`StrictIdentity::location`] is refused
//! instead.
//!
//! **Two different questions hide under one word.** *Between* runs a moved,
//! copied or touched dump is a different weak identity holding the same
//! bytes, which is what the advisory default is right for. *During* a run a
//! source whose identity changes is bytes moving underneath a read that has
//! already returned some of them, which no answer survives — so that is an
//! error under every selection but `none`, which only warns, and
//! [`SourceWatch`] is the mechanism.
//!
//! **A caller opens a source through its cache** ([`CacheMode::open`]),
//! every step of that protocol in one call; a caller with no source reads the
//! cache alone ([`OfflineCache`]).
//!
//! The comparisons, and which signals bind, are [`identity`]'s; the file's
//! format and what a read of it finds, [`file`]'s; a cache read against its
//! source and written beside it, [`store`]'s; how a caller asks for the
//! cache handled, [`mode`]'s; and the open, [`open`]'s.

pub(crate) mod file;
pub(crate) mod identity;
mod mode;
mod open;
pub(crate) mod store;

pub use self::file::{CacheEnvelope, CacheStatus, CompressionShape, LoadedCache};
pub use self::identity::{OriginMatch, SourceWatch, StrictIdentity, WeakIdentity};
pub use self::mode::{CacheLoad, CacheMode, OfflineCache};
pub use self::open::{Held, Opened};
pub use self::store::{CacheClaim, claim, colocated_path};
pub use pgdt_core::Unusable;
pub use pgdt_core::internal::OVERWRITE_WAYS_OUT;
