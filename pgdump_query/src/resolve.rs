//! `ResolvedSchema`: the join of a `COPY` header's column list against
//! `DumpMetadata` (`docs/design/decisions.md`, "D43").
//!
//! This is what a query's `RecordBatch`es actually carry, not a preview:
//! `crate::batch::RowBatcher` builds one column builder per field of the
//! [`ResolvedSchema`] it was constructed from, and `RecordBatch::try_new`
//! rejects any array whose type disagrees with the schema's. Deciding *how*
//! a field's text becomes a value is `crate::decode`'s and `crate::nested`'s
//! job; this module decides *what* each column is.

use std::sync::Arc;

use arrow::datatypes::{Field, Schema, SchemaRef};

use crate::diagnostic::{Finding, Severity};
use crate::index::{ArrayShape, PG_ARRAY_MAX_DIMS};
use crate::pgtype::{
    CompareKind, ComparisonPlan, ComparisonSemantics, NestedPlan, TypeOutcome, comparison_for,
    resolve_declared_type, with_extension,
};
use crate::preamble::{DatabaseMetadata, DumpMetadata};

/// Whether a query resolves column types at all.
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
/// mapping table. Distinct from [`crate::pgtype::TypeOutcome`]: this adds
/// the "no DDL explained this column at all" case, which is a
/// join-against-`DumpMetadata` question, not a type-mapping one.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnResolution {
    Mapped,
    /// Declared, but this build's mapping table has nothing for it.
    UnknownType,
    /// No DDL explained this column — `--data-only`, a typed table (`CREATE
    /// TABLE x OF t`), or `SchemaMode::Strings` (which never looks).
    NotDeclared,
    /// The scan has not finished reading this block's database's DDL, and it
    /// has not declared this column — so nothing is yet known about it, nor
    /// about any column where the scan never reached that DDL at all.
    ///
    /// **No mapping scan produces this pairing**: `crate::stream`'s mapping
    /// pass states a database's DDL at that database's first `COPY` block
    /// (I1's recurring boundary), before any of its blocks can be banked.
    /// What keeps the variant is that [`resolve_columns`] is public and takes
    /// its `metadata` from the caller, so an embedder resolving against an
    /// index it assembled itself can still present the condition.
    ///
    /// Held apart from [`Self::NotDeclared`], which is final where this means
    /// "finish the parse and ask again" (`docs/design/decisions.md`, "D43").
    MetadataNotScanned,
    /// An array whose element type is opaque by construction — `box`, a
    /// C-level base type or a shell type, through any chain of domains. Held
    /// apart from [`Self::OpaqueBaseType`] because the column's *own* type is
    /// understood; it is the element that is not, and the array's separator is
    /// the element type's (I22).
    OpaqueElementType,
    /// An array column whose element type is itself an array, through any
    /// chain of domains (I26). Held apart from [`Self::OpaqueElementType`]
    /// because the label would lie: `integer[]` is not opaque, it is
    /// understood and declined.
    NestedArrayElement,
    /// An array column whose values do not share one Arrow list shape: the
    /// dimensionality differs between rows, or some value carries an
    /// `[lb:ub]=` lower-bound prefix. Both are properties of the *value*
    /// (I21), so only the array-shape census can report them — and it reports
    /// them before the schema commits (`docs/design/decisions.md`, "D35").
    VaryingArrayShape,
    /// A C-level base type or a shell/undefined type.
    OpaqueBaseType,
    EmptyEnum,
    /// A column the map says holds a value PostgreSQL accepts for its declared
    /// type and its Arrow type cannot hold, in the tiers the query's front end
    /// reads, read as its text because the query asked for the untyped mode
    /// ([`crate::UnrepresentableMode::Text`]).
    ///
    /// **It compares in each front end's semantics**: bytewise over that text
    /// in DataFusion's, the order DataFusion evaluates a `Utf8View` in, so no
    /// statistic gathered in the declared type's order is read for it; and in
    /// PostgreSQL's in its declared type's order, special values ranked
    /// ([`ResolvedSchema::comparisons`] keeping the declared type's plan
    /// there alone). A property of the query as much as of the column, so
    /// only a query's resolution produces it ([`read_as_text`]).
    UnrepresentableValues,
}

impl ColumnResolution {
    /// The sentence a human reads for this outcome — what `pgdt info
    /// --detail` prints beside the column, and the tail of
    /// [`ColumnNote`]'s [`Finding::message`].
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Mapped => "mapped",
            Self::UnknownType => "unknown type — no mapping for this build",
            Self::NotDeclared => "not declared — no DDL explained this column",
            Self::MetadataNotScanned => {
                "metadata not scanned — the scan has not finished this database's DDL; finish \
                 the parse"
            }
            Self::OpaqueElementType => {
                "opaque element type — the array's element type is information-free in the dump"
            }
            Self::NestedArrayElement => {
                "nested array element — the array's element type is itself an array"
            }
            Self::VaryingArrayShape => {
                "varying array shape — dimensionality differs between rows, or a value carries an \
                 explicit lower bound"
            }
            Self::OpaqueBaseType => "opaque base type — information-free in the dump",
            Self::EmptyEnum => "empty enum",
            Self::UnrepresentableValues => {
                "unrepresentable values — it holds a value its type cannot hold, and the untyped \
                 mode reads it as text"
            }
        }
    }
}

/// One column's full resolution, named and carrying the raw declared type
/// string (if any DDL named one) alongside the outcome — what a human-facing
/// display (`pgdt info`) needs in one place, for every column.
///
/// **A note, not a diagnostic**: there is exactly one per column, always. The
/// file-level exception channel is [`crate::diagnostic::Diagnostic`], kept a
/// separate type because `DumpIndex` is L1 and [`ColumnResolution`] an L2
/// conclusion (`docs/design/decisions.md`, "D68"). What they share is the
/// [`Severity`] scale, so a caller reading both filters uniformly.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnNote {
    pub column: String,
    pub declared: Option<String>,
    pub resolution: ColumnResolution,
}

impl Finding for ColumnNote {
    /// Where this column sits on the shared [`Severity`] scale: a column that
    /// resolved to a real Arrow type is `Info`; anything that fell back to
    /// `Utf8View` is a `Warning`, since its values come back unparsed.
    ///
    /// Derived rather than stored — it is a pure function of `resolution`,
    /// and a stored copy could disagree with it.
    fn severity(&self) -> Severity {
        match self.resolution {
            ColumnResolution::Mapped => Severity::Info,
            _ => Severity::Warning,
        }
    }

    /// The column, its declared type where DDL named one, and
    /// [`ColumnResolution::describe`] — and, for a column that fell back,
    /// that its value is the file's text and compares as that text, which is
    /// the only finding such a column earns about its comparison under
    /// DataFusion's semantics; under PostgreSQL's, an `UnknownType` or
    /// `OpaqueBaseType` column is also announced as unmodelled
    /// (`crate::predicate::column_divergences`).
    fn message(&self) -> String {
        let declared = self.declared.as_deref().map(|d| format!(" ({d})")).unwrap_or_default();
        let fallback = match self.resolution {
            ColumnResolution::Mapped => "",
            ColumnResolution::UnrepresentableValues => {
                "; its value is the file's text, compared as that text in DataFusion's semantics \
                 and in its declared type's order in PostgreSQL's"
            }
            _ => "; its value is the file's text, and compares as that text",
        };
        format!("column `{}`{declared}: {}{fallback}", self.column, self.resolution.describe())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A table query's resolved schema: the Arrow schema a query's
/// `RecordBatch`es carry (see the module docs), plus one [`ColumnNote`] per
/// column.
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
    /// The Arrow type cannot say (`docs/design/decisions.md`, "D39"), so the
    /// plan travels beside it from resolution into `crate::batch::RowBatcher`
    /// and `crate::batch::render_field`. A scalar column's entry is
    /// `NestedPlan::Scalar`, so every column has one.
    pub plans: Vec<NestedPlan>,
    /// How each column compares — positional, parallel to `schema.fields()`
    /// like `columns` and `plans`.
    ///
    /// An L2 conclusion, produced here because this is where its inputs meet:
    /// the declared type string and the database's own `CREATE TYPE` list.
    /// `crate::predicate` reads this instead of inspecting the Arrow type
    /// (`crate::pgtype::comparison_for`).
    ///
    /// A column that did not resolve `Mapped` is [`ComparisonPlan::Refused`]
    /// whatever its declared type said — the census can take a column out of
    /// `Mapped` after the declared type has been read — but for
    /// [`ColumnResolution::UnrepresentableValues`] under
    /// [`crate::ComparisonSemantics::Postgres`], which keeps the declared
    /// type's plan ([`read_as_text`]).
    pub comparisons: Vec<ComparisonPlan>,
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
            comparisons: Vec::new(),
        }
    }
}

impl ResolvedSchema {
    /// How many columns have no `Mapped` resolution — the count `pgdt info`'s
    /// default summary line reports (`"N of M columns unmapped"`).
    pub fn unmapped_count(&self) -> usize {
        self.columns.iter().filter(|c| **c != ColumnResolution::Mapped).count()
    }

    /// The kinds gathering orders column `i`'s bounds and row order by, one
    /// per [`crate::BoundsSet`] ([`ComparisonPlan::bounds_kinds`]), for a scalar
    /// column — and none for a nested one. A column nothing declared is
    /// [`ComparisonPlan::Refused`], and bounded as its text.
    ///
    /// **Meaningful for a schema resolved as gathering resolves it**:
    /// `SchemaMode::Typed` against the block's own DDL and no census
    /// (`crate::gather`). A query's schema says only which kind a term reads
    /// bounds by ([`ComparisonPlan::bounds_read_by`]); which stored set that
    /// is, is read off these kinds recomputed for the block
    /// ([`crate::pgtype::bounds_set_keyed_by`]).
    pub fn bounds_kinds(&self, i: usize) -> [Option<CompareKind>; 2] {
        if self.plans[i].is_scalar() { self.comparisons[i].bounds_kinds() } else { [None, None] }
    }
}

/// Find the database named `database` in `metadata.databases` — an exact
/// match on [`crate::preamble::DatabaseMetadata::name`], never a guess.
/// `database: None` matches the single unnamed database a plain (non-`\connect`)
/// dump produces.
///
/// Per-block attribution (`docs/design/decisions.md`, "D49") is what makes the
/// exact match possible.
pub(crate) fn database_for_name<'a>(
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
/// longer than [`PG_ARRAY_MAX_DIMS`] did not come out of `array_out` at all
/// (I25), so it is not evidence about an array and must not degrade the
/// column: the honest outcome is the optimistic type plus a `FieldDecode`
/// naming the row. Only after that does a lower-bound prefix disqualify the
/// column on its own, however uniform the dimensionality is.
fn shape_verdict(shape: ArrayShape) -> ShapeVerdict {
    if shape.dims.is_some_and(|(_, max)| max > PG_ARRAY_MAX_DIMS) {
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
/// **the pair, never a half** (`docs/design/decisions.md`, "D39"). This and
/// [`read_as_text`] are the only places after `resolve_declared_type` where
/// either changes.
///
/// Only a column the DDL resolved to an array is touched. The census is keyed
/// by column and records the shape of the whole field, so it says nothing
/// about an array nested inside a composite or inside another array's element
/// type; leaving those on the optimistic path is what keeps this transform
/// from misreading them.
///
/// Deficiency register: `deficiency: KD2` — an array at that depth is
/// therefore decided optimistically, and a multi-dimensional or
/// `[lb:ub]=`-decorated value in one is a hard `Error::FieldDecode` naming the
/// column, which scanning more of the file cannot improve. **(c) unowned**,
/// deferred on frequency. Closing it means a census keyed by *path* rather
/// than by column, so a shape one level down has somewhere to be recorded,
/// and finding another population for the two test floors the types
/// fixture's `t_composite_matrix` alone fills (`tests/pruning.rs`,
/// `datafusion-pgdump/tests/statistics.rs`).
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

/// Resolve `columns` (in `COPY`-header order, and empty for a header listing
/// none — I5) against `metadata` for `qualified_table`, scoped to the
/// database this block was attributed to (`database` — `None` for a plain
/// dump's single unnamed database).
///
/// `census` is the array-shape evidence this schema may commit to, positional
/// like `columns` (`docs/design/decisions.md`, "D35"). A caller with no
/// evidence — or none it may believe, which for a *reported* schema means
/// `crate::index::DumpIndex::is_complete` is false — passes `&[]`, and every
/// column keeps the optimistic type the DDL alone gives it.
///
/// A column of a database whose DDL the scan never reached comes back
/// [`ColumnResolution::MetadataNotScanned`] rather than `NotDeclared` — see
/// that variant. This needs `metadata` to be `Some`: a caller with no metadata
/// at all has no DDL for *any* database, which is exactly `NotDeclared`.
///
/// `SchemaMode::Strings` never looks anything up: every column comes back
/// `NotDeclared`/`Utf8View`, matching the untyped path exactly — and with no
/// `NestedPlan::Array` anywhere, the census cannot reach it either.
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
    // Metadata exists, but not for *this* block's database, so "not declared"
    // would be a different, final claim. `metadata: None` is a caller with no
    // DDL at all, which is exactly `NotDeclared`.
    let unscanned_database =
        mode == SchemaMode::Typed && metadata.is_some() && !db.is_some_and(|d| d.preamble_complete);

    let mut fields = Vec::with_capacity(columns.len());
    let mut resolutions = Vec::with_capacity(columns.len());
    let mut notes = Vec::with_capacity(columns.len());
    let mut plans = Vec::with_capacity(columns.len());
    let mut comparisons = Vec::with_capacity(columns.len());

    let string = || (arrow::datatypes::DataType::Utf8View, NestedPlan::Scalar);
    for (i, name) in columns.iter().enumerate() {
        let declared = db.and_then(|d| d.declared_column(qualified_table, name));
        let (resolution, pair, comparison) = match declared {
            None if unscanned_database => {
                (ColumnResolution::MetadataNotScanned, string(), ComparisonPlan::Refused)
            }
            None => (ColumnResolution::NotDeclared, string(), ComparisonPlan::Refused),
            Some(column) => {
                let ty = &column.declared_type;
                // `db` is always `Some` here: `declared` came from it, so
                // `db.types` is this same database's list.
                let types = &db.unwrap().types;
                let refused = ComparisonPlan::Refused;
                // Asked per column rather than per declared type: the column's
                // own `COLLATE` clause is half the question, and the dump's
                // `CREATE COLLATION` list says whether it is deterministic
                // (I42).
                let collations = &db.unwrap().collations;
                let comparison =
                    || comparison_for(ty, column.collation.as_deref(), types, collations);
                match resolve_declared_type(ty, types) {
                    TypeOutcome::Mapped(dt, plan) => {
                        (ColumnResolution::Mapped, (dt, plan), comparison())
                    }
                    TypeOutcome::Unknown => (ColumnResolution::UnknownType, string(), refused),
                    TypeOutcome::OpaqueElementType => {
                        (ColumnResolution::OpaqueElementType, string(), refused)
                    }
                    TypeOutcome::NestedArrayElement => {
                        (ColumnResolution::NestedArrayElement, string(), refused)
                    }
                    TypeOutcome::OpaqueBaseType => {
                        (ColumnResolution::OpaqueBaseType, string(), refused)
                    }
                    TypeOutcome::EmptyEnum => (ColumnResolution::EmptyEnum, string(), refused),
                }
            }
        };
        // The census is the only thing that can speak for an array column's
        // shape, and it speaks after the DDL, never instead of it.
        let (resolution, (arrow_type, plan)) =
            retype_from_census(census.get(i).copied().unwrap_or_default(), resolution, pair);
        // ... and it may take the column *out* of `Mapped` after the declared
        // type has been read. A comparison plan is only consulted for a column
        // that stayed in, so the two are kept in step here.
        let comparison = if resolution == ColumnResolution::Mapped {
            comparison
        } else {
            ComparisonPlan::Refused
        };
        // Every Arrow field is nullable, regardless of a `NOT NULL` in the
        // DDL — see `docs/design/decisions.md`, "D37".
        let field = Field::new(name, arrow_type, true);
        // A canonical extension name is a claim about what the column's bytes
        // *are*, so only a still-`Mapped` column may carry one: one the census
        // took back to `Utf8View` holds an `array_out` literal.
        let field = match declared {
            Some(column) if resolution == ColumnResolution::Mapped => with_extension(
                field,
                &column.declared_type,
                // `db` is `Some` wherever `declared` is: it came out of it.
                &db.expect("a declared column came from a database entry").types,
            ),
            _ => field,
        };
        fields.push(field);
        notes.push(ColumnNote {
            column: name.clone(),
            declared: declared.map(|c| c.declared_type.clone()),
            resolution: resolution.clone(),
        });
        resolutions.push(resolution);
        plans.push(plan);
        comparisons.push(comparison);
    }

    ResolvedSchema {
        schema: Arc::new(Schema::new(fields)),
        columns: resolutions,
        notes,
        plans,
        comparisons,
    }
}

/// **Read each column `text` marks as its text**, the untyped mode's widening
/// ([`ColumnResolution::UnrepresentableValues`]): positional like `resolved`'s
/// columns, and applied, as the census is, after the DDL has spoken and only
/// to a column still `Mapped` to a type other than `Utf8View` — one the census
/// or the register already took to text holds every value
/// (`docs/design/decisions.md`, "D100").
///
/// The column becomes `Utf8View` at [`NestedPlan::Scalar`], no extension
/// name, its bytes being the file's literal. **Its comparison is its query's
/// semantics'**: the declared type's plan under
/// [`ComparisonSemantics::Postgres`], and under
/// [`ComparisonSemantics::DataFusion`] [`ComparisonPlan::Refused`], which
/// that semantics compares bytewise as every column read as text.
pub(crate) fn read_as_text(
    resolved: &mut ResolvedSchema,
    text: &[bool],
    semantics: ComparisonSemantics,
) {
    use arrow::datatypes::DataType;

    if !text.contains(&true) {
        return;
    }
    let mut fields: Vec<Field> =
        resolved.schema.fields().iter().map(|field| field.as_ref().clone()).collect();
    let mut widened = false;
    for (i, field) in fields.iter_mut().enumerate() {
        let read_as_text = text.get(i).copied().unwrap_or(false)
            && resolved.columns[i] == ColumnResolution::Mapped
            && *field.data_type() != DataType::Utf8View;
        if !read_as_text {
            continue;
        }
        widened = true;
        *field = Field::new(field.name(), DataType::Utf8View, true);
        resolved.columns[i] = ColumnResolution::UnrepresentableValues;
        resolved.notes[i].resolution = ColumnResolution::UnrepresentableValues;
        resolved.plans[i] = NestedPlan::Scalar;
        if semantics == ComparisonSemantics::DataFusion {
            resolved.comparisons[i] = ComparisonPlan::Refused;
        }
    }
    if widened {
        resolved.schema =
            Arc::new(Schema::new_with_metadata(fields, resolved.schema.metadata().clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preamble::{ColumnDef, DatabaseMetadata, TableDef, TypeDef, TypeKind};

    fn one_db(tables: &[(&str, &[(&str, &str)])], types: Vec<TypeDef>) -> DumpMetadata {
        let mut db = DatabaseMetadata {
            name: None,
            preamble_complete: true,
            server_version: None,
            pg_dump_version: None,
            extensions: Vec::new(),
            types,
            collations: Vec::new(),
            tables: Default::default(),
        };
        for (name, cols) in tables {
            db.tables.insert(
                name.to_string(),
                TableDef::with_columns(cols.iter().map(|(c, t)| ColumnDef::new(*c, *t)).collect()),
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
    /// `MetadataNotScanned`, not `NotDeclared`: one is final, the other says
    /// "finish the parse and ask again".
    ///
    /// With *no* metadata at all there is no scan to finish, so `NotDeclared`
    /// stands — the third case below.
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
        // The column the fragment *does* declare still resolves; only the
        // unexplained one carries the "ask again later" outcome.
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

    /// Every non-`Mapped` outcome `crate::pgtype` can produce but
    /// `NestedArrayElement`, seen through the join — and the plan a `Utf8View` fallback carries, which is
    /// `Scalar` whichever reason put it there.
    #[test]
    fn unknown_opaque_and_empty_enum_outcomes() {
        let types = vec![
            TypeDef { name: "public.gtype".to_string(), kind: TypeKind::Base },
            TypeDef {
                name: "public.mood".to_string(),
                kind: TypeKind::Enum { labels: vec![], exact: true },
            },
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
            kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
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
    /// (`docs/design/decisions.md`, "D35").
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
    /// `array_out` (I25), so it is not evidence about an array: the column
    /// keeps its optimistic type and the offending row is a `FieldDecode`
    /// like any other value contradicting its declared type.
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
    /// The third column completes the list: a plan already nested — the one
    /// shape whose census would have to be *disbelieved* rather than ignored
    /// — cannot reach the transform, because an array-typed element is refused
    /// at resolution (I26), and `integer[][]` is a spelling of `integer[]`
    /// (I28) the census is free to deepen.
    #[test]
    fn the_census_only_speaks_for_a_column_the_ddl_resolved_to_an_array() {
        let types = vec![
            TypeDef {
                name: "public.point2d".to_string(),
                kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
            },
            TypeDef { name: "public.intarr".to_string(), kind: TypeKind::domain("integer[]") },
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

    /// A census shorter than the column list — one ending at the highest
    /// field that ever held a brace — leaves the columns
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

    /// The comparison plan is a fourth positional vector, one entry per column
    /// whether or not the column has an order — so no consumer has to ask
    /// whether it applies — and its value is the register's own answer.
    #[test]
    fn every_column_carries_a_comparison_plan() {
        use crate::pgtype::{CompareKind, ComparisonPlan};

        let types = vec![TypeDef {
            name: "public.mood".to_string(),
            kind: TypeKind::Enum { labels: vec!["sad".to_string()], exact: true },
        }];
        let meta = one_db(
            &[(
                "public.t",
                &[
                    ("i", "integer"),
                    ("s", "text"),
                    ("m", "public.mood"),
                    ("v", "integer[]"),
                    ("u", "money"),
                ],
            )],
            types,
        );
        let cols: Vec<String> = ["i", "s", "m", "v", "u"].iter().map(ToString::to_string).collect();
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert_eq!(resolved.comparisons.len(), resolved.schema.fields().len());
        assert_eq!(
            resolved.comparisons,
            [
                ComparisonPlan::Compared { kind: CompareKind::Int { bytes: 4 }, divergence: None },
                ComparisonPlan::diverging(
                    CompareKind::Text { length: None },
                    crate::pgtype::ComparisonDivergence::UnknownCollation,
                ),
                ComparisonPlan::Compared {
                    kind: CompareKind::Enum {
                        labels: ["sad".to_string()].into_iter().collect(),
                        exact: true
                    },
                    divergence: None,
                },
                // Nested: compared structurally, one node per level.
                ComparisonPlan::Nested(crate::pgtype::NestedCompare::Array(Box::new(
                    crate::pgtype::NestedCompare::Leaf {
                        declared: "integer".to_string(),
                        kind: CompareKind::Int { bytes: 4 },
                        divergence: None,
                    },
                ))),
                // A type this build never mapped has no order here.
                ComparisonPlan::Refused,
            ]
        );

        // `SchemaMode::Strings` resolves nothing, so it claims no order for
        // any column — which is what makes an ordering operator refuse.
        let strings =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Strings, &[]);
        assert_eq!(strings.comparisons, vec![ComparisonPlan::Refused; 5]);
    }

    /// The register is consulted **per column**, not per declared type: two
    /// `text` columns of one table get different verdicts, because the
    /// `COLLATE` clause is half the question and is a fact about the column.
    #[test]
    fn two_text_columns_of_one_table_can_compare_differently() {
        use crate::pgtype::{CompareKind, ComparisonDivergence};

        let mut meta = one_db(&[("public.t", &[("plain", "text"), ("bytewise", "text")])], vec![]);
        meta.databases[0].tables.get_mut("public.t").unwrap().columns[1].collation =
            Some("pg_catalog.\"C\"".to_string());

        let cols = vec!["plain".to_string(), "bytewise".to_string()];
        let resolved =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert_eq!(
            resolved.comparisons,
            [
                ComparisonPlan::diverging(
                    CompareKind::Text { length: None },
                    ComparisonDivergence::UnknownCollation
                ),
                ComparisonPlan::Compared {
                    kind: CompareKind::Text { length: None },
                    divergence: None
                },
            ]
        );
        // Both are still the same Arrow type and the same comparison — only
        // the verdict moved.
        assert_eq!(resolved.schema.field(0).data_type(), resolved.schema.field(1).data_type());
    }

    /// The other half of the same join: the register also reads the database's
    /// own `CREATE COLLATION` list, because a `COLLATE` clause naming a
    /// collation the dump declares `deterministic = false` is the one equality
    /// divergence a plain dump states outright (I42). What moves is what the
    /// whole dump said about that name, not the clause.
    #[test]
    fn the_dumps_own_collation_list_decides_whether_a_clause_is_deterministic() {
        use crate::pgtype::{CompareKind, ComparisonDivergence};
        use crate::preamble::CollationDef;

        let build = |deterministic: bool| {
            let mut meta = one_db(&[("public.t", &[("v", "text")])], vec![]);
            meta.databases[0].tables.get_mut("public.t").unwrap().columns[0].collation =
                Some("public.icu_ci".to_string());
            meta.databases[0].collations =
                vec![CollationDef { name: "public.icu_ci".to_string(), deterministic }];
            let cols = vec!["v".to_string()];
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[])
        };

        assert_eq!(
            build(true).comparisons,
            [ComparisonPlan::diverging(
                CompareKind::Text { length: None },
                ComparisonDivergence::NonBytewiseCollation
            )]
        );
        assert_eq!(
            build(false).comparisons,
            [ComparisonPlan::diverging(
                CompareKind::Text { length: None },
                ComparisonDivergence::NonDeterministicCollation
            )]
        );
    }

    /// The census speaks after the declared type and can take a column out of
    /// `Mapped` once the plan has been read — so the plan is re-answered
    /// against the outcome that survived.
    #[test]
    fn a_census_refused_column_claims_no_order() {
        use crate::pgtype::ComparisonPlan;

        let meta = one_db(&[("public.t", &[("v", "text")])], vec![]);
        let cols = vec!["v".to_string()];
        let text = resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        assert_eq!(
            text.comparisons,
            [ComparisonPlan::diverging(
                crate::pgtype::CompareKind::Text { length: None },
                crate::pgtype::ComparisonDivergence::UnknownCollation,
            )]
        );

        // The same column declared as an array, whose values do not share one
        // list shape: `VaryingArrayShape` holds *array* literals as text, and
        // comparing them as text is not an order this build defines.
        let varying = array_column(&[shape(Some((1, 2)), false)]);
        assert_eq!(varying.columns, [ColumnResolution::VaryingArrayShape]);
        assert_eq!(varying.comparisons, [ComparisonPlan::Refused]);
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

    /// `database_for_name` selects by the attributed database's *name*, not by
    /// which database's DDL mentions the table first — asserted by giving two
    /// databases different declared types for the same qualified table name.
    /// The caller knows which database applies from the matched `CopyBlock`'s
    /// own attribution (`docs/design/decisions.md`, "D49").
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

    /// **The untyped mode widens a marked column still `Mapped` and no
    /// other**: to `Utf8View` at a scalar plan, keeping the declared type's
    /// comparison in PostgreSQL's semantics and none in DataFusion's, which
    /// compares its text; a column already text holds every value, so is
    /// left as it is, and an unmarked one keeps its type.
    #[test]
    fn the_untyped_mode_widens_a_marked_column_and_compares_it_per_semantics() {
        use arrow::datatypes::DataType;

        let meta = one_db(
            &[("public.t", &[("id", "integer"), ("d", "date"), ("n", "text"), ("a", "date[]")])],
            vec![],
        );
        let cols: Vec<String> = ["id", "d", "n", "a"].iter().map(|c| c.to_string()).collect();
        let declared =
            resolve_columns("public.t", &cols, Some(&meta), None, SchemaMode::Typed, &[]);
        for semantics in [ComparisonSemantics::Postgres, ComparisonSemantics::DataFusion] {
            let mut widened = declared.clone();
            read_as_text(&mut widened, &[false, true, true, true], semantics);
            let types: Vec<&DataType> =
                widened.schema.fields().iter().map(|f| f.data_type()).collect();
            assert_eq!(
                types,
                [&DataType::Int32, &DataType::Utf8View, &DataType::Utf8View, &DataType::Utf8View]
            );
            use ColumnResolution::{Mapped, UnrepresentableValues};
            assert_eq!(
                widened.columns,
                [Mapped, UnrepresentableValues, Mapped, UnrepresentableValues]
            );
            assert_eq!(widened.notes[1].resolution, UnrepresentableValues);
            assert_eq!(widened.plans[3], NestedPlan::Scalar);
            let expected = |i: usize| match semantics {
                ComparisonSemantics::Postgres => declared.comparisons[i].clone(),
                ComparisonSemantics::DataFusion => ComparisonPlan::Refused,
            };
            assert_eq!(widened.comparisons[1], expected(1), "{semantics:?}");
            assert_eq!(widened.comparisons[3], expected(3), "{semantics:?}");
            assert_eq!(widened.comparisons[..1], declared.comparisons[..1]);
            assert_eq!(widened.comparisons[2], declared.comparisons[2]);
        }
    }

    /// A [`ColumnNote`]'s severity is derived from its resolution, never
    /// stored — so it cannot drift out of agreement with the outcome it
    /// describes.
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
            ColumnResolution::UnrepresentableValues,
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
