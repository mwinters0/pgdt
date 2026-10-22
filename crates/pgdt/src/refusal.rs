//! A library refusal as `pgdt` says it: led by the source's name where it is
//! about which file was read, and closed by `pgdt`'s spelling of the remedy
//! the library names in its own terms (`docs/design/decisions.md`, "D107").

use pgdt_io::Origin;

/// Whether a refusal is about **which file was read** — the classification
/// [`refusal`] renders and `query`'s own reporter shares, so one answer serves
/// every command.
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
        | Lib::Codec { .. }
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
        | Lib::TestNeedsDeclaredType { .. }
        | Lib::FieldRender { .. } => false,
    }
}

/// **`pgdt`'s spelling of the remedy a refusal names in the library's
/// terms**: the flag or command for each remedy `pgdt` has one for, in the
/// order the library's sentence names them, or `None` where it has none. The
/// library's sentence ends with its remedies, so the spelling is appended in
/// parentheses after it ([`spelled`]).
///
/// **Exhaustive, as [`about_the_source`] is**: whether a refusal's remedy
/// has a flag is a property of each variant, so a variant added to
/// [`pgdt_core::Error`] does not compile until it has been spelled or said to
/// need none.
pub(crate) fn spelling(err: &pgdt_core::Error) -> Option<String> {
    use pgdt_core::Error as Lib;
    let spelled = match err {
        Lib::CacheDisabled { .. } => "`--dtcache <path>`, not `--dtcache none`".to_string(),
        Lib::SourceChangedWhileRead { .. } | Lib::SourceUncheckable { .. } => {
            "`--strict-identity=none`".to_string()
        }
        Lib::StrictIdentityUnmet { term, .. } => {
            format!("`pgdt parse`; `--strict-identity` without `{term}`")
        }
        // A file that is not a pgdt cache names another path alone, no flag
        // replacing it (`docs/design/decisions.md`, "D20").
        Lib::CacheUnusable { unusable, .. } if unusable.overwritable() => {
            "`--dtcache <path>`; `--overwrite-unusable-cache`".to_string()
        }
        Lib::CacheUnusable { .. } => "`--dtcache <path>`".to_string(),
        Lib::CachedBlockChanged { path: Some(_), .. } => "`pgdt parse`".to_string(),
        Lib::AmbiguousTable { .. } => "`--database`".to_string(),
        Lib::MetadataNotScanned { .. } => "`pgdt parse`; `--schema-mode strings`".to_string(),
        Lib::FieldDecode { .. } => "`--schema-mode strings`".to_string(),
        Lib::TableAtMetadataLevel { .. } => {
            "`pgdt parse --statistics-level` naming it `data`; `--schema-mode strings`".to_string()
        }
        Lib::Unrepresentable { .. } => {
            "`--unrepresentable null`; `--unrepresentable text`".to_string()
        }
        Lib::TestNeedsDeclaredType { .. } => "`--schema-mode typed`".to_string(),
        // The rest of an enum's labels are in its declaration, which `info
        // --detail` prints.
        Lib::PredicateValueDecode { unlisted, .. } if *unlisted > 0 => {
            "`pgdt info --detail`".to_string()
        }
        // No remedy, or none `pgdt` has a flag for: the condition alone, a
        // remedy that is the user's own act, or one the sentence states
        // beside the request it refuses.
        Lib::CachedBlockChanged { path: None, .. }
        | Lib::Io(_)
        | Lib::Join(_)
        | Lib::UnterminatedCopyBlock { .. }
        | Lib::UnterminatedLargeObjectRegion { .. }
        | Lib::LineTooLong { .. }
        | Lib::ScanCancelled { .. }
        | Lib::InvalidUtf8 { .. }
        | Lib::ColumnCountMismatch { .. }
        | Lib::Arrow(_)
        | Lib::SourceNotReadable { .. }
        | Lib::Remote { .. }
        | Lib::Codec { .. }
        | Lib::CacheEncode(_)
        | Lib::UnknownPredicateColumn { .. }
        | Lib::UnorderedPredicateColumn { .. }
        | Lib::UncomparablePredicateColumn { .. }
        | Lib::PredicateValueDecode { .. }
        | Lib::UnknownProjectionColumn { .. }
        | Lib::DuplicateProjectionColumn { .. }
        | Lib::ResumeQueryMismatch
        | Lib::TableColumnsDisagree { .. }
        | Lib::MapIncomplete { .. }
        | Lib::FieldRefused { .. }
        | Lib::RowEndingRefused { .. }
        | Lib::NullRefused { .. }
        | Lib::FieldRefusedRecorded { .. }
        | Lib::FieldRender { .. } => return None,
    };
    Some(spelled)
}

/// The library's sentence, closed by [`spelling`] where `pgdt` has one.
pub(crate) fn spelled(err: &pgdt_core::Error) -> String {
    match spelling(err) {
        Some(spelling) => format!("{err} ({spelling})"),
        None => err.to_string(),
    }
}

/// A refusal as `pgdt` reports it: led by the name the user typed where
/// [`about_the_source`] says it wants one, and [`spelled`]. Every other
/// error keeps its type, which [`spelled_chain`] spells on the way out.
pub(crate) fn refusal(err: pgdt_core::Error, source: &Origin) -> anyhow::Error {
    if about_the_source(&err) {
        return anyhow::anyhow!("{source}: {}", spelled(&err));
    }
    err.into()
}

/// **Every library refusal on the way out of a command, spelled**: each
/// error in `err`'s chain that is a [`pgdt_core::Error`] is said as
/// [`spelled`] says it, whatever context `pgdt` wrapped it in, and the chain
/// is otherwise said as it was. Where nothing in it has a spelling, `err` is
/// returned as it is.
///
/// This is what reaches a refusal that went up by `?` rather than through
/// [`refusal`], so no call site has to remember to spell one.
pub(crate) fn spelled_chain(err: anyhow::Error) -> anyhow::Error {
    if !err.chain().any(|cause| library(cause).is_some()) {
        return err;
    }
    let mut sentences: Vec<String> = err
        .chain()
        .map(|cause| library(cause).map_or_else(|| cause.to_string(), spelled))
        .collect();
    let root = sentences.pop().expect("an error's chain holds the error itself");
    sentences.into_iter().rev().fold(anyhow::anyhow!(root), |rebuilt, outer| rebuilt.context(outer))
}

/// `cause` as the library refusal it is, where `pgdt` spells one.
fn library<'a>(cause: &'a (dyn std::error::Error + 'static)) -> Option<&'a pgdt_core::Error> {
    cause.downcast_ref::<pgdt_core::Error>().filter(|lib| spelling(lib).is_some())
}

#[cfg(test)]
mod tests;
