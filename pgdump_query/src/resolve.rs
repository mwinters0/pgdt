//! `ResolvedSchema`: the join of a `COPY` header's column list against
//! `DumpMetadata` (`docs/design/architecture.md`, "Joining a header against
//! the metadata").
//!
//! This is what a query's `RecordBatch`es actually carry, not a preview:
//! `crate::batch::RowBatcher` builds one column builder per field of the
//! [`ResolvedSchema`] it was constructed from, and `RecordBatch::try_new`
//! rejects any array whose type disagrees with the schema's. Deciding *how*
//! a field's text becomes a value is `crate::decode`'s and `crate::nested`'s
//! job; this module decides *what* each column is.

use std::sync::Arc;

use arrow::datatypes::{Field, Schema, SchemaRef};

use crate::diagnostic::Severity;
use crate::index::{ArrayShape, MAX_ARRAY_DIMS};
use crate::pgtype::{NestedPlan, TypeOutcome, resolve_declared_type};
use crate::preamble::{DatabaseMetadata, DumpMetadata};

/// Whether a query resolves column types at all. See "Output model" in the
/// phase doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SchemaMode {
    /// Each column gets the narrowest Arrow type this build's mapping table
    /// covers for its declared PostgreSQL type; anything it doesn't cover
    /// stays `Utf8View` with a diagnostic naming why.
    #[default]
    Typed,
    /// Every column is `Utf8View`, byte-for-byte the untyped path, and
    /// no DDL lookup is attempted — the escape hatch for a caller who
    /// distrusts the mapping or wants a schema stable across releases.
    Strings,
}

/// Per-column outcome of resolving one declared type against this build's
/// mapping table — see "Failure and diagnostics" in the phase doc. Distinct
/// from [`crate::pgtype::TypeOutcome`]: this adds the "no DDL explained this
/// column at all" case, which is a join-against-`DumpMetadata` question, not
/// a type-mapping one.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnResolution {
    Mapped,
    /// Declared, but this build's mapping table has nothing for it.
    UnknownType,
    /// No DDL explained this column — `--data-only`, a typed table (`CREATE
    /// TABLE x OF t`), or `SchemaMode::Strings` (which never looks).
    NotDeclared,
    /// The scan never read this block's database's DDL, so nothing is yet
    /// known about *any* of its columns.
    ///
    /// **No mapping scan leaves this pairing any more.** `crate::stream`'s
    /// mapping pass states a database's DDL at that database's first `COPY`
    /// block (I1's recurring boundary), which is strictly before any of its
    /// blocks can be banked, so a block in the map always has its database
    /// covered. What keeps the variant is that [`resolve_columns`] is public
    /// and takes its `metadata` from the caller: an embedder resolving against
    /// an index it assembled itself can still present the condition, and this
    /// is the right answer when it does.
    ///
    /// **Held apart from [`Self::NotDeclared`], which it would otherwise look
    /// exactly like.** `NotDeclared` means the dump never explained this
    /// column and is final; this means "finish the parse and ask again" —
    /// identical-looking output, opposite advice.
    ///
    /// The streaming path refuses this outright
    /// (`crate::Error::MetadataNotScanned`) rather than degrading, because a
    /// stream hands back rows and a wrongly-typed one is a wrong answer. A
    /// *reported* schema cannot refuse: one unresolvable block must not sink
    /// the whole listing, so it reports the reason instead
    /// (`docs/design/architecture.md`, "CLI surface").
    MetadataNotScanned,
    /// An array whose element type is opaque by construction — `box`, a
    /// C-level base type or a shell type, through any chain of domains. Held
    /// apart from [`Self::OpaqueBaseType`] because the column's *own* type is
    /// perfectly well understood; it is the element that is not, and the
    /// array's separator is the element type's (I22).
    OpaqueElementType,
    /// An array column whose element type is itself an array, through any
    /// chain of domains (I26). Held apart from [`Self::OpaqueElementType`]
    /// because the label would lie: `integer[]` is not opaque, it is
    /// understood and declined — opaque means *never improves*, this means
    /// *yes, if anyone needs it*.
    NestedArrayElement,
    /// An array column whose values do not share one Arrow list shape: the
    /// dimensionality differs between rows, or some value carries an
    /// `[lb:ub]=` lower-bound prefix. Both are properties of the *value*
    /// (I21), so only the array-shape census can report them — and it reports
    /// them before the schema commits, which is what makes this a resolution
    /// outcome rather than a decode failure at row 40 million
    /// (`docs/design/architecture.md`, "The array shape census").
    VaryingArrayShape,
    /// A C-level base type or a shell/undefined type.
    OpaqueBaseType,
    EmptyEnum,
}

/// One column's full resolution, named and carrying the raw declared type
/// string (if any DDL named one) alongside the outcome — what a human-facing
/// display (`pgdq info`) needs in one place, for every column, not just the
/// unmapped ones.
///
/// **A note, not a diagnostic.** There is exactly one of these per column,
/// always, and the ordinary case is a column that resolved cleanly — so this
/// is a per-column record, not an exception report. The file-level exception
/// channel is [`crate::diagnostic::Diagnostic`], and the two deliberately
/// stay separate types: `DumpIndex` is L1 while [`ColumnResolution`] is an L2
/// conclusion about PostgreSQL type semantics, so one enum spanning both
/// would have L1 name an L2 type
/// (`docs/design/architecture.md`, "Diagnostics: one severity scale, two types"). What they share is the
/// [`Severity`] scale, so a caller reading both filters uniformly.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnNote {
    pub column: String,
    pub declared: Option<String>,
    pub resolution: ColumnResolution,
}

impl ColumnNote {
    /// Where this column sits on the shared [`Severity`] scale: a column that
    /// resolved to a real Arrow type is `Info`; anything that fell back to
    /// `Utf8View` is a `Warning`, since its values come back unparsed and a
    /// caller may want to know which columns those were.
    ///
    /// Derived rather than stored, for the reason
    /// [`crate::preamble::DumpMetadata`] is a view over spans and
    /// [`crate::index::DumpIndex::blocks`] is a filter rather than a field:
    /// it is a pure function of `resolution`, and a stored copy could
    /// disagree with it. A note whose severity is *not* derivable earns a
    /// field when one exists.
    pub fn severity(&self) -> Severity {
        match self.resolution {
            ColumnResolution::Mapped => Severity::Info,
            _ => Severity::Warning,
        }
    }
}

/// A table query's resolved schema: the Arrow schema a fully-typed decoder
/// would eventually produce (see the module docs for why that's not yet what
/// `RecordBatch`es actually carry), plus one [`ColumnNote`] per column.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSchema {
    pub schema: SchemaRef,
    /// Positional, parallel to `schema.fields()`.
    pub columns: Vec<ColumnResolution>,
    /// Named, one entry per column, in the same order.
    pub notes: Vec<ColumnNote>,
    /// Which PostgreSQL literal form fills each field — positional, parallel
    /// to `schema.fields()` like `columns`.
    ///
    /// **The Arrow type cannot say**: `int4range[]` and `int4multirange`
    /// share one (`crate::pgtype::NestedPlan`), so the plan travels beside it
    /// from resolution — the pair's one producer — into
    /// `crate::batch::RowBatcher` and `crate::batch::render_field`. A scalar
    /// column's entry is `NestedPlan::Scalar`, so every column has one and no
    /// caller has to ask whether this vector applies to it.
    pub plans: Vec<NestedPlan>,
}

impl Default for ResolvedSchema {
    /// The empty schema — what a caller sees before a matching block has
    /// been found (or if the table never appears at all).
    fn default() -> Self {
        Self {
            schema: Arc::new(Schema::empty()),
            columns: Vec::new(),
            notes: Vec::new(),
            plans: Vec::new(),
        }
    }
}

impl ResolvedSchema {
    /// How many columns have no `Mapped` resolution — the count `pgdq info`'s
    /// default summary line reports (`"N of M columns unmapped"`).
    pub fn unmapped_count(&self) -> usize {
        self.columns.iter().filter(|c| **c != ColumnResolution::Mapped).count()
    }
}

/// Find the database named `database` in `metadata.databases` — an exact
/// match on [`crate::preamble::DatabaseMetadata::name`], never a guess.
/// `database: None` matches the single unnamed database a plain (non-`\connect`)
/// dump produces.
///
/// This used to be "the first database whose DDL mentions `qualified_table`"
/// — a guess, since nothing tracked which database a `CopyBlock` actually
/// belonged to. Per-block attribution (`docs/design/architecture.md`,
/// "One target per query") turns it into a fact: the caller already knows,
/// from the block it matched, which database's DDL applies.
fn database_for_name<'a>(
    metadata: &'a DumpMetadata,
    database: Option<&str>,
) -> Option<&'a DatabaseMetadata> {
    metadata.databases.iter().find(|db| db.name.as_deref() == database)
}

/// What the array-shape census says one column's Arrow type should be.
enum ShapeVerdict {
    /// Nothing in the census contradicts what the DDL resolved to.
    Keep,
    /// Every value is this many dimensions deep, and it is more than the one
    /// `List<T>` already commits to.
    Depth(u8),
    /// No single Arrow list type is honest about this column.
    Varying,
}

/// Read one column's census.
///
/// The order of the tests is the load-bearing part. A leading brace run
/// longer than [`MAX_ARRAY_DIMS`] did not come out of `array_out` at all
/// (I25), so it is not *evidence about an array* and must not degrade the
/// column: the file is damaged or hand-edited, and the honest outcome is the
/// optimistic type plus a `FieldDecode` naming the row — the same answer
/// every other value contradicting its declared type gets. Only after that
/// does a lower-bound prefix disqualify the column on its own, however
/// uniform the dimensionality is.
fn shape_verdict(shape: ArrayShape) -> ShapeVerdict {
    if shape.dims.is_some_and(|(_, max)| max > MAX_ARRAY_DIMS) {
        return ShapeVerdict::Keep;
    }
    if shape.lower_bound_prefix {
        return ShapeVerdict::Varying;
    }
    match shape.dims {
        // Every value was SQL NULL or `{}` — both fit any depth, so nothing
        // constrained this column and the optimistic `List<T>` stands.
        None => ShapeVerdict::Keep,
        Some((min, max)) if min != max => ShapeVerdict::Varying,
        Some((1, _)) => ShapeVerdict::Keep,
        Some((depth, _)) => ShapeVerdict::Depth(depth),
    }
}

/// Retype one column's `(DataType, NestedPlan)` pair from its census —
/// **the pair, never a half** (`docs/design/architecture.md`, "Nested
/// columns: `NestedPlan` travels beside the `DataType`"). This is the only
/// place after `resolve_declared_type` where either changes.
///
/// Only a column the DDL resolved to an array is touched. The census is keyed
/// by column and records the shape of the whole field, so it says nothing
/// about an array nested inside a composite or inside another array's element
/// type; leaving those on the optimistic path is what keeps this transform
/// from misreading them.
///
/// **The plan reaching here is always `Array(non-array)`.** The one declared
/// shape that resolves to a nested `Array` — an array whose element type is
/// itself an array, whose literal is one brace deep and whose census therefore
/// reads `(1, 1)` (I26) — is refused at resolution
/// ([`crate::pgtype::TypeOutcome::NestedArrayElement`]), so this function has
/// no such plan to guard against and never needs to ask how the depth it is
/// looking at was arrived at.
fn retype_from_census(
    shape: ArrayShape,
    resolution: ColumnResolution,
    pair: (arrow::datatypes::DataType, NestedPlan),
) -> (ColumnResolution, (arrow::datatypes::DataType, NestedPlan)) {
    use arrow::datatypes::DataType;

    let NestedPlan::Array(_) = &pair.1 else { return (resolution, pair) };
    match shape_verdict(shape) {
        ShapeVerdict::Keep => (resolution, pair),
        ShapeVerdict::Varying => {
            (ColumnResolution::VaryingArrayShape, (DataType::Utf8View, NestedPlan::Scalar))
        }
        ShapeVerdict::Depth(depth) => {
            let (mut data_type, mut plan) = pair;
            // The pair already carries one `List`/`Array` level, so the
            // census's depth adds `depth - 1` more.
            for _ in 1..depth {
                data_type = DataType::List(Arc::new(Field::new("item", data_type, true)));
                plan = NestedPlan::Array(Box::new(plan));
            }
            (resolution, (data_type, plan))
        }
    }
}

/// Resolve `columns` (in `COPY`-header order — placeholder names like
/// `column1` when the header carried none, same as `crate::batch::schema_for`
/// derives) against `metadata` for `qualified_table`, scoped to the
/// database this block was attributed to (`database` — `None` for a plain
/// dump's single unnamed database).
///
/// `census` is the array-shape evidence this schema may commit to, positional
/// like `columns` (`docs/design/architecture.md`, "The array shape census").
/// A caller with no evidence — or none it may believe, which for a *reported*
/// schema means `crate::index::DumpIndex::is_complete` is false — passes
/// `&[]`, and every column keeps the optimistic type the DDL alone gives it.
/// That is the same answer a census of unconstrained shapes produces, so the
/// two need not be told apart.
///
/// A column of a database whose DDL the scan never reached comes back
/// [`ColumnResolution::MetadataNotScanned`] rather than `NotDeclared` — see
/// that variant. This needs `metadata` to be `Some`: a caller with no metadata
/// at all has no DDL for *any* database, which is exactly `NotDeclared`.
///
/// `SchemaMode::Strings` never looks anything up: every column comes back
/// `NotDeclared`/`Utf8View`, matching the untyped path exactly and at zero
/// cost — and with no `NestedPlan::Array` anywhere, the census cannot reach
/// it either.
pub fn resolve_columns(
    qualified_table: &str,
    columns: &[String],
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    mode: SchemaMode,
    census: &[ArrayShape],
) -> ResolvedSchema {
    let db = match mode {
        SchemaMode::Strings => None,
        SchemaMode::Typed => metadata.and_then(|m| database_for_name(m, database)),
    };
    // Metadata exists, but not for *this* block's database, so nothing is
    // known about any column here and saying "not declared" would be a
    // different, final claim. `metadata: None` is left alone: that is a caller
    // with no DDL at all, which is exactly `NotDeclared`.
    let unscanned_database =
        mode == SchemaMode::Typed && metadata.is_some() && !db.is_some_and(|d| d.preamble_complete);
    let declared_cols = db.and_then(|d| d.tables.get(qualified_table));

    let mut fields = Vec::with_capacity(columns.len());
    let mut resolutions = Vec::with_capacity(columns.len());
    let mut notes = Vec::with_capacity(columns.len());
    let mut plans = Vec::with_capacity(columns.len());

    let string = || (arrow::datatypes::DataType::Utf8View, NestedPlan::Scalar);
    for (i, name) in columns.iter().enumerate() {
        let declared = declared_cols.and_then(|cols| cols.iter().find(|(n, _)| n == name));
        let (resolution, pair) = match declared {
            None if unscanned_database => (ColumnResolution::MetadataNotScanned, string()),
            None => (ColumnResolution::NotDeclared, string()),
            Some((_, ty)) => {
                // `db` is always `Some` here: `declared_cols` only came from
                // `db.tables`, so `db.types` is the right list to resolve
                // this same database's `CREATE TYPE`/`DOMAIN` references
                // against.
                match resolve_declared_type(ty, &db.unwrap().types) {
                    TypeOutcome::Mapped(dt, plan) => (ColumnResolution::Mapped, (dt, plan)),
                    TypeOutcome::Unknown => (ColumnResolution::UnknownType, string()),
                    TypeOutcome::OpaqueElementType => {
                        (ColumnResolution::OpaqueElementType, string())
                    }
                    TypeOutcome::NestedArrayElement => {
                        (ColumnResolution::NestedArrayElement, string())
                    }
                    TypeOutcome::OpaqueBaseType => (ColumnResolution::OpaqueBaseType, string()),
                    TypeOutcome::EmptyEnum => (ColumnResolution::EmptyEnum, string()),
                }
            }
        };
        // The census is the only thing that can speak for an array column's
        // shape, and it speaks after the DDL, never instead of it.
        let (resolution, (arrow_type, plan)) =
            retype_from_census(census.get(i).copied().unwrap_or_default(), resolution, pair);
        // Every Arrow field is nullable, regardless of a `NOT
        // NULL` in the DDL -- see "Nullability" in the phase doc.
        fields.push(Field::new(name, arrow_type, true));
        notes.push(ColumnNote {
            column: name.clone(),
            declared: declared.map(|(_, ty)| ty.clone()),
            resolution: resolution.clone(),
        });
        resolutions.push(resolution);
        plans.push(plan);
    }

    ResolvedSchema { schema: Arc::new(Schema::new(fields)), columns: resolutions, notes, plans }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preamble::{DatabaseMetadata, TypeDef, TypeKind};

    fn one_db(tables: &[(&str, &[(&str, &str)])], types: Vec<TypeDef>) -> DumpMetadata {
        let mut db = DatabaseMetadata {
            name: None,
            preamble_complete: true,
            server_version: None,
            pg_dump_version: None,
            extensions: Vec::new(),
            types,
            tables: Default::default(),
        };
        for (name, cols) in tables {
            db.tables.insert(
                name.to_string(),
                cols.iter().map(|(c, t)| (c.to_string(), t.to_string())).collect(),
            );
        }
        DumpMetadata { databases: vec![db] }
    }

    #[test]
    fn mapped_and_not_declared_columns() {
        let meta = one_db(&[("public.t", &[("id", "integer"), ("name", "text")])], vec![]);
        let cols = vec!["id".to_string(), "name".to_string(), "extra".to_string()];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert_eq!(
            resolved.columns,
            [ColumnResolution::Mapped, ColumnResolution::Mapped, ColumnResolution::NotDeclared,]
        );
        assert_eq!(resolved.schema.field(0).data_type(), &arrow::datatypes::DataType::Int32);
        assert_eq!(resolved.schema.field(1).data_type(), &arrow::datatypes::DataType::Utf8View);
        assert_eq!(resolved.schema.field(2).data_type(), &arrow::datatypes::DataType::Utf8View);
        assert!(resolved.schema.fields().iter().all(|f| f.is_nullable()));
        assert_eq!(resolved.unmapped_count(), 1);
    }

    #[test]
    fn strings_mode_never_looks_up_ddl() {
        let meta = one_db(&[("public.t", &[("id", "integer")])], vec![]);
        let cols = vec!["id".to_string()];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Strings, &[]);
        assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
        assert_eq!(resolved.schema.field(0).data_type(), &arrow::datatypes::DataType::Utf8View);
    }

    #[test]
    fn no_metadata_at_all_is_every_column_not_declared() {
        let cols = vec!["id".to_string()];
        let resolved = resolve_columns("public.t", &cols, None, None, SchemaMode::Typed, &[]);
        assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
    }

    /// A block attributed to a database the metadata does not cover is
    /// `MetadataNotScanned`, not `NotDeclared`. The two look identical in the
    /// output and mean opposite things: one is final, the other says "finish
    /// the parse and ask again".
    ///
    /// The contrast that makes it a real distinction is the third case below:
    /// with *no* metadata at all there is no scan to finish, so `NotDeclared`
    /// stands.
    #[test]
    fn a_database_the_scan_never_reached_is_told_apart_from_undeclared_columns() {
        let mut first = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
        first.name = Some("first".to_string());
        let meta = DumpMetadata { databases: vec![first] };
        let cols = vec!["id".to_string()];

        let later =
            resolve_columns("public.t", &cols, Some(&meta), Some("second"), SchemaMode::Typed, &[]);
        assert_eq!(later.columns, [ColumnResolution::MetadataNotScanned]);
        assert_eq!(later.schema.field(0).data_type(), &arrow::datatypes::DataType::Utf8View);
        assert_eq!(later.notes[0].severity(), Severity::Warning);

        // The same metadata, asked about a table the database it *did* read
        // never declared: final, and says so.
        let undeclared = resolve_columns(
            "public.absent",
            &cols,
            Some(&meta),
            Some("first"),
            SchemaMode::Typed,
            &[],
        );
        assert_eq!(undeclared.columns, [ColumnResolution::NotDeclared]);
    }

    /// A database entry that exists but is not `preamble_complete` is the same
    /// case: the scan reached it and stopped inside it, so whatever DDL it
    /// holds is a fragment rather than an answer.
    #[test]
    fn an_incomplete_database_entry_is_also_metadata_not_scanned() {
        let mut db = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
        db.preamble_complete = false;
        let meta = DumpMetadata { databases: vec![db] };
        let cols = vec!["id".to_string(), "other".to_string()];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        // The column the fragment *does* declare still resolves — what the
        // dump said is what the dump said. Only the unexplained one carries
        // the "ask again later" outcome.
        assert_eq!(
            resolved.columns,
            [ColumnResolution::Mapped, ColumnResolution::MetadataNotScanned]
        );
    }

    /// `SchemaMode::Strings` never looks, so it never reaches this outcome
    /// either — every column is `NotDeclared`, matching the untyped path
    /// exactly.
    #[test]
    fn strings_mode_never_reports_metadata_not_scanned() {
        let mut first = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
        first.name = Some("first".to_string());
        let meta = DumpMetadata { databases: vec![first] };
        let cols = vec!["id".to_string()];
        let resolved = resolve_columns(
            "public.t",
            &cols,
            Some(&meta),
            Some("second"),
            SchemaMode::Strings,
            &[],
        );
        assert_eq!(resolved.columns, [ColumnResolution::NotDeclared]);
    }

    /// Every non-`Mapped` outcome `crate::pgtype` can produce, seen through
    /// the join — and the plan a `Utf8View` fallback carries, which is
    /// `Scalar` whichever reason put it there.
    #[test]
    fn unknown_opaque_and_empty_enum_outcomes() {
        let types = vec![
            TypeDef { name: "public.gtype".to_string(), kind: TypeKind::Base },
            TypeDef { name: "public.mood".to_string(), kind: TypeKind::Enum { labels: vec![] } },
        ];
        let meta = one_db(
            &[(
                "public.t",
                &[
                    ("a", "money"),
                    ("b", "public.gtype[]"),
                    ("c", "public.gtype"),
                    ("d", "public.mood"),
                ],
            )],
            types,
        );
        let cols = vec!["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string()];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert_eq!(
            resolved.columns,
            [
                ColumnResolution::UnknownType,
                ColumnResolution::OpaqueElementType,
                ColumnResolution::OpaqueBaseType,
                ColumnResolution::EmptyEnum,
            ]
        );
        assert!(
            resolved
                .schema
                .fields()
                .iter()
                .all(|f| *f.data_type() == arrow::datatypes::DataType::Utf8View),
            "every unmapped outcome falls back to text"
        );
        assert_eq!(resolved.plans, vec![NestedPlan::Scalar; 4]);
    }

    /// The `(DataType, NestedPlan)` pair reaches the schema positionally,
    /// with one entry per column whether or not the column is nested — so no
    /// consumer has to ask whether `plans` applies to it.
    #[test]
    fn a_nested_column_carries_its_plan_beside_its_type() {
        let types = vec![TypeDef {
            name: "public.point2d".to_string(),
            kind: TypeKind::Composite {
                fields: Some(vec![("x".to_string(), "integer".to_string())]),
            },
        }];
        let meta = one_db(
            &[("public.t", &[("id", "integer"), ("v", "integer[]"), ("p", "public.point2d")])],
            types,
        );
        let cols = vec!["id".to_string(), "v".to_string(), "p".to_string()];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert!(resolved.columns.iter().all(|c| *c == ColumnResolution::Mapped));
        assert_eq!(
            resolved.plans,
            [
                NestedPlan::Scalar,
                NestedPlan::Array(Box::new(NestedPlan::Scalar)),
                NestedPlan::Record(vec![NestedPlan::Scalar]),
            ]
        );
        assert_eq!(resolved.plans.len(), resolved.schema.fields().len());
    }

    fn shape(dims: Option<(u8, u8)>, lower_bound_prefix: bool) -> ArrayShape {
        ArrayShape { dims, lower_bound_prefix }
    }

    /// One table with one array column, resolved against `census`.
    fn array_column(census: &[ArrayShape]) -> ResolvedSchema {
        let meta = one_db(&[("public.t", &[("v", "integer[]")])], vec![]);
        let cols = vec!["v".to_string()];
        resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, census)
    }

    fn list_of(child: arrow::datatypes::DataType) -> arrow::datatypes::DataType {
        arrow::datatypes::DataType::List(Arc::new(Field::new("item", child, true)))
    }

    /// Uniform depth retypes the pair — both halves, in step
    /// (`docs/design/architecture.md`, "The array shape census").
    #[test]
    fn a_uniformly_deep_array_column_gains_a_list_level_per_dimension() {
        use arrow::datatypes::DataType::Int32;
        for (depth, expected) in [
            (1, list_of(Int32)),
            (2, list_of(list_of(Int32))),
            (3, list_of(list_of(list_of(Int32)))),
        ] {
            let resolved = array_column(&[shape(Some((depth, depth)), false)]);
            assert_eq!(resolved.schema.field(0).data_type(), &expected, "depth {depth}");
            assert_eq!(resolved.columns, [ColumnResolution::Mapped], "depth {depth}");
            let mut plan = NestedPlan::Scalar;
            for _ in 0..depth {
                plan = NestedPlan::Array(Box::new(plan));
            }
            assert_eq!(resolved.plans, [plan], "depth {depth}");
        }
    }

    /// The two shapes no `List` type is honest about, and the one that
    /// constrains nothing at all.
    #[test]
    fn a_varying_or_decorated_array_column_becomes_text_and_says_why() {
        for (census, why) in [
            (shape(Some((1, 2)), false), "mixed dimensionality"),
            (shape(Some((1, 1)), true), "a lower-bound prefix, however uniform"),
        ] {
            let resolved = array_column(&[census]);
            assert_eq!(resolved.columns, [ColumnResolution::VaryingArrayShape], "{why}");
            assert_eq!(
                resolved.schema.field(0).data_type(),
                &arrow::datatypes::DataType::Utf8View,
                "{why}"
            );
            assert_eq!(resolved.plans, [NestedPlan::Scalar], "{why}");
            // Still a `Warning` on the shared scale, like every other
            // fallback to text.
            assert_eq!(resolved.notes[0].severity(), Severity::Warning, "{why}");
        }

        // Every value was NULL or `{}`: nothing was constrained, so the
        // optimistic type stands.
        let unconstrained = array_column(&[shape(None, false)]);
        assert_eq!(unconstrained.columns, [ColumnResolution::Mapped]);
        assert_eq!(
            unconstrained.schema.field(0).data_type(),
            &list_of(arrow::datatypes::DataType::Int32)
        );
    }

    /// A leading brace run longer than `MAXDIM` did not come out of
    /// `array_out` (I25), so it is not evidence about an array. Degrading the
    /// column on it would hide a damaged file behind a text column, and
    /// believing it would build a `List` nested as deep as the file asked —
    /// so the column keeps its optimistic type and the offending row is a
    /// `FieldDecode` like any other value contradicting its declared type.
    #[test]
    fn a_brace_run_past_the_dimension_limit_is_not_evidence() {
        for dims in [(7, 7), (200, 200), (1, 200)] {
            let resolved = array_column(&[shape(Some(dims), false)]);
            assert_eq!(resolved.columns, [ColumnResolution::Mapped], "{dims:?}");
            assert_eq!(
                resolved.schema.field(0).data_type(),
                &list_of(arrow::datatypes::DataType::Int32),
                "{dims:?}"
            );
        }
    }

    /// The census is type-blind and keyed by column, so it constrains exactly
    /// one thing: a column the DDL resolved to an array. A composite and a
    /// multirange (whose `List` is filled by `multirange_out`, not
    /// `array_out`) read the same census entry and must ignore it.
    ///
    /// The third column is what makes that list complete: a plan already
    /// nested — the only shape whose census would have to be *disbelieved*
    /// rather than merely ignored — cannot reach the transform at all, because
    /// an array-typed element is refused at resolution (I26). It is spelled as
    /// a domain over an array, which is the only DDL shape that reaches the
    /// refusal: `integer[][]` is a spelling of `integer[]` (I28) and resolves
    /// to a plain `List<Int32>` the census is free to deepen.
    #[test]
    fn the_census_only_speaks_for_a_column_the_ddl_resolved_to_an_array() {
        let types = vec![
            TypeDef {
                name: "public.point2d".to_string(),
                kind: TypeKind::Composite {
                    fields: Some(vec![("x".to_string(), "integer".to_string())]),
                },
            },
            TypeDef {
                name: "public.intarr".to_string(),
                kind: TypeKind::Domain { base_type: "integer[]".to_string() },
            },
        ];
        let meta = one_db(
            &[(
                "public.t",
                &[("c", "public.point2d"), ("m", "int4multirange"), ("n", "public.intarr[]")],
            )],
            types,
        );
        let cols = vec!["c".to_string(), "m".to_string(), "n".to_string()];
        let census = vec![shape(Some((2, 2)), true); 3];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &census);
        assert_eq!(
            resolved.columns,
            [
                ColumnResolution::Mapped,
                ColumnResolution::Mapped,
                ColumnResolution::NestedArrayElement
            ]
        );
        let untouched =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert_eq!(resolved.schema, untouched.schema);
        assert_eq!(resolved.plans, untouched.plans);
    }

    /// A census shorter than the column list — a header-less block's, which
    /// ends at the highest field that ever held a brace — leaves the columns
    /// past its end exactly as the DDL resolved them.
    #[test]
    fn a_census_shorter_than_the_column_list_constrains_only_what_it_covers() {
        let meta = one_db(&[("public.t", &[("a", "integer[]"), ("b", "integer[]")])], vec![]);
        let cols = vec!["a".to_string(), "b".to_string()];
        let resolved = resolve_columns(
            "public.t",
            &cols,
            Some(&meta),
            None,
            SchemaMode::Typed,
            &[shape(Some((1, 2)), false)],
        );
        assert_eq!(
            resolved.columns,
            [ColumnResolution::VaryingArrayShape, ColumnResolution::Mapped]
        );
        assert_eq!(
            resolved.schema.field(1).data_type(),
            &list_of(arrow::datatypes::DataType::Int32)
        );
    }

    #[test]
    fn diagnostic_carries_the_declared_string_when_present() {
        let meta = one_db(&[("public.t", &[("id", "integer")])], vec![]);
        let cols = vec!["id".to_string(), "extra".to_string()];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert_eq!(resolved.notes[0].declared.as_deref(), Some("integer"));
        assert_eq!(resolved.notes[1].declared, None);
    }

    /// `database_for_name` selects by the attributed database's *name*, not
    /// by which database's DDL happens to mention the table first — proven
    /// here by giving the two databases genuinely different declared types
    /// for the same qualified table name, so the outcome differs observably
    /// depending on which name is passed. This is what the one-target-per-query
    /// rule
    /// (`docs/design/architecture.md`, "One target per query") turned the old first-match guess into: the caller already
    /// knows, from the matched `CopyBlock`'s own attribution, which database
    /// applies — see `docs/design/architecture.md`,
    /// "One target per query", for the guess this test used to pin down.
    #[test]
    fn database_selects_by_attributed_name_not_by_first_match() {
        let mut a = one_db(&[("public.t", &[("id", "text")])], vec![]).databases.remove(0);
        a.name = Some("a".to_string());
        let mut b = one_db(&[("public.t", &[("id", "integer")])], vec![]).databases.remove(0);
        b.name = Some("b".to_string());
        let meta = DumpMetadata { databases: vec![a, b] };
        let cols = vec!["id".to_string()];

        let resolved_a =
            resolve_columns("public.t", &cols, Some(&meta), Some("a"), SchemaMode::Typed, &[]);
        assert_eq!(resolved_a.schema.field(0).data_type(), &arrow::datatypes::DataType::Utf8View);

        let resolved_b =
            resolve_columns("public.t", &cols, Some(&meta), Some("b"), SchemaMode::Typed, &[]);
        assert_eq!(resolved_b.schema.field(0).data_type(), &arrow::datatypes::DataType::Int32);
    }

    /// A [`ColumnNote`]'s severity is derived from its resolution, never
    /// stored — so it cannot drift out of agreement with the outcome it
    /// describes, the same reason `DumpMetadata` is a view over spans.
    #[test]
    fn column_note_severity_follows_its_resolution() {
        let note = |resolution| ColumnNote { column: "c".to_string(), declared: None, resolution };
        assert_eq!(note(ColumnResolution::Mapped).severity(), Severity::Info);
        for fell_back in [
            ColumnResolution::UnknownType,
            ColumnResolution::NotDeclared,
            ColumnResolution::MetadataNotScanned,
            ColumnResolution::OpaqueBaseType,
            ColumnResolution::EmptyEnum,
        ] {
            assert_eq!(note(fell_back.clone()).severity(), Severity::Warning, "{fell_back:?}");
        }
    }

    /// The two channels share one scale, which is the whole point of keeping
    /// them separate types: a caller can filter both by the same threshold
    /// without knowing which produced what.
    #[test]
    fn both_channels_order_on_one_severity_scale() {
        use crate::diagnostic::{Diagnostic, DiagnosticKind};

        let file = Diagnostic::tiling_broken(vec![crate::map::TilingIssue::Empty { index: 0 }]);
        let column = ColumnNote {
            column: "c".to_string(),
            declared: None,
            resolution: ColumnResolution::UnknownType,
        };
        assert!(file.severity > column.severity());
        assert!(matches!(file.kind, DiagnosticKind::TilingBroken { .. }));
    }
}
