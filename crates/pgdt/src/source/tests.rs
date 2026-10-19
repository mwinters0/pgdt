use super::*;
use pgdt_core::Unusable;
use std::path::PathBuf;

/// **The source's name is the CLI's to add.** The library detects a
/// source moving underneath a run wherever the check is cheapest and has
/// no name for what it was reading; every other error keeps its own
/// words. Which is which is an exhaustive match, so the compiler is what
/// pins the coverage and this test pins the rendering — one case per
/// refusal that fires *because the source moved*, since each is raised by
/// a different mechanism and names a different thing by itself.
#[test]
fn only_the_refusals_about_a_source_are_given_its_name() {
    let origin = Origin::local("/tmp/koji.dump");
    let named = |err: pgdt_core::Error| naming_the_source(err, &origin).to_string();

    let said = named(pgdt_core::Error::SourceChangedWhileRead {
        differences: "its modification time moved".into(),
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    assert!(said.contains("while it was being read"), "{said}");

    // The cache names itself; what it cannot name is the dump it was
    // checked against, which is the whole point where the path was
    // derived from a URL.
    let said = named(pgdt_core::Error::CacheUnusable {
        path: PathBuf::from("koji.dump.dtcache"),
        unusable: Unusable::SourceChanged { cached_stored_size: 10, live_stored_size: 11 },
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    assert!(said.contains("koji.dump.dtcache"), "the cache is still named: {said}");

    // A bound weak signal that does not hold: the same derived cache
    // path, and a clause about times rather than sizes.
    let said = named(pgdt_core::Error::StrictIdentityUnmet {
        path: PathBuf::from("koji.dump.dtcache"),
        term: "time",
        unmet: "the modification time has moved".into(),
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    assert!(said.contains("koji.dump.dtcache"), "the cache is still named: {said}");

    // Both arms of the block refusal, the cache-less one naming nothing
    // at all by itself — the case with most to gain.
    let said = named(pgdt_core::Error::CachedBlockChanged {
        path: Some(PathBuf::from("koji.dump.dtcache")),
        header_offset: 4096,
    });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");
    let said = named(pgdt_core::Error::CachedBlockChanged { path: None, header_offset: 4096 });
    assert!(said.starts_with("/tmp/koji.dump: "), "the source leads the sentence: {said}");

    let other = pgdt_core::Error::CacheDisabled { operation: "parse" };
    let verbatim = other.to_string();
    assert_eq!(named(other), verbatim);
}
