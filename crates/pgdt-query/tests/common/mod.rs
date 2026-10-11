//! The fixture vocabulary every integration test speaks, `pgdt-fixtures`, and
//! what reading a batch back adds to it.
//!
//! Everything here is a *path* or an *expectation*. Anything that drives the
//! library — a `collect`, a `drain`, a `census_of` — stays in the file whose
//! subject it is.

// Each test binary compiles the whole module and uses a subset of it, so
// per-binary dead-code warnings are structural rather than informative.
#![allow(dead_code)]

use arrow::array::RecordBatch;
use pgdt_codec::pgtype::NestedPlan;
use pgdt_query::render_field;

#[allow(unused_imports)]
pub use pgdt_fixtures::*;

/// Every column of `batch` rendered back to PostgreSQL text via
/// [`render_field`] — which works regardless of `SchemaMode`: a hand-written
/// fixture with no DDL (like [`edge_cases`]) resolves every column
/// `Utf8View` either way, so this reads identically to a hardcoded
/// `StringViewArray` downcast there, and it also handles a real `pg_dump`
/// fixture's typed columns (`docs/design/decisions.md`, "D66":
/// render-back is exactly this build's own decode/render round trip).
///
/// The plan is `Scalar` for every column, which is right for every
/// non-nested type. A test reading a nested column wants the stream's own
/// `ResolvedSchema::plans` instead, and asks for them where it queries.
pub fn rows_of(batch: &RecordBatch) -> Vec<Vec<Option<String>>> {
    (0..batch.num_rows())
        .map(|row| {
            batch
                .columns()
                .iter()
                .map(|c| {
                    render_field(c.as_ref(), row, &NestedPlan::Scalar)
                        .expect("every fixture value renders back")
                })
                .collect()
        })
        .collect()
}

/// `public.widgets` from the hand-written [`edge_cases`] dump, as decoded
/// rows. Every value here is chosen to break a naive scanner or decoder: an
/// embedded newline and tab, a literal backslash, a `COPY`-like phrase inside
/// a field, an empty string next to a NULL, and a bare carriage return.
pub fn widgets_expected() -> Vec<Vec<Option<String>>> {
    vec![
        vec![
            Some("1".into()),
            Some("alpha".into()),
            Some("a simple widget".into()),
            Some("2024-01-01 00:00:00+00".into()),
        ],
        vec![Some("2".into()), Some("beta".into()), None, Some("2024-01-02 00:00:00+00".into())],
        vec![
            Some("3".into()),
            Some("gamma".into()),
            Some("multi\nline\twith a backslash \\ inside".into()),
            None,
        ],
        vec![
            Some("4".into()),
            Some("delta".into()),
            Some(
                "contains a COPY-like phrase: COPY public.widgets (id) FROM stdin; -- not a directive"
                    .into(),
            ),
            Some("2024-01-04 00:00:00+00".into()),
        ],
        vec![
            Some("5".into()),
            Some("".into()),
            Some("empty name to the left".into()),
            Some("2024-01-05 00:00:00+00".into()),
        ],
        vec![
            Some("6".into()),
            Some("epsilon".into()),
            Some("carriage\rreturn, octal A, hex B".into()),
            Some("2024-01-06 00:00:00+00".into()),
        ],
    ]
}
