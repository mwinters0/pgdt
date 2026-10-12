//! Opening the dump against its cache, and naming the source in what a
//! refusal says.

use std::sync::Arc;

use anyhow::Result;
use pgdt_dump::cache::{CacheClaim, CacheMode, Unusable};
use pgdt_io::{KnownCompression, Origin, Recognized, open};

/// Whether a refusal is about **which file was read** — the classification
/// [`naming_the_source`] renders and `query`'s own reporter shares,
/// so one answer serves every command.
///
/// Four refusals fire *because the source moved*, and the library raises each
/// wherever its check is cheapest: in flight, from the cache machinery when a
/// cache cannot be used — written for another file, or damaged or another
/// build's, which is said with the same name — or a bound weak signal does not hold,
/// and from the back-fill when a block no longer ends where the map says
/// (`docs/design/decisions.md`, "D20", "D21"). None of those places has a name
/// for what was being read, and the user typed one.
///
/// **A cache's own name is not the source's.** Three of the four name the
/// cache, which settles nothing where that path was *derived*: a remote dump's
/// default cache is named after the URL's last path segment and sits in the
/// working directory, so "the cache at `koji.dump.dtcache`" does not say which
/// `koji.dump` this run asked for
/// (`docs/design/decisions.md`, "D87"). The fourth, the in-flight refusal,
/// names neither, and nor does the back-fill's against a map with no cache
/// attached.
///
/// **It is an exhaustive match, not a test of the variants somebody
/// remembered.** Whether a refusal wants the source's name is a property of
/// each variant, and arms have been added here by hand slices apart; under
/// `_ => false` the next variant that names no source would inherit that
/// silence with nothing to catch it. Every variant is listed instead, so a
/// variant added to [`pgdt_core::Error`] does not compile until it has been
/// classified.
pub(crate) fn about_the_source(err: &pgdt_core::Error) -> bool {
    use pgdt_core::Error as Lib;
    match err {
        Lib::SourceChangedWhileRead { .. }
        | Lib::SourceUncheckable { .. }
        | Lib::CacheUnusable { .. }
        | Lib::StrictIdentityUnmet { .. }
        | Lib::CachedBlockChanged { .. } => true,
        // A caller's map short of the file's end names neither. `pgdt` hands
        // the library no map of its own, so it never meets this one.
        Lib::MapIncomplete { .. } => true,
        // A caller's map holding the table at the metadata level: `pgdt`'s
        // query maps for itself and reads such a table again, so it never
        // meets this one either.
        Lib::TableAtMetadataLevel { .. } => true,
        // Named by the library itself: each of these carries the origin or
        // the URL in its own sentence.
        Lib::SourceNotReadable { .. } | Lib::Remote { .. } => false,
        // About a place inside the dump, a value in it, or the request made
        // of it — never about which file was read.
        Lib::Io(_)
        | Lib::Join(_)
        | Lib::Arrow(_)
        | Lib::Xz(_)
        | Lib::CacheEncode(_)
        | Lib::UnterminatedCopyBlock { .. }
        | Lib::UnterminatedLargeObjectRegion { .. }
        | Lib::LineTooLong { .. }
        | Lib::ScanCancelled { .. }
        | Lib::InvalidUtf8 { .. }
        | Lib::ColumnCountMismatch { .. }
        | Lib::CacheDisabled { .. }
        | Lib::CacheModeMismatch(_)
        | Lib::UnknownPredicateColumn { .. }
        | Lib::UnorderedPredicateColumn { .. }
        | Lib::UncomparablePredicateColumn { .. }
        | Lib::PredicateValueDecode { .. }
        | Lib::UnknownProjectionColumn { .. }
        | Lib::DuplicateProjectionColumn { .. }
        | Lib::ResumeQueryMismatch
        | Lib::AmbiguousTable { .. }
        | Lib::TableColumnsDisagree { .. }
        | Lib::MetadataNotScanned { .. }
        | Lib::FieldDecode { .. }
        | Lib::FieldRefused { .. }
        | Lib::RowEndingRefused { .. }
        | Lib::NullRefused { .. }
        | Lib::FieldRefusedRecorded { .. }
        | Lib::Unrepresentable { .. }
        | Lib::UnrepresentableTestUntyped { .. }
        | Lib::FieldRender { .. } => false,
    }
}

/// Lead a refusal [`about_the_source`] with the name the user typed, and
/// leave every other error its own words.
pub(crate) fn naming_the_source(err: pgdt_core::Error, source: &Origin) -> anyhow::Error {
    if about_the_source(&err) {
        return anyhow::anyhow!("{source}: {err}");
    }
    err.into()
}

/// What [`open_with_cache`] found: the source to read, or why the cache at
/// the mode's path cannot be used, settled before a stream-footer walk.
/// **What to do about the second is the caller's**: `parse` and `query` ask
/// `CacheMode::refusal`, and `info`, which never writes, prints its own
/// sentence (`docs/design/decisions.md`, "D20").
pub(crate) enum Opened {
    /// The source, ready to read, and the heap the cache's statistics will
    /// hold once a scan loads them (`CacheClaim::Settles`).
    Source { source: Arc<dyn pgdt_io::ByteRangeSource>, cached_statistics: u64 },
    /// The reason `cache::load` would answer once a source existed — or, for
    /// a compression claim recognition refused, the reason it answers for a
    /// source opened without one.
    Unusable(Unusable),
}

/// Open `file`, handing recognition whatever the cache at `cache` says about
/// its compression layer, so an `.xz` source is built from the seek table a
/// previous walk already produced instead of re-walking the file's stream
/// footers (`docs/design/decisions.md`, "D18"). `--dtcache none` claims
/// nothing; cache-only mode never reaches here at all, having no live source.
///
/// An unusable cache stops here rather than at each of the three call sites,
/// so an `.xz` source never walks its stream footers to reach a refusal the
/// cache path alone settles. The comparisons stay in `cache::claim` and in
/// recognition (D20).
pub(crate) async fn open_with_cache(origin: &Origin, cache: &CacheMode) -> Result<Opened> {
    let claimed_by = match cache {
        CacheMode::Enabled { path, .. } => Some(path.as_path()),
        CacheMode::Disabled { .. } | CacheMode::Offline(_) => None,
    };
    let (known, cached_statistics) = match claimed_by {
        Some(path) => match pgdt_dump::cache::claim(path, origin).await? {
            CacheClaim::Settles { compression, statistics_heap_bytes } => {
                (compression, statistics_heap_bytes)
            }
            CacheClaim::Unusable(why) => return Ok(Opened::Unusable(why)),
        },
        None => (KnownCompression::Unknown, 0),
    };
    match open(origin, known).await? {
        Recognized::Source(source) => Ok(Opened::Source { source, cached_statistics }),
        Recognized::Mismatch => Ok(Opened::Unusable(Unusable::CompressionContradicted)),
    }
}

/// [`open_with_cache`] for the two commands that scan: an unusable cache is
/// `CacheMode::refusal`'s to decide — the one decision the four scan entry
/// points make, so this refusal is word for word the one it pre-empts, and the
/// library still refuses on its own: this spares the walk, it does not replace
/// the guarantee (`docs/design/decisions.md`, "D20"). Under
/// `--overwrite-unusable-cache` the source is opened claiming nothing, and the
/// scan's own load meets the same reason and starts cold. Beside the source it
/// answers the heap the cache's statistics will hold, which
/// [`crate::parallel::Discovered::resolve`] carves the workers around — none, for a cache about
/// to be replaced.
pub(crate) async fn open_for_scan(
    origin: &Origin,
    cache: &CacheMode,
) -> Result<(Arc<dyn pgdt_io::ByteRangeSource>, u64)> {
    match open_with_cache(origin, cache).await? {
        Opened::Source { source, cached_statistics } => Ok((source, cached_statistics)),
        Opened::Unusable(why) => match cache.refusal(&why) {
            Some(refusal) => Err(naming_the_source(refusal, origin)),
            None => match open(origin, KnownCompression::Unknown).await? {
                Recognized::Source(source) => Ok((source, 0)),
                Recognized::Mismatch => unreachable!("`Unknown` claims nothing to contradict"),
            },
        },
    }
}

#[cfg(test)]
mod tests;
