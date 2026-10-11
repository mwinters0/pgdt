//! `ResolvedSchema`: the join of a `COPY` header's column list against
//! `DumpMetadata` (`docs/design/decisions.md`, "D43").
//!
//! This is what a query's `RecordBatch`es actually carry, not a preview:
//! `pgdt_query::batch::RowBatcher` builds one column builder per field of the
//! [`ResolvedSchema`] it was constructed from, and `RecordBatch::try_new`
//! rejects any array whose type disagrees with the schema's. Deciding *how*
//! a field's text becomes a value is `crate::decode`'s and `crate::nested`'s
//! job; this module decides *what* each column is.

use std::sync::Arc;

use arrow_schema::{Field, Schema, SchemaRef};

use crate::compare::{CompareKind, ComparisonPlan, ComparisonSemantics, comparison_for};
use crate::declared::domain_not_null;
use crate::pgtype::{NestedPlan, TypeOutcome, resolve_declared_type, with_extension};
use pgdt_model::{
    ArrayShape, DatabaseMetadata, DumpMetadata, Finding, PG_ARRAY_MAX_DIMS, Severity,
};

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
    /// No DDL explained this column — `--data-only`, a typed table whose type
    /// is undeclared or read short, or `SchemaMode::Strings` (which never
    /// looks).
    NotDeclared,
    /// The scan has not finished reading this block's database's DDL, and it
    /// has not declared this column — so nothing is yet known about it, nor
    /// about any column where the scan never reached that DDL at all.
    ///
    /// **No mapping scan produces this pairing**: `pgdt_mapping`'s mapping
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
/// file-level exception channel is [`pgdt_model::Diagnostic`], kept a
/// separate type because `DumpIndex` is the model's and [`ColumnResolution`]
/// a codec conclusion (`docs/design/decisions.md`, "D68"). What they share is the
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
    /// (`pgdt_query::column_divergences`).
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
    /// The Arrow type cannot say (`docs/design/decisions.md`, "D43"), so the
    /// plan travels beside it from resolution into `pgdt_query::batch::RowBatcher`
    /// and `pgdt_query::render_field`. A scalar column's entry is
    /// `NestedPlan::Scalar`, so every column has one.
    pub plans: Vec<NestedPlan>,
    /// How each column compares — positional, parallel to `schema.fields()`
    /// like `columns` and `plans`.
    ///
    /// A codec conclusion, produced here because this is where its inputs meet:
    /// the declared type string and the database's own `CREATE TYPE` list.
    /// `pgdt_query::predicate` reads this instead of inspecting the Arrow type
    /// (`crate::compare::comparison_for`).
    ///
    /// A column that did not resolve `Mapped` is [`ComparisonPlan::Refused`]
    /// whatever its declared type said — the census can take a column out of
    /// `Mapped` after the declared type has been read — but for
    /// [`ColumnResolution::UnrepresentableValues`] under
    /// [`crate::compare::ComparisonSemantics::Postgres`], which keeps the declared
    /// type's plan ([`read_as_text`]).
    pub comparisons: Vec<ComparisonPlan>,
    /// Whether each column's declaration makes it `NOT NULL` — on the column,
    /// at the table, through a parent or through a domain — positional like
    /// `columns` (I76). A NULL in such a column is refused wherever it is
    /// read, but under [`pgdt_model::PostgresInvalidValues::Ignore`]; the Arrow
    /// field stays nullable whatever this says (`docs/design/decisions.md`,
    /// "D37"). `false` for every column of a schema resolved with no DDL, the
    /// strings schema mode's included.
    pub not_null: Vec<bool>,
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
            not_null: Vec::new(),
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
    /// per [`pgdt_model::BoundsSet`] ([`ComparisonPlan::bounds_kinds`]), for a scalar
    /// column — and none for a nested one. A column nothing declared is
    /// [`ComparisonPlan::Refused`], and bounded as its text.
    ///
    /// **Meaningful for a schema resolved as gathering resolves it**:
    /// `SchemaMode::Typed` against the block's own DDL and no census
    /// (`pgdt_mapping`'s `statistics::gather`). A query's schema says only which kind a term reads
    /// bounds by ([`ComparisonPlan::bounds_read_by`]); which stored set that
    /// is, is read off these kinds recomputed for the block
    /// ([`crate::compare::bounds_set_keyed_by`]).
    pub fn bounds_kinds(&self, i: usize) -> [Option<CompareKind>; 2] {
        if self.plans[i].is_scalar() { self.comparisons[i].bounds_kinds() } else { [None, None] }
    }
}

/// Find the database named `database` in `metadata.databases` — an exact
/// match on [`pgdt_model::DatabaseMetadata::name`], never a guess.
/// `database: None` matches the single unnamed database a plain (non-`\connect`)
/// dump produces.
///
/// Per-block attribution is what makes the exact match possible.
pub fn database_for_name<'a>(
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
/// **the pair, never a half** (`docs/design/decisions.md`, "D43"). This and
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
/// `crates/datafusion-pgdump/tests/statistics.rs`).
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
    pair: (arrow_schema::DataType, NestedPlan),
) -> (ColumnResolution, (arrow_schema::DataType, NestedPlan)) {
    use arrow_schema::DataType;

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
                plan = NestedPlan::array(plan);
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
/// `pgdt_model::DumpIndex::is_complete` is false — passes `&[]`, and every
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
    let mut not_null = Vec::with_capacity(columns.len());

    let string = || (arrow_schema::DataType::Utf8View, NestedPlan::Scalar);
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
        // Whatever the type resolved to: a column read as its text is still
        // declared `NOT NULL`.
        not_null.push(declared.is_some_and(|column| {
            let db = db.expect("a declared column came from a database entry");
            db.column_not_null(qualified_table, name)
                || domain_not_null(&column.declared_type, &db.types)
        }));
    }

    ResolvedSchema {
        schema: Arc::new(Schema::new(fields)),
        columns: resolutions,
        notes,
        plans,
        comparisons,
        not_null,
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
pub fn read_as_text(resolved: &mut ResolvedSchema, text: &[bool], semantics: ComparisonSemantics) {
    use arrow_schema::DataType;

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
mod tests;
