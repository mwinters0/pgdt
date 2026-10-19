//! Naming the source in what a refusal says.

use pgdt_io::Origin;

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

#[cfg(test)]
mod tests;
