//! Post-parse row filtering (`docs/design/architecture.md`, "Predicates").

use std::cmp::Ordering;

use arrow::datatypes::{DataType, TimeUnit, i256};

use crate::copy::{decode_field, split_fields};
use crate::decode;
use crate::pgtype::{NestedPlan, split_typmod};
use crate::resolve::{ColumnResolution, ResolvedSchema};
use crate::{Error, Result};

/// Comparison operator for [`Predicate`].
///
/// `Eq`/`Ne` compare the field as an unparsed string; the four ordering
/// operators compare **typed**, decoding both the field and the filter's own
/// literal with the column's resolved decoder and comparing the values
/// (`docs/design/architecture.md`, "Predicates"). That split is why a
/// nested column still answers `Eq`/`Ne` and refuses `<`: every value in a
/// dump is already in canonical `*_out` form, so a string comparison against
/// it is right, while an *order* over a composite or an array literal is not
/// a thing this layer can define.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOp {
    Eq,
    Ne,
    /// `col IS NULL` — matches only a NULL field (`\N`). Rounds out the NULL
    /// semantics `Eq`/`Ne` deliberately can't express (see [`Predicate::value`]).
    IsNull,
    /// `col IS NOT NULL` — matches every non-NULL field.
    IsNotNull,
    Lt,
    Le,
    Gt,
    Ge,
}

impl PredicateOp {
    /// Whether this operator compares by the column's own order rather than
    /// as text — the four that need a `Mapped` column with a
    /// [`NestedPlan::Scalar`] plan.
    pub fn is_ordering(self) -> bool {
        matches!(self, Self::Lt | Self::Le | Self::Gt | Self::Ge)
    }

    /// How an error message names this operator — the same spelling
    /// `pgdq query --filter` accepts.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::IsNull => "IS NULL",
            Self::IsNotNull => "IS NOT NULL",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// A single-column post-parse filter: `column <op> value`, and one **term**
/// of a conjunction — a query carries a list of these and keeps a row only
/// if every one of them matches (`QueryOptions::filters`). `column` is
/// matched against the queried table's column names (the `COPY` header list,
/// or the `column1`, `column2`, ... placeholders used when the header has
/// none). `value` is `None` for `IsNull`/`IsNotNull`, which need no
/// comparison value; it is always `Some` for every other operator.
///
/// **How `value` is compared depends on the operator.** `Eq`/`Ne` compare it
/// against each row's decoded (unescaped) field as a plain string. The four
/// ordering operators decode it once, when the block's schema resolves, with
/// the column's own decoder, and compare decoded values — so both sides of a
/// `numeric(p,s)` comparison carry that column's scale, and a literal that
/// does not parse as the column's type is `Error::PredicateValueDecode`
/// before any row is read.
///
/// A NULL field matches nothing at all — not `Eq`, not `Ne`, and not an
/// ordering operator — because SQL's own three-valued logic collapses
/// unknown to "excluded", which is exactly why `IsNull`/`IsNotNull` exist:
/// without them there is no way to ask for a NULL explicitly
/// (`docs/status/history/2026-08-22.md`). That collapse is what bounds the
/// conjunction to `AND`: it is sound under `AND` and unsound under `NOT`,
/// which is why `OR`/`NOT` are deferred rather than added alongside
/// (`docs/design/architecture.md`, "Predicates").
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: Option<String>,
}

/// Whether an ordering comparison on a column of a given Arrow type orders
/// values the way PostgreSQL's own operator for that column's type does —
/// the *ordering register*, rendered as a table in
/// `docs/design/architecture.md`, "Predicates".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderingSupport {
    /// This comparison and PostgreSQL's agree on every value a dump can hold.
    Agrees,
    /// Both orders are total and they are not the same order.
    Diverges(OrderingDivergence),
    /// No ordering is defined for this Arrow type here at all.
    Refused,
}

/// How an ordering comparison differs from PostgreSQL's, keyed by the Arrow
/// type — which is what the code can key on. The *declared* PostgreSQL type
/// then sharpens the sentence a user reads, since several declared types
/// reach `Utf8View` for different reasons (see [`OrderingNote::message`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderingDivergence {
    /// The column is held as text (`Utf8View`) and compared bytewise.
    AsText,
    /// An enum, compared by label text where PostgreSQL uses declaration
    /// order.
    EnumLabels,
}

/// One column of one query whose ordering comparison diverges from
/// PostgreSQL's — reported per stream by
/// `crate::stream::TableStream::ordering_notes`.
///
/// **Neither a `Diagnostic` nor a [`crate::resolve::ColumnNote`]**, and
/// deliberately: `DumpIndex.diagnostics` is the L1 file-level channel and
/// `ResolvedSchema.notes` is the L2 per-column one, while this is per-column
/// *and* conditional on a predicate — L4. Writing it into either would
/// invert the layering (`docs/design/layering.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderingNote {
    pub column: String,
    /// The declared PostgreSQL type, as the DDL spelled it.
    pub declared_type: String,
    pub divergence: OrderingDivergence,
}

impl OrderingNote {
    /// One sentence naming the column and what its order is not.
    ///
    /// The `AsText` wording is chosen from the *declared* type because three
    /// unrelated situations share `Utf8View`: a text type (PostgreSQL orders
    /// by collation, which a plain dump does not record — I32), a bare
    /// `numeric` (no Arrow decimal representation, so it orders
    /// lexicographically), and everything else held as text, which has a
    /// server-side operator of its own that this does not implement.
    pub fn message(&self) -> String {
        let column = &self.column;
        let declared = &self.declared_type;
        match self.divergence {
            OrderingDivergence::EnumLabels => format!(
                "`{column}` ({declared}) is an enum compared by label text, but PostgreSQL orders \
                 an enum by declaration order"
            ),
            OrderingDivergence::AsText => {
                let why = match split_typmod(declared).0.to_ascii_lowercase().as_str() {
                    "text" | "character varying" | "character" | "name" => {
                        "PostgreSQL orders text by collation, which a plain dump does not record, \
                         so this matches the server only under C/POSIX"
                    }
                    "numeric" => {
                        "an unconstrained `numeric` has no decimal representation here, so `9` \
                         sorts after `10`"
                    }
                    _ => "PostgreSQL orders this type by its own operator, not bytewise",
                };
                format!("`{column}` ({declared}) is compared bytewise as text: {why}")
            }
        }
    }
}

/// How one column's field text becomes a comparable value. Produced only by
/// [`ordering_register`], so it cannot name a type that register refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderKind {
    Bool,
    Int,
    Float32,
    Float64,
    Decimal(i8),
    Date,
    Time,
    Timestamp { with_tz: bool },
    Uuid,
    Bytea,
    Text,
}

/// One side of an ordering comparison, decoded from text per the column's
/// [`OrderKind`]. Both sides of any one comparison come from the same kind,
/// so a variant mismatch is unreachable by construction.
#[derive(Debug, Clone, PartialEq)]
enum OrderKey {
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(i256),
    Bytes(Vec<u8>),
    Text(String),
}

/// **The ordering register, in code**, and the authority the Markdown table
/// in `docs/design/architecture.md`, "Predicates", renders for humans.
///
/// Exhaustive over `DataType` with **no wildcard arm**, so a type this build
/// starts producing cannot silently inherit a classification: the arm it
/// falls into has to be written down. That is the check the register needs,
/// because the moment it would otherwise go stale is a routine type-mapping
/// change in `pgtype.rs`, and it fires there as a compile error rather than
/// as a test someone might not run.
///
/// *Rejected: a test asserting the Markdown table and this function agree row
/// for row.* Its own failure mode is bit-rot in the doc parser, and the table
/// is small enough to be re-read whenever the register changes.
fn ordering_register(data_type: &DataType) -> (OrderingSupport, Option<OrderKind>) {
    use OrderingSupport::{Agrees, Diverges, Refused};
    match data_type {
        DataType::Boolean => (Agrees, Some(OrderKind::Bool)),
        // `false < true`, PostgreSQL's own boolean order.
        DataType::Int16 | DataType::Int32 | DataType::Int64 => (Agrees, Some(OrderKind::Int)),
        DataType::Float32 => (Agrees, Some(OrderKind::Float32)),
        DataType::Float64 => (Agrees, Some(OrderKind::Float64)),
        // Both sides carry the column's own scale, because the literal is
        // decoded with the column's own decoder, so unscaled integers compare
        // directly.
        DataType::Decimal128(_, scale) | DataType::Decimal256(_, scale) => {
            (Agrees, Some(OrderKind::Decimal(*scale)))
        }
        DataType::Date32 => (Agrees, Some(OrderKind::Date)),
        DataType::Time64(TimeUnit::Microsecond) => (Agrees, Some(OrderKind::Time)),
        // Compared as the stored instant, which is what PostgreSQL compares
        // for both `timestamp` and `timestamptz`.
        DataType::Timestamp(TimeUnit::Microsecond, tz) => {
            (Agrees, Some(OrderKind::Timestamp { with_tz: tz.is_some() }))
        }
        // `uuid_internal_cmp` is `memcmp` over 16 bytes, and `byteacmp` is
        // `memcmp` then length — both are `[u8]`'s own order (I33).
        DataType::FixedSizeBinary(16) => (Agrees, Some(OrderKind::Uuid)),
        DataType::Binary => (Agrees, Some(OrderKind::Bytea)),
        DataType::Dictionary(key, value)
            if **key == DataType::Int32 && **value == DataType::Utf8 =>
        {
            (Diverges(OrderingDivergence::EnumLabels), Some(OrderKind::Text))
        }
        DataType::Utf8View => (Diverges(OrderingDivergence::AsText), Some(OrderKind::Text)),
        // Nothing below is produced by `pgtype.rs`'s mapping table with a
        // `NestedPlan::Scalar` plan. The list is written out rather than
        // wildcarded so that a new mapping has to choose an arm.
        DataType::Null
        | DataType::Int8
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::Float16
        | DataType::Timestamp(_, _)
        | DataType::Date64
        | DataType::Time32(_)
        | DataType::Time64(_)
        | DataType::Duration(_)
        | DataType::Interval(_)
        | DataType::FixedSizeBinary(_)
        | DataType::LargeBinary
        | DataType::BinaryView
        | DataType::Utf8
        | DataType::LargeUtf8
        | DataType::List(_)
        | DataType::ListView(_)
        | DataType::FixedSizeList(_, _)
        | DataType::LargeList(_)
        | DataType::LargeListView(_)
        | DataType::Struct(_)
        | DataType::Union(_, _)
        | DataType::Dictionary(_, _)
        | DataType::Decimal32(_, _)
        | DataType::Decimal64(_, _)
        | DataType::Map(_, _)
        | DataType::RunEndEncoded(_, _) => (Refused, None),
    }
}

/// Decode one already-COPY-unescaped value into a comparable key. `None`
/// when the text is not a value of that type — for a *field* that is
/// `Error::FieldDecode`, exactly as the typed build path reports it; for the
/// filter's own literal it is `Error::PredicateValueDecode`, raised before a
/// row is read.
fn order_key(kind: OrderKind, text: &str) -> Option<OrderKey> {
    Some(match kind {
        OrderKind::Bool => OrderKey::Bool(decode::decode_bool(text)?),
        // Parsed as `i64` whatever the column's width: a literal outside a
        // `smallint`'s range still orders correctly against every value the
        // column can hold, and refusing it would be a refusal PostgreSQL's
        // own comparison does not need to make.
        OrderKind::Int => OrderKey::Int(text.parse::<i64>().ok()?),
        OrderKind::Float32 => OrderKey::Float(f64::from(decode::decode_f32(text)?)),
        OrderKind::Float64 => OrderKey::Float(decode::decode_f64(text)?),
        OrderKind::Decimal(scale) => {
            OrderKey::Decimal(i256::from_string(&decode::decimal_unscaled_digits(text, scale)?)?)
        }
        OrderKind::Date => OrderKey::Int(decode::decode_date32(text)?.into()),
        OrderKind::Time => OrderKey::Int(decode::decode_time64_micros(text)?),
        OrderKind::Timestamp { with_tz } => {
            OrderKey::Int(decode::decode_timestamp_micros(text, with_tz)?)
        }
        OrderKind::Uuid => OrderKey::Bytes(decode::decode_uuid(text)?.to_vec()),
        OrderKind::Bytea => OrderKey::Bytes(decode::decode_bytea(text)?),
        OrderKind::Text => OrderKey::Text(text.to_string()),
    })
}

/// PostgreSQL's float order, not Rust's: `NaN` is greater than every other
/// value, infinities included, and `NaN = NaN` is true — `float8_gt(a, b)` is
/// `!isnan(b) && (isnan(a) || a > b)` (I33). Rust's `partial_cmp` answers
/// `None` for either case. `real`/`double precision` are the only columns whose
/// decoder yields a NaN at all: a `NaN` in a `numeric(p,s)` column has no
/// `Decimal128` representation and fails to decode long before any
/// comparison (I4).
fn pg_float_cmp(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).expect("neither side is NaN"),
    }
}

fn compare_keys(a: &OrderKey, b: &OrderKey) -> Ordering {
    match (a, b) {
        (OrderKey::Bool(x), OrderKey::Bool(y)) => x.cmp(y),
        (OrderKey::Int(x), OrderKey::Int(y)) => x.cmp(y),
        (OrderKey::Float(x), OrderKey::Float(y)) => pg_float_cmp(*x, *y),
        (OrderKey::Decimal(x), OrderKey::Decimal(y)) => x.cmp(y),
        (OrderKey::Bytes(x), OrderKey::Bytes(y)) => x.cmp(y),
        (OrderKey::Text(x), OrderKey::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ => unreachable!("both sides of a comparison decode through one column's `OrderKind`"),
    }
}

/// Everything an ordering term needs, settled once when the block's schema
/// resolves rather than per row.
#[derive(Debug, Clone)]
struct OrderTerm {
    column: String,
    kind: OrderKind,
    /// The filter's literal, decoded once with the column's own decoder.
    bound: OrderKey,
    /// The declared PostgreSQL type, for `Error::FieldDecode`'s context and
    /// for [`OrderingNote::message`].
    declared_type: String,
    divergence: Option<OrderingDivergence>,
}

/// One filter term resolved against one `COPY` block: the field index it
/// reads, plus — for an ordering operator only — the typed comparison it
/// will make.
///
/// The index is into the block's **unprojected** column list, because that is
/// what the raw row's fields are numbered by: a term may name a column the
/// projection dropped.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTerm {
    index: usize,
    order: Option<OrderTerm>,
}

impl ResolvedTerm {
    /// This term's divergence from PostgreSQL's own comparison, if it has
    /// one. `None` for every non-ordering term and for every ordering term
    /// whose type agrees.
    pub(crate) fn ordering_note(&self) -> Option<OrderingNote> {
        let order = self.order.as_ref()?;
        Some(OrderingNote {
            column: order.column.clone(),
            declared_type: order.declared_type.clone(),
            divergence: order.divergence?,
        })
    }
}

/// The refusal an ordering operator earns on a column that cannot carry one.
/// Three reasons, each a different fact about the column.
const NOT_MAPPED: &str = "the column's declared type did not resolve to an Arrow type, so it has no order of its own \
     (`--schema-mode strings` resolves no column, by design)";
const NESTED: &str = "the column is nested (array, composite, range or multirange), and an order over such a \
     literal is not defined here";
const NO_ORDER: &str = "this build defines no ordering for the column's resolved Arrow type";

/// Resolve one filter term against the block's **unprojected**
/// [`ResolvedSchema`], at `index` — the column's position, already looked up
/// by the caller.
///
/// A non-ordering term needs nothing else. An ordering term is refused here,
/// before a row of this block flows, unless the column resolved `Mapped` with
/// a [`NestedPlan::Scalar`] plan and its Arrow type is in the ordering
/// register; and its literal is decoded here too, so a value that is not of
/// the column's type is a fault reported once rather than a filter that
/// matches nothing.
pub(crate) fn resolve_term(
    predicate: &Predicate,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
) -> Result<ResolvedTerm> {
    if !predicate.op.is_ordering() {
        return Ok(ResolvedTerm { index, order: None });
    }
    let refuse = |reason: &'static str| Error::UnorderedPredicateColumn {
        header_offset,
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        reason,
    };
    if resolved.columns[index] != ColumnResolution::Mapped {
        return Err(refuse(NOT_MAPPED));
    }
    if resolved.plans[index] != NestedPlan::Scalar {
        return Err(refuse(NESTED));
    }
    let (support, kind) = ordering_register(resolved.schema.field(index).data_type());
    let Some(kind) = kind else {
        return Err(refuse(NO_ORDER));
    };
    let declared_type = resolved.notes[index].declared.clone().unwrap_or_default();
    // `value` is `Some` for every operator but the two NULL tests; an
    // embedder that builds a `Gt` term without one gets the same fault as an
    // unparseable literal, named the same way.
    let text = predicate.value.as_deref().unwrap_or_default();
    let bound = order_key(kind, text).ok_or_else(|| Error::PredicateValueDecode {
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        value: text.to_string(),
        declared_type: declared_type.clone(),
    })?;
    Ok(ResolvedTerm {
        index,
        order: Some(OrderTerm {
            column: predicate.column.clone(),
            kind,
            bound,
            declared_type,
            divergence: match support {
                OrderingSupport::Diverges(d) => Some(d),
                OrderingSupport::Agrees | OrderingSupport::Refused => None,
            },
        }),
    })
}

impl Predicate {
    /// Evaluate this predicate against `raw_row`'s field, per `term` — the
    /// resolution of *this* predicate against the block being replayed.
    /// `table` and `row_offset` are context for the one error this can
    /// raise: a field that does not decode as its mapped type under an
    /// ordering operator, which is `Error::FieldDecode`, worded exactly as
    /// the typed build path words it.
    pub(crate) fn matches(
        &self,
        raw_row: &[u8],
        term: &ResolvedTerm,
        table: &str,
        row_offset: u64,
    ) -> Result<bool> {
        let field = split_fields(raw_row).nth(term.index);
        let decoded = match field {
            Some(f) => decode_field(f)?,
            None => None,
        };
        Ok(match self.op {
            PredicateOp::IsNull => decoded.is_none(),
            PredicateOp::IsNotNull => decoded.is_some(),
            PredicateOp::Eq => decoded.is_some_and(|v| Some(v.as_ref()) == self.value.as_deref()),
            PredicateOp::Ne => decoded.is_some_and(|v| Some(v.as_ref()) != self.value.as_deref()),
            PredicateOp::Lt | PredicateOp::Le | PredicateOp::Gt | PredicateOp::Ge => {
                let order = term
                    .order
                    .as_ref()
                    .expect("an ordering predicate resolves to a term carrying its comparison");
                match decoded {
                    // A NULL compares to nothing: unknown collapses to false,
                    // exactly as it does under `Eq`/`Ne`.
                    None => false,
                    Some(text) => {
                        let key =
                            order_key(order.kind, &text).ok_or_else(|| Error::FieldDecode {
                                table: table.to_string(),
                                column: order.column.clone(),
                                row_offset,
                                declared_type: order.declared_type.clone(),
                                value: text.to_string(),
                            })?;
                        let ord = compare_keys(&key, &order.bound);
                        match self.op {
                            PredicateOp::Lt => ord.is_lt(),
                            PredicateOp::Le => ord.is_le(),
                            PredicateOp::Gt => ord.is_gt(),
                            _ => ord.is_ge(),
                        }
                    }
                }
            }
        })
    }
}

/// Evaluate a conjunction against `raw_row`: every term must match.
/// `terms[i]` is term `i` resolved against the block's **unprojected**
/// schema, so the two slices are parallel by construction
/// (`docs/design/architecture.md`, "Predicates").
///
/// An empty conjunction matches every row, which is what makes "no filter"
/// need no separate case anywhere above this.
///
/// Terms are tested in the order they were given and the walk stops at the
/// first that fails, so the ordinary case costs one pass over the row. Each
/// term does walk the row itself — there is no shared pass — which is a real
/// cost only for a conjunction whose leading terms nearly always pass, and
/// which buys the short-circuit for the common shape.
pub(crate) fn matches_all(
    filters: &[Predicate],
    terms: &[ResolvedTerm],
    raw_row: &[u8],
    table: &str,
    row_offset: u64,
) -> Result<bool> {
    for (filter, term) in filters.iter().zip(terms) {
        if !filter.matches(raw_row, term, table, row_offset)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::datatypes::{Field, Schema};

    use super::*;
    use crate::resolve::ColumnNote;

    /// A term with no typed comparison behind it — what every `Eq`/`Ne`/NULL
    /// test resolves to.
    fn text_term(index: usize) -> ResolvedTerm {
        ResolvedTerm { index, order: None }
    }

    fn matches(p: &Predicate, raw_row: &[u8], index: usize) -> bool {
        p.matches(raw_row, &text_term(index), "public.t", 0).unwrap()
    }

    /// A one-column `ResolvedSchema` for `declared`/`data_type`, mapped and
    /// scalar — the shape an ordering term is allowed on.
    fn one_column(declared: &str, data_type: DataType) -> ResolvedSchema {
        ResolvedSchema {
            schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
            columns: vec![ColumnResolution::Mapped],
            notes: vec![ColumnNote {
                column: "v".into(),
                declared: Some(declared.into()),
                resolution: ColumnResolution::Mapped,
            }],
            plans: vec![NestedPlan::Scalar],
        }
    }

    fn order_predicate(op: PredicateOp, value: &str) -> Predicate {
        Predicate { column: "v".into(), op, value: Some(value.into()) }
    }

    /// Resolve `op value` against a one-column schema and evaluate it over a
    /// single-field row holding `field`.
    fn ordered(
        declared: &str,
        data_type: DataType,
        op: PredicateOp,
        value: &str,
        field: &str,
    ) -> Result<bool> {
        let p = order_predicate(op, value);
        let term = resolve_term(&p, 0, &one_column(declared, data_type), 0)?;
        p.matches(field.as_bytes(), &term, "public.t", 0)
    }

    #[test]
    fn eq_matches_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a\tb".into()) };
        assert!(matches(&p, b"other\ta\\tb", 1));
        assert!(!matches(&p, b"other\tc", 1));
    }

    #[test]
    fn ne_matches_everything_but_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(matches(&p, b"other\tb", 1));
        assert!(!matches(&p, b"other\ta", 1));
    }

    #[test]
    fn null_matches_neither_eq_nor_ne() {
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
        let ne = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(!matches(&eq, b"other\t\\N", 1));
        assert!(!matches(&ne, b"other\t\\N", 1));
    }

    #[test]
    fn is_null_and_is_not_null() {
        let is_null = Predicate { column: "x".into(), op: PredicateOp::IsNull, value: None };
        let is_not_null = Predicate { column: "x".into(), op: PredicateOp::IsNotNull, value: None };
        assert!(matches(&is_null, b"other\t\\N", 1));
        assert!(!matches(&is_null, b"other\ta", 1));
        assert!(!matches(&is_not_null, b"other\t\\N", 1));
        assert!(matches(&is_not_null, b"other\ta", 1));
    }

    #[test]
    fn an_empty_conjunction_matches_every_row() {
        assert!(matches_all(&[], &[], b"a\tb", "public.t", 0).unwrap());
    }

    #[test]
    fn every_term_must_match() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Eq, value: Some("1".into()) },
            Predicate { column: "b".into(), op: PredicateOp::Eq, value: Some("2".into()) },
        ];
        let terms = [text_term(0), text_term(1)];
        assert!(matches_all(&filters, &terms, b"1\t2", "public.t", 0).unwrap());
        assert!(!matches_all(&filters, &terms, b"1\t3", "public.t", 0).unwrap());
        assert!(!matches_all(&filters, &terms, b"9\t2", "public.t", 0).unwrap());
    }

    /// Two terms on one column are an ordinary conjunction, and a
    /// contradictory pair simply matches nothing — no term is special-cased.
    #[test]
    fn two_terms_may_name_the_same_column() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("1".into()) },
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("2".into()) },
        ];
        let terms = [text_term(0), text_term(0)];
        assert!(matches_all(&filters, &terms, b"3", "public.t", 0).unwrap());
        assert!(!matches_all(&filters, &terms, b"2", "public.t", 0).unwrap());
    }

    #[test]
    fn missing_column_index_is_treated_as_null() {
        // Can't happen once a caller resolves the index from the block's own
        // schema, but the fallback is still exercised here.
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(!matches(&p, b"onlyone", 5));
    }

    /// The four operators over the boundary itself — the case a `<` / `<=`
    /// pair differs on, and the one an off-by-one would pass.
    #[test]
    fn the_four_operators_differ_only_at_the_boundary() {
        for (op, below, at, above) in [
            (PredicateOp::Lt, true, false, false),
            (PredicateOp::Le, true, true, false),
            (PredicateOp::Gt, false, false, true),
            (PredicateOp::Ge, false, true, true),
        ] {
            for (field, expected) in [("4", below), ("5", at), ("6", above)] {
                assert_eq!(
                    ordered("integer", DataType::Int32, op, "5", field).unwrap(),
                    expected,
                    "{} {} 5",
                    field,
                    op.symbol()
                );
            }
        }
    }

    /// Numbers order as numbers, which is the whole point: the text
    /// comparison `Eq` uses would put `9` after `10`.
    #[test]
    fn integers_order_numerically_not_lexicographically() {
        assert!(ordered("integer", DataType::Int32, PredicateOp::Gt, "9", "10").unwrap());
        assert!(!ordered("integer", DataType::Int32, PredicateOp::Lt, "9", "10").unwrap());
    }

    /// A `numeric(10,2)` literal and field are both taken to the column's
    /// scale before comparing, so `1.5` and `1.50` are one value.
    #[test]
    fn a_decimal_compares_at_the_columns_scale() {
        let t = DataType::Decimal128(10, 2);
        assert!(ordered("numeric(10,2)", t.clone(), PredicateOp::Ge, "1.5", "1.50").unwrap());
        assert!(ordered("numeric(10,2)", t.clone(), PredicateOp::Le, "1.5", "1.50").unwrap());
        assert!(ordered("numeric(10,2)", t, PredicateOp::Lt, "1.5", "-1.50").unwrap());
    }

    /// A literal finer than the column's scale is refused rather than
    /// rounded: it is decoded with the column's own decoder, and that decoder
    /// does not drop non-zero digits.
    #[test]
    fn a_literal_finer_than_the_columns_scale_is_refused() {
        let err =
            ordered("numeric(10,2)", DataType::Decimal128(10, 2), PredicateOp::Gt, "1.005", "1.00")
                .unwrap_err();
        assert!(
            matches!(err, Error::PredicateValueDecode { ref value, .. } if value == "1.005"),
            "{err:?}"
        );
    }

    /// PostgreSQL's NaN, not Rust's: greater than everything, equal to
    /// itself, and never `None`.
    #[test]
    fn nan_is_the_largest_float_and_equals_itself() {
        let f = |op, value, field| {
            ordered("double precision", DataType::Float64, op, value, field).unwrap()
        };
        assert!(f(PredicateOp::Gt, "Infinity", "NaN"));
        assert!(f(PredicateOp::Ge, "NaN", "NaN"));
        assert!(f(PredicateOp::Le, "NaN", "NaN"));
        assert!(!f(PredicateOp::Gt, "NaN", "NaN"));
        assert!(f(PredicateOp::Lt, "NaN", "-Infinity"));
    }

    /// A NULL field is excluded by every ordering operator, the same collapse
    /// `Eq`/`Ne` make.
    #[test]
    fn a_null_field_matches_no_ordering_operator() {
        for op in [PredicateOp::Lt, PredicateOp::Le, PredicateOp::Gt, PredicateOp::Ge] {
            assert!(!ordered("integer", DataType::Int32, op, "0", "\\N").unwrap());
        }
    }

    /// A field that is not a value of its mapped type is the same fault the
    /// typed build path reports, with the same wording and the same escape.
    #[test]
    fn an_undecodable_field_is_a_field_decode_error() {
        let err = ordered("integer", DataType::Int32, PredicateOp::Gt, "0", "twelve").unwrap_err();
        assert!(
            matches!(err, Error::FieldDecode { ref column, ref value, .. }
                if column == "v" && value == "twelve"),
            "{err:?}"
        );
    }

    /// A literal that is not a value of the column's type is refused when the
    /// block's schema resolves — before a row is read, and once rather than
    /// per row.
    #[test]
    fn an_undecodable_literal_is_refused_at_resolution() {
        let p = order_predicate(PredicateOp::Gt, "twelve");
        let err = resolve_term(&p, 0, &one_column("integer", DataType::Int32), 0).unwrap_err();
        assert!(
            matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v"),
            "{err:?}"
        );
    }

    /// Refusal is by resolution and plan, and the three reasons are distinct
    /// facts about the column rather than one catch-all.
    #[test]
    fn an_ordering_operator_is_refused_off_a_mapped_scalar_column() {
        let p = order_predicate(PredicateOp::Gt, "1");

        let mut unmapped = one_column("mystery", DataType::Utf8View);
        unmapped.columns[0] = ColumnResolution::UnknownType;
        assert!(matches!(
            resolve_term(&p, 0, &unmapped, 7).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, header_offset: 7, .. } if reason == NOT_MAPPED
        ));

        let mut nested = one_column("integer[]", DataType::Utf8View);
        nested.plans[0] = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        assert!(matches!(
            resolve_term(&p, 0, &nested, 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NESTED
        ));

        // A `Mapped` scalar column whose Arrow type the register refuses:
        // unreachable from the mapping table today, and refused anyway.
        assert!(matches!(
            resolve_term(&p, 0, &one_column("mystery", DataType::UInt8), 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NO_ORDER
        ));
    }

    /// Only the two divergent classifications produce a note, and the
    /// sentence is chosen from the declared type rather than the Arrow one.
    #[test]
    fn a_divergent_comparison_produces_a_note_naming_why() {
        let note = |declared, data_type, literal| {
            let p = order_predicate(PredicateOp::Gt, literal);
            resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap().ordering_note()
        };

        assert_eq!(note("integer", DataType::Int32, "1"), None, "an agreeing type says nothing");

        let text = note("character varying(10)", DataType::Utf8View, "a").unwrap();
        assert_eq!(text.divergence, OrderingDivergence::AsText);
        assert!(text.message().contains("collation"), "{}", text.message());

        let numeric = note("numeric", DataType::Utf8View, "10").unwrap();
        assert!(numeric.message().contains("unconstrained"), "{}", numeric.message());

        let other = note("interval", DataType::Utf8View, "1 day").unwrap();
        assert!(other.message().contains("its own operator"), "{}", other.message());

        let enumerated = note(
            "public.mood",
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            "sad",
        )
        .unwrap();
        assert_eq!(enumerated.divergence, OrderingDivergence::EnumLabels);
        assert!(enumerated.message().contains("declaration order"), "{}", enumerated.message());
    }

    /// `uuid` and `bytea` compare as their bytes, which is what
    /// `uuid_internal_cmp` and `byteacmp` do.
    #[test]
    fn uuid_and_bytea_compare_bytewise() {
        assert!(
            ordered(
                "uuid",
                DataType::FixedSizeBinary(16),
                PredicateOp::Gt,
                "00000000-0000-0000-0000-000000000000",
                "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
            )
            .unwrap()
        );
        // `\x00ff` is COPY-escaped in the row, and sorts after `\x00`:
        // `memcmp` ties, then the longer value wins.
        assert!(ordered("bytea", DataType::Binary, PredicateOp::Gt, "\\x00", "\\\\x00ff").unwrap());
    }

    /// Dates and timestamps compare as the instant they decode to, so a
    /// BC date is below every AD one however its text sorts.
    #[test]
    fn dates_compare_as_instants() {
        assert!(
            ordered("date", DataType::Date32, PredicateOp::Lt, "2024-01-01", "0044-01-01 BC")
                .unwrap()
        );
        assert!(
            ordered(
                "timestamp with time zone",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                PredicateOp::Lt,
                "2024-01-01 00:00:00+00",
                "2023-12-31 18:30:00+00",
            )
            .unwrap()
        );
    }
}
