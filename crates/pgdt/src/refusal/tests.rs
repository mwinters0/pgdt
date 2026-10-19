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
    let named = |err: pgdt_core::Error| refusal(err, &origin).to_string();

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

/// **Each refusal still names its flag.** The library names a remedy in its
/// own terms (`docs/design/decisions.md`, "D107"), so the flag a `pgdt` user
/// types is this layer's to add — one case per refusal whose remedy `pgdt`
/// has a flag or a command for, each said as it leaves a command: through
/// [`refusal`] where it names the source, and otherwise up by `?` to
/// [`spelled_chain`], context and all.
#[test]
fn each_refusal_names_the_flag_its_remedy_is() {
    let origin = Origin::local("/tmp/koji.dump");
    let cache = || PathBuf::from("koji.dump.dtcache");
    let said = |err: pgdt_core::Error| {
        if about_the_source(&err) {
            spelled_chain(refusal(err, &origin)).to_string()
        } else {
            let said = spelled_chain(anyhow::Error::from(err).context("outer"));
            let mut chain = said.chain();
            assert_eq!(chain.next().unwrap().to_string(), "outer", "the context is kept");
            chain.next().unwrap().to_string()
        }
    };
    let cases: Vec<(pgdt_core::Error, &[&str])> = vec![
        (pgdt_core::Error::CacheDisabled { operation: "info" }, &["`--dtcache none`"]),
        (
            pgdt_core::Error::SourceChangedWhileRead { differences: "its size moved".into() },
            &["`--strict-identity=none`"],
        ),
        (
            pgdt_core::Error::SourceUncheckable { why: "no tag".into() },
            &["`--strict-identity=none`"],
        ),
        (
            pgdt_core::Error::StrictIdentityUnmet {
                path: cache(),
                term: "time",
                unmet: "the modification time has moved".into(),
            },
            &["`--strict-identity` without `time`", "`pgdt parse`"],
        ),
        (
            pgdt_core::Error::CacheUnusable { path: cache(), unusable: Unusable::Unreadable },
            &["`--overwrite-unusable-cache`", "`--dtcache <path>`"],
        ),
        (
            pgdt_core::Error::CacheUnusable { path: cache(), unusable: Unusable::NotACache },
            &["`--dtcache <path>`"],
        ),
        (
            pgdt_core::Error::CachedBlockChanged { path: Some(cache()), header_offset: 4096 },
            &["`pgdt parse`"],
        ),
        (
            pgdt_core::Error::AmbiguousTable {
                name: "t".into(),
                candidates: vec!["a.public.t".into(), "b.public.t".into()],
            },
            &["`--database`"],
        ),
        (
            pgdt_core::Error::MetadataNotScanned { database: Some("shop".into()) },
            &["`pgdt parse`", "`--schema-mode strings`"],
        ),
        (
            pgdt_core::Error::FieldDecode {
                table: "public.t".into(),
                column: "v".into(),
                line_offset: 7,
                declared_type: "integer".into(),
                value: "x".into(),
            },
            &["`--schema-mode strings`"],
        ),
        (
            pgdt_core::Error::TableAtMetadataLevel { table: "public.t".into() },
            &["`pgdt parse --statistics-level`", "`--schema-mode strings`"],
        ),
        (
            pgdt_core::Error::Unrepresentable {
                table: "public.t".into(),
                column: "d".into(),
                declared_type: "date".into(),
                values: 1,
            },
            &["`--unrepresentable null`", "`--unrepresentable text`"],
        ),
        (
            pgdt_core::Error::UnrepresentableTestUntyped {
                column: "d".into(),
                op: "IS UNREPRESENTABLE",
            },
            &["`--schema-mode typed`"],
        ),
    ];
    for (err, flags) in cases {
        let library = err.to_string();
        assert!(!library.contains("--"), "the library names no flag: {library}");
        let said = said(err);
        assert!(said.contains(&library), "the library's sentence is kept whole: {said}");
        for flag in flags {
            assert!(said.contains(flag), "{flag}: {said}");
        }
    }
}

/// An error with nothing to spell leaves a command exactly as it was raised,
/// its type included.
#[test]
fn a_chain_with_nothing_to_spell_is_left_alone() {
    let err = anyhow::Error::from(pgdt_core::Error::ResumeQueryMismatch).context("outer");
    let said = spelled_chain(err);
    assert!(said.downcast_ref::<pgdt_core::Error>().is_some(), "{said:?}");
}
