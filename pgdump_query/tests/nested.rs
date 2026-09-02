//! The nested literal codec against real `pg_dump` output — the I20
//! conformance test (`docs/design/architecture.md`, "Decoders and
//! render-back").
//!
//! `nested.rs`'s own unit tests are hand-written literals, which can encode a
//! misreading of the escaping rules twice. This file cannot: it reads every
//! nested column of `fixtures/*/types/default.sql` in `SchemaMode::Strings` —
//! the untyped path, byte-for-byte what `copy::decode_field` produced — and
//! requires `render_*(decode_*(value)) == value` for every one. A force-quote
//! predicate transcribed even slightly wrong from `arrayfuncs.c`/`rowtypes.c`/
//! `rangetypes.c` shows up here as an inequality, on values PostgreSQL itself
//! wrote.

use std::ops::ControlFlow;
use std::path::Path;

use pgdump_query::cache::CacheMode;
use pgdump_query::nested::{
    decode_array, decode_multirange, decode_range, decode_record, render_array, render_multirange,
    render_range, render_record,
};
use pgdump_query::resolve::SchemaMode;
use pgdump_query::{
    LocalFileSource, NestedPlan, QueryOptions, ScanOptions, read_table, render_field,
};

/// Which codec a column's literals belong to. Resolution does not choose this
/// yet — that is 4.4's job — so this file names it per column.
#[derive(Clone, Copy)]
enum Kind {
    Array,
    Record,
    Range,
    Multirange,
}

/// Every nested column in the `types` fixture, by table and column name.
/// `t_multirange` exists only on PG14+ (I10), so it carries a minimum version.
const NESTED_COLUMNS: &[(&str, &str, Kind, u32)] = &[
    ("public.t_array", "v_empty", Kind::Array, 13),
    ("public.t_array", "v_with_null", Kind::Array, 13),
    ("public.t_array", "v_null_array", Kind::Array, 13),
    ("public.t_array", "v_text_special", Kind::Array, 13),
    ("public.t_array", "v_enum_array", Kind::Array, 13),
    ("public.t_array_shape", "v_multidim", Kind::Array, 13),
    ("public.t_array_shape", "v_mixed_dim", Kind::Array, 13),
    ("public.t_array_shape", "v_lbound", Kind::Array, 13),
    ("public.t_composite", "v_point", Kind::Record, 13),
    ("public.t_composite", "v_points", Kind::Array, 13),
    ("public.t_composite", "v_tagged", Kind::Record, 13),
    ("public.t_composite", "v_empty_comp", Kind::Record, 13),
    ("public.t_base_type", "v_mybase_array", Kind::Array, 13),
    ("public.t_nested_array", "v_nested_array", Kind::Array, 13),
    ("public.t_nested_array", "v_arr_holder", Kind::Record, 13),
    ("public.t_nested_array", "v_pointdom", Kind::Record, 13),
    ("public.t_nested_array", "v_pointdom_array", Kind::Array, 13),
    ("public.t_nested_array", "v_boxed_point", Kind::Record, 13),
    ("public.t_nested_array", "v_myrange_array", Kind::Array, 13),
    ("public.t_nested_array", "v_rangedom", Kind::Range, 13),
    ("public.t_range", "v_range", Kind::Range, 13),
    ("public.t_user_range", "v_myrange", Kind::Range, 13),
    ("public.t_text_range", "v_textrange", Kind::Range, 13),
    ("public.t_multirange", "v_int4multirange", Kind::Multirange, 14),
    ("public.t_multirange", "v_myrange_multi", Kind::Multirange, 14),
];

mod common;
use common::types_fixture;

/// One column's non-NULL values, as decoded COPY text. `SchemaMode::Strings`
/// is deliberate: it never consults the DDL, so what comes back is exactly
/// what `copy::decode_field` produced, which is this codec's input.
async fn column_values(path: &Path, table: &str, column: &str) -> Vec<String> {
    let source = LocalFileSource::open(path).unwrap();
    let options = QueryOptions { schema_mode: SchemaMode::Strings, ..Default::default() };
    let mut out = Vec::new();
    read_table(&source, table, &ScanOptions::default(), &options, CacheMode::Disabled, |batch| {
        let index = batch.schema().index_of(column).expect("column is in the COPY header");
        for row in 0..batch.num_rows() {
            // `Strings` mode resolves every column `Utf8View`/`Scalar`,
            // which is the point: what comes back is the codec's input,
            // not something the typed path has already parsed.
            let column = batch.column(index).as_ref();
            if let Some(value) = render_field(column, row, &NestedPlan::Scalar) {
                out.push(value);
            }
        }
        ControlFlow::Continue(())
    })
    .await
    .unwrap();
    out
}

#[track_caller]
fn assert_round_trips(kind: Kind, value: &str, context: &str) {
    let rendered = match kind {
        Kind::Array => {
            render_array(&decode_array(value).unwrap_or_else(|| panic!("{context}: {value}")))
        }
        Kind::Record => {
            render_record(&decode_record(value).unwrap_or_else(|| panic!("{context}: {value}")))
        }
        Kind::Range => {
            render_range(&decode_range(value).unwrap_or_else(|| panic!("{context}: {value}")))
        }
        Kind::Multirange => render_multirange(
            &decode_multirange(value).unwrap_or_else(|| panic!("{context}: {value}")),
        ),
    };
    assert_eq!(rendered, value, "{context}");
}

#[tokio::test]
async fn every_nested_fixture_value_round_trips_byte_for_byte() {
    for version in [13, 14, 15, 16, 17, 18] {
        let path = types_fixture(version, "default");
        let mut seen = 0;
        for (table, column, kind, min_version) in NESTED_COLUMNS {
            if version < *min_version {
                continue;
            }
            let values = column_values(&path, table, column).await;
            assert!(!values.is_empty(), "pg_dump {version}: {table}.{column} has no non-NULL rows");
            for value in &values {
                assert_round_trips(*kind, value, &format!("pg_dump {version}: {table}.{column}"));
            }
            seen += values.len();
        }
        assert!(seen > 30, "pg_dump {version}: only {seen} nested values reached the codec");
    }
}

/// The both-escape-conventions-at-once case, in both nesting orders, end to
/// end: peel one layer, decode the other, and put both back. This is the
/// shape a single-convention decoder passes every other fixture value on and
/// still gets wrong.
#[tokio::test]
async fn a_nested_layer_round_trips_through_the_other_conventions_codec() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");

        // `public.point2d[]` — `array_out` (backslash) around `record_out`
        // (doubling).
        let mut composites_seen = 0;
        for value in column_values(&path, "public.t_composite", "v_points").await {
            let outer = decode_array(&value).unwrap();
            let elements: Vec<Option<String>> = outer
                .elements
                .iter()
                .map(|element| {
                    element.as_ref().map(|text| {
                        let record = decode_record(text).unwrap();
                        assert_eq!(record.fields.len(), 2, "point2d has two fields");
                        composites_seen += 1;
                        render_record(&record)
                    })
                })
                .collect();
            assert_eq!(elements, outer.elements, "pg_dump {version}: v_points");
            assert_eq!(render_array(&outer), value, "pg_dump {version}: v_points");
        }
        assert!(composites_seen >= 3, "pg_dump {version}: {composites_seen} nested composites");

        // `public.tagged` — `record_out` around `array_out`, the other way up.
        let mut arrays_seen = 0;
        for value in column_values(&path, "public.t_composite", "v_tagged").await {
            let outer = decode_record(&value).unwrap();
            let tags = outer.fields[1].as_ref().expect("v_tagged's array field is never NULL");
            let inner = decode_array(tags).unwrap();
            assert_eq!(&render_array(&inner), tags, "pg_dump {version}: v_tagged");
            arrays_seen += 1;
            assert_eq!(render_record(&outer), value, "pg_dump {version}: v_tagged");
        }
        assert!(arrays_seen >= 2, "pg_dump {version}: {arrays_seen} nested arrays");
    }
}

/// `()` is what `record_out` writes for a composite with no fields *and* for
/// a one-field composite holding SQL NULL, so the literal cannot say which it
/// is — this codec answers "one NULL field" because that is all the text
/// supports. The declared field list is the only discriminator, which is why
/// arity is the caller's join and not a check made here.
#[tokio::test]
async fn a_zero_field_composite_is_indistinguishable_from_a_one_field_null() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let values = column_values(&path, "public.t_composite", "v_empty_comp").await;
        assert_eq!(values, vec!["()".to_string(), "()".to_string()], "pg_dump {version}");
        for value in &values {
            let record = decode_record(value).unwrap();
            assert_eq!(record.fields, vec![None], "pg_dump {version}: {value}");
            assert_eq!(render_record(&record), *value, "pg_dump {version}: {value}");
        }
    }
}

/// The delimiter trap (I22), and why no round-trip test can catch it. `box`'s
/// `typdelim` is `;`, a domain over it inherits that, and this codec's
/// separator is a hardcoded `,` — so a two-element array comes apart into
/// seven elements whose commas are then *not* force-quoted, and re-renders
/// byte-for-byte identical. Wrong element boundaries, an exact round trip, no
/// error anywhere: the refusal has to happen at resolution, on the element
/// type after the domain walk, because nothing downstream of it can tell.
#[tokio::test]
async fn the_delimiter_trap_round_trips_while_splitting_on_the_wrong_character() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let values = column_values(&path, "public.t_delimiter", "v_box_domain_array").await;
        assert_eq!(
            values,
            vec!["{(1,1),(0,0);(3,3),(2,2)}".to_string()],
            "pg_dump {version}: two boxes, semicolon-separated"
        );
        let decoded = decode_array(&values[0]).unwrap();
        assert_eq!(decoded.elements.len(), 7, "pg_dump {version}: a comma split of two boxes");
        assert_eq!(render_array(&decoded), values[0], "pg_dump {version}");
    }
}

/// The shapes 4.5's census exists to find, read off real values rather than
/// asserted from the DDL — which cannot see them at all (I21).
#[tokio::test]
async fn the_fixture_carries_the_array_shapes_the_census_will_have_to_report() {
    for version in [13, 16, 18] {
        let path = types_fixture(version, "default");
        let ndims = |values: &[String]| -> Vec<usize> {
            values.iter().map(|v| decode_array(v).unwrap().ndim()).collect()
        };

        let uniform = column_values(&path, "public.t_array_shape", "v_multidim").await;
        assert_eq!(ndims(&uniform), vec![2, 2], "pg_dump {version}: v_multidim");

        let mixed = column_values(&path, "public.t_array_shape", "v_mixed_dim").await;
        assert_eq!(ndims(&mixed), vec![1, 2], "pg_dump {version}: v_mixed_dim");

        let decorated = column_values(&path, "public.t_array_shape", "v_lbound").await;
        assert!(
            decorated.iter().all(|v| decode_array(v).unwrap().is_decorated()),
            "pg_dump {version}: v_lbound"
        );
        assert_eq!(
            decode_array(&decorated[0]).unwrap().lower_bounds,
            vec![0],
            "pg_dump {version}: v_lbound"
        );

        // And the ordinary case they are contrasted against.
        let plain = column_values(&path, "public.t_array", "v_text_special").await;
        assert!(
            plain.iter().all(|v| {
                let a = decode_array(v).unwrap();
                a.ndim() == 1 && !a.is_decorated()
            }),
            "pg_dump {version}: v_text_special"
        );
    }
}

/// The input grammars against the committed comparison oracle: every literal
/// row of `fixtures/<13-18>/oracle/literals.tsv` whose declared type resolves
/// to a nested one, put to `parse_*` and compared with whether the server
/// itself accepted it (`docs/design/architecture.md`, "The comparison
/// oracle").
///
/// **This is what the `*_in` supersets are checked against.** The spec's
/// stated risk runs toward over-acceptance — a literal we take that the server
/// refuses is a divergence in the direction nothing else here permits — and
/// the oracle's malformed rows are the only evidence of where that line is
/// that was not written by the same hand as the parser.
mod oracle {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

    use pgdump_query::cache::CacheMode;
    use pgdump_query::copy::{decode_field, split_fields};
    use pgdump_query::nested::{
        parse_array, parse_multirange, parse_range, parse_record, render_array, render_multirange,
        render_range, render_record,
    };
    use pgdump_query::{
        LocalFileSource, NestedPlan, ScanOptions, TypeDef, TypeOutcome, preamble_only,
        resolve_declared_type,
    };

    /// The majors `scripts/generate_fixtures.py` generates.
    const MAJORS: [u32; 6] = [13, 14, 15, 16, 17, 18];

    /// The oracle's declared types that resolve to a nested plan, asserted as
    /// an exact set so a type that quietly stops being nested — or a new
    /// nested case nobody wired up — fails here rather than passing as one
    /// more skip.
    ///
    /// **`public.intarr[]` is deliberately absent** even though its literals
    /// are array literals: an array whose element is an array (I26) resolves
    /// to `TypeOutcome::NestedArrayElement`, so the column is text and
    /// compares as text (`KD3`). The parser reads its literals perfectly well
    /// — `nested.rs`'s unit tests carry the shape — but nothing will ever ask
    /// it to.
    const NESTED: &[&str] = &[
        "integer[]",
        "text[]",
        "public.mood[]",
        "public.point2d",
        "public.tagged",
        "public.empty_comp",
        "int4range",
        "numrange",
        "daterange",
        "tsrange",
        "tstzrange",
        "public.myrange",
        "public.textrange",
        "int4multirange",
        "public.myrange_multi",
    ];

    /// Accepted rows where this build's re-rendering is *not* the server's
    /// output, as `(type, input)` — the canonicalization 11.10 owns, and
    /// **met**: every entry differs in every major that carries the case, and
    /// every difference is an entry.
    ///
    /// Two mechanisms, and neither is the container grammar:
    ///
    /// - **The element's own type canonicalizes.** `( 1 , a )` keeps both
    ///   fields' blanks (that is `record_in`), and then `int4in` throws the
    ///   first field's away while `textin` keeps the second's. Putting the two
    ///   sides of a comparison into one spelling is per element and needs the
    ///   element type, which this module does not have.
    /// - **A discrete range canonicalizes its bounds.** `int4range`'s
    ///   `[1,10]` is `[1,11)` on the server, through the subtype's successor
    ///   function; `numrange` has none and is absent here for that reason.
    const CANONICALIZED: &[(&str, &str)] = &[
        ("public.point2d", "( 1 , a )"),
        ("int4range", "[1,10]"),
        ("int4range", "(0,10)"),
        ("daterange", "[2020-01-01,2020-01-01]"),
    ];

    /// Rows the server refuses for a reason the *grammar* cannot see, so the
    /// parser accepts them: `[10,1)` is well-formed and its bounds are out of
    /// order, which needs the subtype's comparison. Asserted as an exact set
    /// for the same reason as the two above.
    const SEMANTIC_REFUSALS: &[(&str, &str)] = &[("int4range", "[10,1)")];

    fn fixture(major: u32, rest: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../fixtures/{major}/{rest}"))
    }

    /// Split one COPY TEXT line of a committed oracle file into its decoded
    /// fields — the same L1 decoder that reads a dump, because the server
    /// wrote these files with `COPY ... TO STDOUT`.
    fn rows(path: &Path) -> Vec<Vec<Option<String>>> {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        bytes
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                split_fields(line)
                    .map(|f| decode_field(f).unwrap().map(|v| v.into_owned()))
                    .collect()
            })
            .collect()
    }

    /// Every major's `CREATE TYPE`/`CREATE DOMAIN` list, read from the `types`
    /// fixture that major's oracle database was loaded from.
    async fn types_of(major: u32) -> Vec<TypeDef> {
        let source = LocalFileSource::open(fixture(major, "types/default.sql")).unwrap();
        let (metadata, _) =
            preamble_only(&source, &ScanOptions::default(), &CacheMode::Disabled).await.unwrap();
        metadata.databases.into_iter().next().expect("a dump names a database").types
    }

    /// Which parser a declared type's literals belong to, **read off
    /// resolution** rather than off a hand-kept list — so a case whose type
    /// changes shape is re-classified rather than silently mis-parsed.
    fn parser(declared: &str, types: &[TypeDef]) -> Option<fn(&str) -> Option<String>> {
        let TypeOutcome::Mapped(_, plan) = resolve_declared_type(declared, types) else {
            return None;
        };
        match plan {
            NestedPlan::Scalar => None,
            NestedPlan::Array(_) => Some(|s| parse_array(s).as_ref().map(render_array)),
            NestedPlan::Record(fields) => match fields.len() {
                0 => Some(|s| parse_record(s, 0).as_ref().map(render_record)),
                1 => Some(|s| parse_record(s, 1).as_ref().map(render_record)),
                2 => Some(|s| parse_record(s, 2).as_ref().map(render_record)),
                n => panic!("no oracle composite has {n} fields"),
            },
            NestedPlan::Range(_) => Some(|s| parse_range(s).as_ref().map(render_range)),
            NestedPlan::Multirange(_) => {
                Some(|s| parse_multirange(s).map(|m| render_multirange(&m)))
            }
        }
    }

    #[tokio::test]
    async fn the_input_grammars_accept_exactly_what_the_server_accepted() {
        let mut seen_types: BTreeSet<String> = BTreeSet::new();
        // Keyed by case, valued by the majors it showed up in, so "met" can
        // mean met everywhere rather than met somewhere.
        let mut canonicalized: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut semantic: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut carried: BTreeMap<(String, String), usize> = BTreeMap::new();
        let mut asserted = 0usize;

        for major in MAJORS {
            let types = types_of(major).await;
            for row in rows(&fixture(major, "oracle/literals.tsv")) {
                let declared = row[0].clone().expect("a case names a type");
                let Some(parse) = parser(&declared, &types) else { continue };
                seen_types.insert(declared.clone());
                // A SQL NULL input is the `\N` field, not a literal.
                let Some(input) = row[1].clone() else { continue };
                let status = row[2].as_deref().expect("a case records its status");
                // The type itself does not exist at this major — `int4range`'s
                // multirange companion before v14 — which says nothing about
                // the grammar.
                if status == "E42704" {
                    continue;
                }
                let case = (declared.clone(), input.clone());
                *carried.entry(case.clone()).or_default() += 1;
                asserted += 1;
                let got = parse(&input);
                if status == "ok" {
                    let output = row[3].as_deref().expect("an accepted literal has an output");
                    let got = got.unwrap_or_else(|| {
                        panic!("{major} {declared}: refused {input:?}, which the server took")
                    });
                    if got != output {
                        assert!(
                            CANONICALIZED.contains(&(declared.as_str(), input.as_str())),
                            "{major} {declared}: {input:?} re-renders as {got:?}, not the \
                             server's {output:?}"
                        );
                        *canonicalized.entry(case).or_default() += 1;
                    }
                } else if got.is_some() {
                    assert!(
                        SEMANTIC_REFUSALS.contains(&(declared.as_str(), input.as_str())),
                        "{major} {declared}: accepted {input:?}, which the server refused with \
                         {status}"
                    );
                    *semantic.entry(case).or_default() += 1;
                }
            }
        }

        assert_eq!(
            seen_types,
            NESTED.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
            "the oracle's declared types that resolve to a nested plan"
        );
        for (name, found, expected) in [
            ("canonicalized", &canonicalized, CANONICALIZED),
            ("semantic refusals", &semantic, SEMANTIC_REFUSALS),
        ] {
            let expected: BTreeSet<_> =
                expected.iter().map(|(t, l)| (t.to_string(), l.to_string())).collect();
            let got: BTreeSet<_> = found.keys().cloned().collect();
            assert_eq!(got, expected, "the {name} set");
            // An entry is keyed by the case, so a difference that stopped
            // happening for one major must not leave it satisfied by the rest.
            let partial: Vec<_> =
                expected.iter().filter(|case| found[*case] != carried[*case]).collect();
            assert!(partial.is_empty(), "{name} met in only part of the walk: {partial:?}");
        }
        // A floor, not a count: the walk skips a row for three good reasons,
        // and a bug in any of them would leave it asserting almost nothing
        // while passing. 397 today.
        assert!(asserted > 350, "only {asserted} literals asserted");
    }
}
