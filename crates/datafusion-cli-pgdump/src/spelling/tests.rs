use std::path::PathBuf;

use pgdt_core::Unusable;
use pgdt_model::PostgresInvalidValues;

use super::*;

fn pass(postgres_invalid_values: PostgresInvalidValues) -> MappingPass {
    MappingPass { location: "/d/koji.dump".to_string(), cache_path: None, postgres_invalid_values }
}

fn metadata_level(postgres_invalid_values: PostgresInvalidValues) -> Error {
    Error::MetadataLevel {
        refused: Box::new(pgdt_core::Error::TableAtMetadataLevel { table: "public.meta".into() }),
        pass: pass(postgres_invalid_values),
        table: Box::new(TableName {
            database: None,
            schema: Some("public".to_string()),
            table: "meta".to_string(),
        }),
    }
}

/// **Each refusal names its remedy in the grammar that registered its
/// dump**: a `--dump`'s suffix or a statement's `pgdump.` option, and
/// `pgdt`'s own command where the remedy is a parse. Which variants have a
/// spelling is an exhaustive match, so the compiler pins the coverage and
/// this test pins the words — one case per refusal the shell spells, its
/// spelling under each grammar, `None` where that grammar has none.
#[test]
fn each_refusal_is_spelled_in_its_registration_s_grammar() {
    let lib = |err: pgdt_core::Error| Error::Library(err);
    let cache = || PathBuf::from("koji.dump.dtcache");
    let cases: Vec<(Error, Option<&str>, Option<&str>)> = vec![
        (
            Error::CacheNotComplete {
                pass: pass(PostgresInvalidValues::Default),
                why: "there is no cache".into(),
            },
            Some("`pgdt parse --source /d/koji.dump`"),
            Some("`pgdt parse --source /d/koji.dump`"),
        ),
        // Opened to go past what PostgreSQL refuses, the parse goes past
        // them too, where the default parse would fail at the first.
        (
            Error::CacheNotComplete {
                pass: MappingPass {
                    cache_path: Some(cache()),
                    ..pass(PostgresInvalidValues::Ignore)
                },
                why: "there is no cache".into(),
            },
            Some(
                "`pgdt parse --source /d/koji.dump --dtcache koji.dump.dtcache \
                 --postgres-invalid-values ignore`",
            ),
            Some(
                "`pgdt parse --source /d/koji.dump --dtcache koji.dump.dtcache \
                 --postgres-invalid-values ignore`",
            ),
        ),
        (
            metadata_level(PostgresInvalidValues::Default),
            Some(
                "`pgdt parse --source /d/koji.dump --statistics-level metadata,public.meta=data`; \
                 `:strings`",
            ),
            Some(
                "`pgdt parse --source /d/koji.dump --statistics-level metadata,public.meta=data`; \
                 `'pgdump.schema_mode' 'strings'`",
            ),
        ),
        (
            metadata_level(PostgresInvalidValues::Ignore),
            Some(
                "`pgdt parse --source /d/koji.dump --postgres-invalid-values ignore \
                 --statistics-level metadata,public.meta=data`; `:strings`",
            ),
            Some(
                "`pgdt parse --source /d/koji.dump --postgres-invalid-values ignore \
                 --statistics-level metadata,public.meta=data`; \
                 `'pgdump.schema_mode' 'strings'`",
            ),
        ),
        (
            Error::CatalogNameNeeded { origin: "/d/koji.dump".into() },
            Some("`--dump NAME=SOURCE`"),
            None,
        ),
        (
            Error::CatalogNameRefused {
                origin: "/d/all.dump".into(),
                databases: 2,
                name: "shop".into(),
            },
            Some("`--dump SOURCE`, without `NAME=`"),
            None,
        ),
        (Error::DatabaseUnnamed { origin: "/d/all.dump".into() }, None, None),
        (Error::Table("/d/koji.dump holds no table `x`".into()), None, None),
        (
            lib(pgdt_core::Error::SourceChangedWhileRead { differences: "its size moved".into() }),
            Some("`:strict-identity=none`"),
            Some("`'pgdump.strict_identity' 'none'`"),
        ),
        (
            lib(pgdt_core::Error::SourceUncheckable { why: "no entity tag".into() }),
            Some("`:strict-identity=none`"),
            Some("`'pgdump.strict_identity' 'none'`"),
        ),
        (
            lib(pgdt_core::Error::StrictIdentityUnmet {
                path: cache(),
                term: "time",
                unmet: "the modification time moved".into(),
            }),
            Some("`pgdt parse`; `:strict-identity=` without `time`"),
            Some("`pgdt parse`; `'pgdump.strict_identity'` without `time`"),
        ),
        (
            lib(pgdt_core::Error::CacheUnusable {
                path: cache(),
                unusable: Unusable::SourceChanged { cached_stored_size: 10, live_stored_size: 11 },
            }),
            Some("`pgdt parse --overwrite-unusable-cache`"),
            Some("`pgdt parse --overwrite-unusable-cache`"),
        ),
        (
            lib(pgdt_core::Error::CacheUnusable { path: cache(), unusable: Unusable::NotACache }),
            None,
            None,
        ),
        (
            lib(pgdt_core::Error::CachedBlockChanged { path: Some(cache()), header_offset: 4096 }),
            Some("`pgdt parse`"),
            Some("`pgdt parse`"),
        ),
        (
            lib(pgdt_core::Error::MetadataNotScanned { database: Some("shop".into()) }),
            Some("`pgdt parse`; `:strings`"),
            Some("`pgdt parse`; `'pgdump.schema_mode' 'strings'`"),
        ),
        (
            lib(pgdt_core::Error::FieldDecode {
                table: "public.t".into(),
                column: "a".into(),
                line_offset: 7,
                declared_type: "integer".into(),
                value: "abc".into(),
            }),
            Some("`:strings`"),
            Some("`'pgdump.schema_mode' 'strings'`"),
        ),
        (
            lib(pgdt_core::Error::TableAtMetadataLevel { table: "public.meta".into() }),
            Some("`pgdt parse --statistics-level` naming it `data`; `:strings`"),
            Some(
                "`pgdt parse --statistics-level` naming it `data`; \
                 `'pgdump.schema_mode' 'strings'`",
            ),
        ),
        (
            lib(pgdt_core::Error::Unrepresentable {
                table: "public.t".into(),
                column: "d".into(),
                declared_type: "date".into(),
                values: 1,
            }),
            Some("`:unrepresentable=null`; `:unrepresentable=text`"),
            Some("`'pgdump.unrepresentable' 'null'`; `'pgdump.unrepresentable' 'text'`"),
        ),
        (
            lib(pgdt_core::Error::TestNeedsDeclaredType { column: "d".into(), op: "<" }),
            Some("`--dump` without `:strings`"),
            Some("`'pgdump.schema_mode' 'typed'`"),
        ),
        (
            lib(pgdt_core::Error::AmbiguousTable {
                name: "t".into(),
                candidates: vec!["a.t".into(), "b.t".into()],
            }),
            None,
            Some("`'pgdump.schema'`; `'pgdump.database'`"),
        ),
        (
            lib(pgdt_core::Error::PredicateValueDecode {
                column: "mood".into(),
                op: "=",
                value: "meh".into(),
                declared_type: "public.mood".into(),
                accepted: "one of `happy`".into(),
                unlisted: 30,
            }),
            Some("`pgdt info --detail`"),
            Some("`pgdt info --detail`"),
        ),
        (lib(pgdt_core::Error::CacheDisabled { operation: "open" }), None, None),
    ];
    for (refused, dump, statement) in cases {
        assert_eq!(spelling(Grammar::Dump, &refused).as_deref(), dump, "{refused}");
        assert_eq!(spelling(Grammar::Statement, &refused).as_deref(), statement, "{refused}");
        // The library's sentence, and the provider's own, name no flag or
        // command of the shell's.
        let said = refused.to_string();
        for spelled in ["pgdt parse", "--", ":strings", "'pgdump."] {
            assert!(!said.contains(spelled), "{said}");
        }
    }
}

/// **A spelled refusal is not spelled again**, whatever grammar asks.
#[test]
fn a_spelled_refusal_is_not_spelled_again() {
    let spelled = Error::Spelled {
        refused: Box::new(metadata_level(PostgresInvalidValues::Default)),
        spelling: "`pgdt parse`".into(),
    };
    assert_eq!(spelling(Grammar::Dump, &spelled), None);
    assert_eq!(spelling(Grammar::Statement, &spelled), None);
}
