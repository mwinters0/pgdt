//! **What the cache persists, and nothing else**: the serde shapes a
//! [`crate::DumpIndex`] is saved and loaded as — the spans and their blocks,
//! the preamble's declarations, a block's census and statistics, and the
//! TimescaleDB catalog rows it records — all but a block's header,
//! `pgdt_core::CopyHeader`, which the scanner parses below this crate and a
//! block holds as it is. **An edit here, or to that header, is the edit that
//! asks whether [`crate::CACHE_FORMAT_VERSION`] bumps**, though not the only
//! one: a new way of choosing what an unchanged field holds asks it too,
//! wherever that rule lives, as the constant's doc says.
//!
//! What reads, builds or concludes from these shapes sits beside this tree,
//! an impl on a persisted type included: the walks over the catalog, the
//! contracts over a block's rows, the diagnostics and TimescaleDB's recipe.
//! A persisted type's field is private to the crate where its meaning is read
//! only through a method beside it.

pub(crate) mod block;
pub(crate) mod catalog;
pub(crate) mod span;
pub(crate) mod statistics;
pub(crate) mod timescaledb;
