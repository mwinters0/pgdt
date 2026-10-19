//! What the predicate's tests build their terms, schemas and row groups
//! from, shared by the tests beside each of its modules.

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema};

use super::resolve::{
    BelievedStatistics, ComparedTerm, Comparison, ResolvedMembership, Spellings, resolve_membership,
};
use super::truths::TruthSet;
use super::*;
use pgdt_codec::ColumnNote;
use pgdt_codec::compare::comparison_for;
use pgdt_codec::compare::{CompareKind, ComparisonSemantics};
use pgdt_codec::internal::scalar_read;
use pgdt_codec::pgtype::NestedPlan;
use pgdt_codec::{ColumnResolution, ResolvedSchema};
use pgdt_core::Result;
use pgdt_core::internal::{RawRow, RowSplit};
use pgdt_model::{PostgresInvalidValues, Unrepresentable};
use pgdt_model::{TypeDef, TypeKind, UnrepresentableTier};

/// [`super::resolve::resolve_term`] in PostgreSQL's semantics, which every test
/// but the DataFusion-semantics ones asks for.
pub(super) fn resolve_term(
    predicate: &Predicate,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
) -> Result<ResolvedTerm> {
    super::resolve::resolve_term(
        predicate,
        index,
        resolved,
        header_offset,
        ComparisonSemantics::Postgres,
    )
}

/// A term resolved against a column the register has no plan for — what
/// every `Eq`/`Ne` on an unresolved column falls back to, and what the
/// two NULL tests always are.
pub(super) fn text_term(p: &Predicate, index: usize) -> ResolvedTerm {
    let value = match p.op.operand() {
        Operand::Literal(value) => Some(value),
        Operand::None | Operand::Set(_) => None,
    };
    let compared = value.map(|value| ComparedTerm {
        column: p.column.clone(),
        comparison: Comparison::Canonical(Spellings::one(value.to_string())),
        declared_type: String::new(),
        divergences: Vec::new(),
        statistics: BelievedStatistics::NONE,
    });
    let compared = if p.op.is_ordering() { None } else { compared };
    ResolvedTerm {
        op: p.op.clone(),
        index,
        compared,
        unrepresentable: None,
        tested: None,
        invalid: PostgresInvalidValues::Default,
        not_null: None,
    }
}

/// The type list every one-column schema below resolves against: one
/// enum, so a column declared `public.mood` reaches the register's enum
/// arm rather than its "no such type" one.
pub(super) fn test_types() -> Vec<TypeDef> {
    vec![
        TypeDef {
            name: "public.mood".into(),
            kind: TypeKind::Enum { labels: vec!["sad".into(), "ok".into()], exact: true },
        },
        // A range whose DDL stated no `subtype` — the one shape the
        // resolver calls nested and the register refuses outright, there
        // being no bound type to name the refusal after.
        TypeDef {
            name: "public.opaquerange".into(),
            kind: TypeKind::Range { subtype: None, multirange_type_name: None, canonical: None },
        },
    ]
}

/// A one-column `ResolvedSchema` for `declared`/`data_type`, mapped and
/// scalar — the shape an ordering term is allowed on.
///
/// The comparison plan comes from the register itself rather than being
/// stated here, so these tests exercise the same `declared -> plan` walk
/// `resolve_columns` makes.
pub(super) fn one_column(declared: &str, data_type: DataType) -> ResolvedSchema {
    ResolvedSchema {
        schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
        columns: vec![ColumnResolution::Mapped],
        notes: vec![ColumnNote {
            column: "v".into(),
            declared: Some(declared.into()),
            resolution: ColumnResolution::Mapped,
        }],
        plans: vec![NestedPlan::Scalar],
        comparisons: vec![comparison_for(declared, None, &test_types(), &[])],
        not_null: vec![false],
    }
}

/// A one-column schema for a **nested** declared type, resolved against
/// `types` so the Arrow type, the nested plan and the comparison plan
/// agree the way they do in a real query.
pub(super) fn nested_column(declared: &str, types: &[TypeDef]) -> ResolvedSchema {
    let pgdt_codec::pgtype::TypeOutcome::Mapped(data_type, plan) =
        pgdt_codec::pgtype::resolve_declared_type(declared, types)
    else {
        panic!("{declared} does not resolve")
    };
    ResolvedSchema {
        schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
        columns: vec![ColumnResolution::Mapped],
        notes: vec![ColumnNote {
            column: "v".into(),
            declared: Some(declared.into()),
            resolution: ColumnResolution::Mapped,
        }],
        plans: vec![plan],
        comparisons: vec![comparison_for(declared, None, types, &[])],
        not_null: vec![false],
    }
}

/// A comparing operator before its value: the [`PredicateOp`] variant, as
/// the constructor taking the value.
pub(super) type Comparing = fn(String) -> PredicateOp;

/// What a test reads of a [`Comparing`] operator before it holds a value.
pub(super) trait ComparingExt {
    fn symbol(self) -> &'static str;
    fn is_ordering(&self) -> bool;
}

impl ComparingExt for Comparing {
    fn symbol(self) -> &'static str {
        self(String::new()).symbol()
    }

    fn is_ordering(&self) -> bool {
        self(String::new()).is_ordering()
    }
}

pub(super) fn order_predicate(op: Comparing, value: &str) -> Predicate {
    Predicate { column: "v".into(), op: op(value.into()) }
}

/// Resolve `op value` against a one-column schema and evaluate it over a
/// single-field row holding `field`.
pub(super) fn ordered(
    declared: &str,
    data_type: DataType,
    op: Comparing,
    value: &str,
    field: &str,
) -> Result<bool> {
    let p = order_predicate(op, value);
    let term = resolve_term(&p, 0, &one_column(declared, data_type), 0)?;
    Ok(term
        .eval(RawRow::unchecked(field.as_bytes()), &mut RowSplit::default(), "public.t", 0)?
        .is_true())
}

/// `op` over a `date` column, testing in the tiers `reach` reads and read
/// as the null mode reads it there, or tested by nothing.
pub(super) fn date_test(op: PredicateOp, reach: Option<UnrepresentableTier>) -> ResolvedExpr {
    let predicate = Predicate { column: "v".into(), op };
    let resolved = one_column("date", DataType::Date32);
    let term = resolve_term(&predicate, 0, &resolved, 0).unwrap();
    let reads = [reach.and_then(|reach| scalar_read(&DataType::Date32, reach))];
    ResolvedExpr::Term(term).reading(&reads).testing(&reads)
}

/// The row every tree test below is evaluated over: field 0 holds `1` and
/// field 1 is NULL, so a leaf of each truth value is built out of real
/// terms rather than out of a constant.
pub(super) const THREE_VALUED_ROW: &[u8] = b"1\t\\N";

/// A leaf that evaluates to `want` over [`THREE_VALUED_ROW`]: `f0 = 1`,
/// `f0 = 2`, and `f1 = 1` against the NULL field.
pub(super) fn leaf(want: Truth) -> ResolvedExpr {
    let (index, value) = match want {
        Truth::True => (0, "1"),
        Truth::False => (0, "2"),
        Truth::Unknown => (1, "1"),
    };
    let p = Predicate { column: "v".into(), op: PredicateOp::Eq(value.into()) };
    ResolvedExpr::Term(text_term(&p, index))
}

pub(super) fn exact(expr: &ResolvedExpr) -> Truth {
    expr.eval(true, RawRow::unchecked(THREE_VALUED_ROW), &mut RowSplit::default(), "public.t", 0)
        .unwrap()
}

/// One column's statistics for one row group, stated by hand — the
/// shape [`GroupStatistics`] reads, with every column index answered
/// alike.
#[derive(Debug, Clone, Default)]
pub(super) struct Group {
    pub(super) rows: u64,
    pub(super) nulls: Option<u64>,
    pub(super) bounds: Option<(String, String)>,
    pub(super) dictionary: Option<Vec<String>>,
}

impl GroupStatistics for Group {
    fn rows(&self) -> u64 {
        self.rows
    }
    fn null_count(&self, _: usize) -> Option<u64> {
        self.nulls
    }
    fn bounds(&self, _: usize, _: &CompareKind) -> Option<(&str, &str)> {
        self.bounds.as_ref().map(|(min, max)| (min.as_str(), max.as_str()))
    }
    fn dictionary(&self, _: usize) -> Option<impl Iterator<Item = &str>> {
        self.dictionary.as_ref().map(|entries| entries.iter().map(String::as_str))
    }
    fn unrepresentable(&self, _: usize) -> Option<Unrepresentable> {
        None
    }
}

pub(super) fn sets(truths: &[Truth]) -> TruthSet {
    truths.iter().fold(TruthSet::EMPTY, |set, t| set.union(TruthSet::of(*t)))
}

/// A two-column schema, both `integer`, for the tests about *where* a
/// decode failure surfaces.
pub(super) fn two_integers() -> ResolvedSchema {
    let note = |name: &str| ColumnNote {
        column: name.into(),
        declared: Some("integer".into()),
        resolution: ColumnResolution::Mapped,
    };
    ResolvedSchema {
        schema: Arc::new(Schema::new(vec![
            Field::new("a", DataType::Int32, true),
            Field::new("b", DataType::Int32, true),
        ])),
        columns: vec![ColumnResolution::Mapped; 2],
        notes: vec![note("a"), note("b")],
        plans: vec![NestedPlan::Scalar; 2],
        comparisons: vec![comparison_for("integer", None, &[], &[]); 2],
        not_null: vec![false; 2],
    }
}

/// `column IN (values)` over `schema`'s column `index`, in PostgreSQL's
/// semantics, `None` in `values` a NULL.
pub(super) fn membership(
    schema: &ResolvedSchema,
    index: usize,
    values: &[Option<&str>],
) -> Result<ResolvedMembership> {
    let column = schema.schema.field(index).name().clone();
    let values: Vec<Option<String>> = values.iter().map(|v| v.map(str::to_string)).collect();
    resolve_membership(&column, &values, "IN", index, schema, 0, ComparisonSemantics::Postgres)
}

/// A tree's exact value over `row`, as a `Not` above it asks for it.
pub(super) fn exact_over(expr: &ResolvedExpr, row: &[u8]) -> Result<Truth> {
    expr.eval(true, RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
}
