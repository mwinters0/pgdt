//! Post-parse row filtering (`docs/design/decisions.md`, "Predicates").

use crate::copy::{RawRow, RowSplit};
use crate::decode;
#[cfg(feature = "introspect")]
use crate::instrument::timed_span;
use crate::instrument::{EvaluationPart as Part, timed};
use crate::key::{
    NestedKey, OrderKey, Side, accepted_form, compare_keys, compare_nested, int_in, literal_key,
    nested_accepted_form, nested_key, order_key, read_key, render_macaddr, special_order_key,
};
use crate::model::{Finding, PostgresInvalidValues, Severity, Unrepresentable};
use crate::pgtype::{
    CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics, NestedCompare,
    UnanswerableReason, datafusion_position_divergences,
};
use crate::resolve::{ColumnResolution, ResolvedSchema};
use crate::unrepresentable::UnrepresentableRead;
use crate::{Error, Result};

/// Comparison operator for [`Predicate`].
///
/// Every operator but the NULL and unrepresentable tests compares typed,
/// through the column's own [`ComparisonPlan`]: the four ordering operators decode both sides and
/// compare the values, `Eq`/`Ne` take the cheapest of three canonicalizations
/// that gives the server's answer for that column ([`equality_comparison`]).
/// A nested column with a plan is compared structurally under every operator,
/// and a query's DataFusion semantics refuses one under every operator instead.
/// Where the register has no plan for a column — it did not resolve, one of
/// its nested positions is not compared, or this build orders its type not at
/// all — `Eq`/`Ne` compare the canonical `*_out` text the file holds and the
/// ordering operators are refused, where a query's DataFusion semantics orders a
/// scalar one bytewise instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOp {
    Eq,
    Ne,
    /// `col IS NULL` — matches a NULL field (`\N`), and under
    /// [`crate::UnrepresentableMode::Null`] a value the query reads as NULL,
    /// which [`Self::IsUnrepresentable`] tells apart. Rounds out the NULL
    /// semantics `Eq`/`Ne` deliberately can't express (see [`Predicate::value`]).
    IsNull,
    /// `col IS NOT NULL` — matches every field [`Self::IsNull`] does not.
    IsNotNull,
    /// `col IS UNREPRESENTABLE` — matches a value PostgreSQL accepts for the
    /// column's **declared** type and the query's front end cannot hold
    /// (`docs/design/decisions.md`, "D101"): past Arrow's format spec in
    /// PostgreSQL's semantics, past the engine's calendar too in DataFusion's
    /// — exactly the values [`crate::UnrepresentableMode::Null`] reads as
    /// NULL there. Two-valued, as `IsNull` is, and takes no value; answered
    /// under every [`crate::UnrepresentableMode`], and refused under
    /// [`crate::SchemaMode::Strings`], which reads no declared type.
    IsUnrepresentable,
    /// `col IS NOT UNREPRESENTABLE` — matches every field
    /// [`Self::IsUnrepresentable`] does not, a NULL field among them.
    IsNotUnrepresentable,
    Lt,
    Le,
    Gt,
    Ge,
    /// `col IS DISTINCT FROM <value>` — [`Self::Ne`] with NULL counted as a
    /// value rather than as unknown, so a NULL field answers [`Truth::True`]
    /// where `!=` answers [`Truth::Unknown`]
    /// (`docs/design/decisions.md`, "D53").
    IsDistinctFrom,
    /// `col IS NOT DISTINCT FROM <value>` — [`Self::Eq`] with the same NULL
    /// rule, answering [`Truth::False`] on a NULL field.
    IsNotDistinctFrom,
}

impl PredicateOp {
    /// Whether this operator compares by the column's own order rather than
    /// as text — the four that need, in PostgreSQL's semantics, a column
    /// compared as its declared type (`compares_as_declared`).
    pub fn is_ordering(self) -> bool {
        matches!(self, Self::Lt | Self::Le | Self::Gt | Self::Ge)
    }

    /// How an error message names this operator — the same spelling
    /// `pgdt query --filter` accepts.
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
            Self::IsDistinctFrom => "IS DISTINCT FROM",
            Self::IsNotDistinctFrom => "IS NOT DISTINCT FROM",
            Self::IsUnrepresentable => "IS UNREPRESENTABLE",
            Self::IsNotUnrepresentable => "IS NOT UNREPRESENTABLE",
        }
    }

    /// Whether this operator reads no comparison value: the NULL tests and
    /// the unrepresentable tests.
    pub fn takes_no_value(self) -> bool {
        matches!(
            self,
            Self::IsNull | Self::IsNotNull | Self::IsUnrepresentable | Self::IsNotUnrepresentable
        )
    }

    /// Whether this is one of the two unrepresentable tests.
    pub fn tests_unrepresentable(self) -> bool {
        matches!(self, Self::IsUnrepresentable | Self::IsNotUnrepresentable)
    }
}

/// A single-column post-parse filter: `column <op> value`, and the **leaf**
/// of an [`Expr`] — a query carries one expression and keeps a row only if
/// its root evaluates [`Truth::True`] (`QueryOptions::filter`). `column` is
/// matched against the queried table's column names, the `COPY` header's
/// list. `value` is `None` for the NULL and unrepresentable tests, which need
/// no comparison value ([`PredicateOp::takes_no_value`]); both parsers give
/// one for every other operator, and a `None` there is read as the empty
/// value.
///
/// `value` is read with the column's own decoder wherever the register gives
/// the column a comparison, whatever the operator, once when the block's schema
/// resolves rather than per row — so a literal that
/// is not a value of the column's type is `Error::PredicateValueDecode`
/// before any row is read, and both sides of a `numeric(p,s)` comparison held
/// as a decimal carry that column's scale. An ordering operator keeps the decoded key and
/// decodes the field to match; `Eq`/`Ne` usually keep the literal rendered
/// back into the `*_out` spelling the file holds and compare bytes
/// ([`equality_comparison`]).
///
/// A NULL field is [`Truth::Unknown`] under every comparing operator, and a
/// row survives only where the root is `True`. The operators that are
/// two-valued on a NULL field are `IsNull`/`IsNotNull`, which ask about the
/// NULL directly, `IsUnrepresentable`/`IsNotUnrepresentable`, which ask of a
/// field's text, and `IsDistinctFrom`/`IsNotDistinctFrom`, which count a NULL
/// as a value (`docs/design/decisions.md`, "D53").
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: Option<String>,
}

/// SQL's three-valued truth domain, which is what a filter evaluates in.
///
/// A row survives only if the expression's root is [`Truth::True`], so
/// `Unknown` and `False` are indistinguishable at the top — but not beneath
/// an [`Expr::Not`], since `NOT UNKNOWN` is `UNKNOWN`
/// (`docs/design/decisions.md`, "D54").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    Unknown,
}

impl Truth {
    /// `True` for `True` and nothing else — the question the row path asks.
    pub fn is_true(self) -> bool {
        matches!(self, Self::True)
    }

    /// `True`/`False`, for the comparisons that cannot be unknown once the
    /// field is known not to be NULL.
    fn of(b: bool) -> Self {
        if b { Self::True } else { Self::False }
    }

    /// SQL's `NOT`: `Unknown` is its own negation.
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// A row filter as a boolean expression tree over single-column
/// [`Predicate`] terms — what `QueryOptions::filter` carries
/// (`docs/design/decisions.md`, "D54").
///
/// `And` and `Or` are n-ary, the shape a repeated `--filter` builds. The
/// default is the empty conjunction, [`Expr::all`] over nothing, which every
/// row satisfies, so "no filter" is a degenerate tree rather than a case of
/// its own.
///
/// Nothing here is parsed: `Expr` is a tree an embedder builds variant by
/// variant, and the `--where` grammar that builds one from text lives in the
/// CLI (`docs/design/decisions.md`, "D60").
#[derive(Debug, Clone)]
pub enum Expr {
    Term(Predicate),
    /// `column IN (…)`, answering exactly as the [`Expr::Or`] of one
    /// [`PredicateOp::Eq`] term per value would — a `None` value as SQL's
    /// `= NULL`, which no [`Predicate`] spells — and `NOT IN` as [`Expr::Not`]
    /// over it — but finding and decoding its field once a row, and answering
    /// by one lookup however long the list (`docs/design/decisions.md`,
    /// "D53").
    In(Membership),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

/// The leaf [`Expr::In`] carries: a column and the values it is tested for
/// membership of, in the order written.
///
/// A value is read as a [`PredicateOp::Eq`] term's is, once per block, so a
/// literal that is not of the column's type is `Error::PredicateValueDecode`
/// for the first such value, naming the operator `IN`. **`None` is SQL's
/// `NULL`**, which no row equals: it never makes the membership true, and
/// wherever no other value matches it makes it [`Truth::Unknown`] rather than
/// `False` — so `NOT IN` over a list holding one keeps no row. A NULL field
/// is `Unknown` against any list with a value in it; the empty list is
/// `False` on every row, as the empty disjunction is, and a list holding
/// nothing but NULLs reads no field at all, as `= NULL` reads none.
#[derive(Debug, Clone)]
pub struct Membership {
    pub column: String,
    pub values: Vec<Option<String>>,
}

impl Expr {
    /// The conjunction of `terms` — the shape a repeated `--filter` builds.
    /// Over an empty iterator it is the filter that keeps every row.
    pub fn all(terms: impl IntoIterator<Item = Predicate>) -> Self {
        Self::And(terms.into_iter().map(Self::Term).collect())
    }
}

impl Expr {
    /// Whether this tree holds an unrepresentable test
    /// ([`PredicateOp::tests_unrepresentable`]).
    pub fn tests_unrepresentable(&self) -> bool {
        self.first_unrepresentable_test().is_some()
    }

    /// The first unrepresentable test in this tree, left to right.
    pub(crate) fn first_unrepresentable_test(&self) -> Option<&Predicate> {
        match self {
            Self::Term(predicate) => predicate.op.tests_unrepresentable().then_some(predicate),
            Self::In(_) => None,
            Self::And(children) | Self::Or(children) => {
                children.iter().find_map(Self::first_unrepresentable_test)
            }
            Self::Not(inner) => inner.first_unrepresentable_test(),
        }
    }
}

impl Default for Expr {
    fn default() -> Self {
        Self::all([])
    }
}

/// One term of one query whose comparison does not answer what PostgreSQL's
/// own operator for that column would — reported per stream by
/// `crate::stream::TableStream::comparison_notes`, on its own channel rather
/// than as a `Diagnostic` or a [`crate::resolve::ColumnNote`]
/// (`docs/design/decisions.md`, "D59", "D68"), and reaching a caller's
/// `crate::model::DiagnosticSink` beside them as a
/// [`crate::model::Finding`].
///
/// Per term, a divergence being operator-conditional
/// ([`ComparisonDivergence::affects_equality`]): a `text` column with no
/// `COLLATE` clause earns a note under `<` and none under `=`, so a query
/// filtering it with both operators carries one note. The same record is also
/// a column's, from [`column_divergences`], whatever terms a query names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonNote {
    pub column: String,
    /// The position *inside* the column the divergence is about, as the
    /// accessor a user would write — `[]` for an array's elements, `.label`
    /// for a composite's field, appended as the nesting descends. `None` is
    /// the column itself, which is every scalar column.
    ///
    /// A nested column can carry more than one, which is why a term yields a
    /// list of notes rather than one: `(label text, tags text[])` is on the
    /// database's own collation twice, once directly and once through an
    /// element.
    pub path: Option<String>,
    /// The declared PostgreSQL type **at that position**, as the DDL spelled
    /// it — the element's or the field's for a nested note, and the column's
    /// own where `path` is `None`.
    pub declared_type: String,
    pub divergence: ComparisonDivergence,
}

impl Finding for ComparisonNote {
    /// Always `Warning`: the rows a query returns are not the server's
    /// wherever the divergence reaches them.
    fn severity(&self) -> Severity {
        Severity::Warning
    }

    /// One sentence naming the column, its declared type, and what its
    /// comparison is not.
    fn message(&self) -> String {
        let column = format!("{}{}", self.column, self.path.as_deref().unwrap_or(""));
        // A column no DDL declared has no type to name.
        let subject = match self.declared_type.as_str() {
            "" => format!("`{column}`"),
            declared => format!("`{column}` ({declared})"),
        };
        let bytewise = |why: &str| format!("{subject} is compared bytewise: {why}");
        match self.divergence {
            // Not "PostgreSQL orders this differently": it does not order it
            // at all, and the sentence has to say which way the difference
            // runs.
            ComparisonDivergence::AsText => bytewise(
                "PostgreSQL defines no comparison for this type at all — no equality, no \
                 ordering, no operator class — so this comparison is one the server does not \
                 have",
            ),
            ComparisonDivergence::UnknownCollation => bytewise(
                "the column declares no COLLATE clause, so its collation is the database's, which \
                 a plain dump does not record — this matches the server only if that collation is \
                 C or POSIX",
            ),
            ComparisonDivergence::NonBytewiseCollation => bytewise(
                "the column declares a collation other than C/POSIX, and PostgreSQL orders it by \
                 that collation",
            ),
            // The one sentence reporting what the dump *said* rather than
            // what it left out, which is why it names equality outright.
            ComparisonDivergence::NonDeterministicCollation => bytewise(
                "the column declares a collation this dump declares non-deterministic, so \
                 PostgreSQL neither orders nor compares it byte for byte — two values spelled \
                 differently can be equal to the server",
            ),
            ComparisonDivergence::UnmodelledType => bytewise(
                "this build models no comparison for the column's declared type — the four \
                 ordering operators are refused on it, and PostgreSQL's own equality for such a \
                 type need not be a byte comparison (`box` compares areas)",
            ),
            // Not a `bytewise` sentence: the structure *is* compared the way
            // PostgreSQL compares it, and only a string leaf is left.
            ComparisonDivergence::JsonbStringCollation => format!(
                "{subject} is compared structurally, but every string value and object key inside \
                 it is ordered by the database's collation, which a plain dump does not record — \
                 this matches the server only if that collation is C or POSIX",
            ),
            ComparisonDivergence::LabelText => format!(
                "{subject} is compared by its labels' text, as DataFusion compares the emitted \
                 dictionary, where PostgreSQL orders an enum's labels as its type declares them"
            ),
            ComparisonDivergence::ValueAsText => bytewise(
                "DataFusion compares the text it is emitted as, where PostgreSQL compares it by \
                 value — the orders can differ, and so can equality: a literal matches only the \
                 spelling the server writes (`'12:00+00'` misses `12:00:00+00`, `'10.0.0.1/32'` \
                 misses an inet's `10.0.0.1`, uppercase hex misses a macaddr), and some types \
                 write one value two ways (`1.5` and `1.50`)",
            ),
            ComparisonDivergence::IntervalFields => format!(
                "{subject} is compared as DataFusion compares an Interval(MonthDayNano) — months, \
                 then days, then the time part — where PostgreSQL compares the whole span, so \
                 `1 mon` and `30 days` are equal to the server and not here"
            ),
            ComparisonDivergence::PaddedText => bytewise(
                "DataFusion compares the value with its blank padding, as it is emitted, where \
                 PostgreSQL ignores trailing blanks on both sides — so a literal matches only \
                 when padded to the column's width (`'abc'` misses `abc  ` in a character(5))",
            ),
            ComparisonDivergence::NestedOrder => format!(
                "{subject} is ordered as DataFusion orders the emitted list or struct, a NULL \
                 element or field first where PostgreSQL puts it last — an order that is not \
                 the type's and not one this build promises to match; equality is the \
                 server's but where a position inside it says otherwise"
            ),
            ComparisonDivergence::UnnormalizedZero => format!(
                "{subject} is compared as DataFusion compares a float inside a list or struct, \
                 by IEEE totalOrder with no sign dropped from zero, so `-0` is below `0` and \
                 unequal to it, where PostgreSQL equates them"
            ),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Every column's divergence from PostgreSQL's comparison under `semantics`,
/// as the notes a caller drains once, when a table is registered, rather than
/// per term (`docs/design/decisions.md`, "D59"). In column order, a nested
/// column's positions in walk order.
///
/// **Under [`ComparisonSemantics::Postgres`]** a column's notes are the union
/// of what a term on it would announce under any operator
/// ([`ResolvedTerm::comparison_notes`]): an operator refused on the column
/// adds nothing, having no answer to diverge.
///
/// **Under [`ComparisonSemantics::DataFusion`]** they say how DataFusion's
/// comparison of the emitted value differs from the server's, whether or not
/// this build answers a term on the column — `ORDER BY`, `MIN`/`MAX` and every
/// filter a plan keeps reach it too, so a nested column whose every term is
/// refused here still reports [`ComparisonDivergence::NestedOrder`] for
/// its order, and each position inside it its own divergence under its path
/// ([`crate::pgtype::NestedCompare::datafusion_divergences`]). A
/// kind [`CompareKind::datafusion_order`] moves reports
/// [`CompareKind::datafusion_divergence`], beside any collation the register
/// already announced; `jsonb`'s string collation is dropped, the whole value
/// being compared as text. A column that fell back to text reports nothing,
/// its `Warning` column note being the finding.
pub fn column_divergences(
    resolved: &ResolvedSchema,
    semantics: ComparisonSemantics,
) -> Vec<ComparisonNote> {
    let mut out = Vec::new();
    for (index, note) in resolved.notes.iter().enumerate() {
        let declared = note.declared.clone().unwrap_or_default();
        let positions = match semantics {
            ComparisonSemantics::Postgres => postgres_divergences(resolved, index, &declared),
            ComparisonSemantics::DataFusion => datafusion_divergences(resolved, index, &declared),
        };
        out.extend(positions.into_iter().map(|(path, declared_type, divergence)| ComparisonNote {
            column: note.column.clone(),
            path,
            declared_type,
            divergence,
        }));
    }
    out
}

/// Whether a column of `resolution` compares by its register plan: a
/// `Mapped` one, and one the untyped mode reads as its text, whose plan is
/// its declared type's in PostgreSQL's semantics and bytewise
/// ([`ComparisonPlan::Refused`]) in DataFusion's
/// ([`ColumnResolution::UnrepresentableValues`]).
fn compares_as_declared(resolution: &ColumnResolution) -> bool {
    matches!(resolution, ColumnResolution::Mapped | ColumnResolution::UnrepresentableValues)
}

/// One diverging position: its path inside the column (`None` for the column
/// itself), its declared type, and the divergence.
type DivergingPosition = (Option<String>, String, ComparisonDivergence);

/// [`column_divergences`] under PostgreSQL's semantics: the arms of
/// [`resolve_term`], each taken under both operator families.
fn postgres_divergences(
    resolved: &ResolvedSchema,
    index: usize,
    declared: &str,
) -> Vec<DivergingPosition> {
    let column = |divergence| vec![(None, declared.to_string(), divergence)];
    if !compares_as_declared(&resolved.columns[index]) {
        // Ordering refused; `=` compares the text, announced only where the
        // file named a type this build models nothing for.
        return match resolved.columns[index] {
            ColumnResolution::UnknownType | ColumnResolution::OpaqueBaseType => {
                column(ComparisonDivergence::UnmodelledType)
            }
            _ => Vec::new(),
        };
    }
    match &resolved.comparisons[index] {
        ComparisonPlan::Unanswerable(_) => Vec::new(),
        ComparisonPlan::Nested(tree) => {
            // A tree with an uncompared position answers only `=`/`!=`.
            let equality_only = tree.uncomparable().is_some();
            tree.divergences()
                .into_iter()
                .filter(|(_, _, d)| !equality_only || d.affects_equality())
                .map(|(path, declared, d)| (Some(path), declared, d))
                .collect()
        }
        _ if !resolved.plans[index].is_scalar() => Vec::new(),
        ComparisonPlan::Compared { divergence, .. } => divergence.map(column).unwrap_or_default(),
        ComparisonPlan::Refused => Vec::new(),
    }
}

/// [`column_divergences`] under DataFusion's semantics, read off what the column
/// emits rather than off which terms this build answers.
///
/// A column that did not resolve says nothing here: its fall-back
/// [`crate::resolve::ColumnNote`] is already a `Warning` saying the value is
/// the file's text, which is all DataFusion compares.
fn datafusion_divergences(
    resolved: &ResolvedSchema,
    index: usize,
    declared: &str,
) -> Vec<DivergingPosition> {
    let column = |divergence| (None, declared.to_string(), divergence);
    if resolved.columns[index] != ColumnResolution::Mapped {
        return Vec::new();
    }
    if !resolved.plans[index].is_scalar() {
        // The container's order, then each position's own divergence; a
        // plan with no tree has no position to name.
        let positions = match &resolved.comparisons[index] {
            ComparisonPlan::Nested(tree) => tree.datafusion_divergences(),
            _ => Vec::new(),
        };
        return std::iter::once(column(ComparisonDivergence::NestedOrder))
            .chain(positions.into_iter().map(|(path, declared, d)| (Some(path), declared, d)))
            .collect();
    }
    match &resolved.comparisons[index] {
        ComparisonPlan::Compared { kind, divergence } => {
            datafusion_position_divergences(kind, *divergence).into_iter().map(column).collect()
        }
        // A scalar column's register never answers either; kept rather than
        // assumed away, the fallback being the emitted text.
        ComparisonPlan::Nested(_) | ComparisonPlan::Unanswerable(_) => {
            vec![column(ComparisonDivergence::NestedOrder)]
        }
        // A mapped scalar with no comparison, which no declared type reaches
        // today: DataFusion compares its text, as the note's `=` clause says;
        // its ordering clause is PostgreSQL semantics', DataFusion ordering
        // that text bytewise.
        ComparisonPlan::Refused => vec![column(ComparisonDivergence::UnmodelledType)],
    }
}

/// How `=`/`!=` compare two values of a column of `kind`, given the filter's
/// own literal — settled once, when the block's schema resolves.
///
/// Three canonicalizations, chosen by whether the file's `*_out` text is a
/// *unique* spelling of the value it holds (`docs/design/decisions.md`,
/// "D56"):
///
/// - **[`Comparison::Canonical`] — the literal rendered once** into `*_out`
///   form, so the per-row comparison is a byte comparison. `text`'s rendering
///   is the identity.
/// - **[`Comparison::Trimmed`] — the field narrowed per row.** `character(n)`
///   alone, `bpchareq` stripping the padding from both sides (I38). It is
///   admissible on the per-row path because it is a reverse scan for `0x20`,
///   not a decode.
/// - **[`Comparison::Decoded`] — both sides decoded per row**, for the kinds
///   where `*_out` is *not* injective over the values one file can hold: a
///   bare `numeric` (I33), an `interval` (I40), `jsonb` (I41), and
///   `real`/`double precision`, which have two zeros. A `numeric` whose
///   typmod `Decimal256` cannot carry shares the bare form's kind and so its
///   comparison, `time with time zone`
///   and `inet`/`cidr` decode for a reason about this build instead, and
///   `macaddr` renders (`docs/design/decisions.md`, "D56").
///
/// Deficiency register: `deficiency: KD82` — `Canonical` takes the field's
/// spelling for the value's only one, which holds of what `*_out` wrote. A
/// hand-written field the decoder reads in another spelling — `1.005` or
/// `1.0` in a `numeric(10,2)`, which a typed read rounds and pads to `1.01`
/// and `1.00`, or a braced `uuid` (I64) — is missed by `=`, `!=`, `IN` and a
/// group's dictionary, where an ordering operator and a typed read take its
/// value. Closing it means decoding a field whose bytes miss the literal's,
/// a cost every non-matching row of a dump `pg_dump` wrote would pay.
/// **(c) unowned**; promoted by a user meeting it.
///
/// `None` when the literal is not a value of the column's type at all, which
/// is `Error::PredicateValueDecode` — the same refusal an ordering operator
/// makes, on the same grammar (`docs/design/decisions.md`, "D55").
fn equality_comparison(
    kind: &CompareKind,
    text: &str,
    semantics: ComparisonSemantics,
) -> Option<Comparison> {
    use CompareKind as K;
    let rendered = match kind {
        // Both sides per row. `order_key` reads a special value on the way,
        // so `NaN = NaN` and `Infinity = Infinity` come out of the same order
        // the ordering operators use (I33, I34).
        K::Float32
        | K::Float64
        | K::Numeric { .. }
        | K::Interval { .. }
        | K::IntervalFields { .. }
        | K::Jsonb
        | K::TimeTz
        | K::Network { .. } => {
            let bound = literal_key(kind, text, semantics)?;
            return Some(Comparison::Decoded { kind: kind.clone(), bound });
        }
        K::PaddedText { .. } => {
            return Some(Comparison::Trimmed(text.trim_end_matches(' ').to_string()));
        }
        // A special value is written in its own type's `*_out` spelling on
        // both sides, so it renders to itself. Only `date`, `timestamp` and
        // `numeric(p,s)` reach this arm and admit one.
        _ if special_order_key(kind, text).is_some() => text.to_string(),
        K::Bool => decode::render_bool(decode::decode_bool(text)?).to_string(),
        K::Int { bytes } => match semantics {
            ComparisonSemantics::Postgres => int_in(text, *bytes).ok()?,
            ComparisonSemantics::DataFusion => text.parse::<i64>().ok()?,
        }
        .to_string(),
        K::UnsignedInt => text.parse::<u32>().ok()?.to_string(),
        K::Decimal { .. }
            if semantics == ComparisonSemantics::Postgres && !decode::numeric_in_stores(text) =>
        {
            return None;
        }
        K::Decimal { scale, .. } => {
            decode::render_decimal(&decode::decimal_unscaled_digits(text, *scale)?, *scale)
        }
        // A label is its own canonical form; what the lookup buys is the
        // refusal, a string the type does not declare being no value of the
        // column.
        // deficiency: KD86 — the refusal is made whether or not the labels
        // were read exactly (`TypeKind::Enum`'s `exact`), so a literal naming
        // a label an unread `RENAME VALUE` or `ADD VALUE IF NOT EXISTS` gave
        // the type is refused, where the server reads it; an ordering term,
        // and any term comparing an array or composite holding the enum,
        // raises `FieldDecode` on a field naming one, having no position for
        // it. Comparing by text would answer `=` but no ordering.
        K::Enum { labels, .. } => labels.iter().find(|label| label.as_str() == text)?.clone(),
        K::Date => decode::render_date32(decode::decode_date32(text)?),
        K::Time => decode::render_time64_micros(decode::time_of_day_micros(text).ok()?),
        K::Timestamp { with_tz } => decode::render_timestamp_postgres_micros(
            decode::timestamp_postgres_micros(text, *with_tz).ok()?,
            *with_tz,
        ),
        K::MacAddr { octets } => render_macaddr(text, *octets)?,
        K::Uuid => decode::render_uuid(&decode::decode_uuid(text)?),
        K::Bytea => {
            let bytes = decode::decode_bytea(text)?;
            return Some(Comparison::Canonical(Spellings {
                first: decode::render_bytea(&bytes),
                other: Some(decode::render_bytea_escape(&bytes)),
            }));
        }
        // The identity: `=` on a text column is a byte comparison, with no
        // per-row work added. A literal past a `varchar(n)`'s length is
        // read, the server coercing it with no typmod.
        K::Text { .. } => text.to_string(),
    };
    Some(Comparison::Canonical(Spellings::one(rendered)))
}

/// A literal in each `*_out` form a file can write it in: one, but for a
/// `bytea`, whose `bytea_output` the dump does not pin (I4), so a file holds
/// `hex` or `escape`. No text is a spelling in both (I56), so a field is the
/// literal exactly where it is one of these.
#[derive(Debug, Clone)]
struct Spellings {
    first: String,
    other: Option<String>,
}

impl Spellings {
    fn one(text: String) -> Self {
        Self { first: text, other: None }
    }

    fn matches(&self, text: &str) -> bool {
        text == self.first || self.other.as_deref() == Some(text)
    }

    fn iter(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.first).chain(&self.other)
    }
}

/// What one term compares, settled once when the block's schema resolves
/// rather than per row.
#[derive(Debug, Clone)]
enum Comparison {
    /// The four ordering operators: the field is decoded per row with the
    /// column's own decoder and its key compared against `bound`.
    Ordered { kind: CompareKind, bound: OrderKey },
    /// `=`/`!=` against the literal already rendered into each `*_out` form
    /// the file can hold it in — a byte comparison per row (see
    /// [`equality_comparison`]).
    Canonical(Spellings),
    /// `=`/`!=` on a `character(n)` column, against the literal with its own
    /// trailing blanks already gone.
    Trimmed(String),
    /// `=`/`!=` where the field has to be decoded per row too, because the
    /// file's spelling of a value is not unique.
    Decoded { kind: CompareKind, bound: OrderKey },
    /// A nested column, under **any** comparing operator: both sides read
    /// into a [`NestedKey`] and compared structurally
    /// (`docs/design/decisions.md`, "D58"). One variant for both operator
    /// families, where a scalar has four, because a nested value's `=` is
    /// byte-comparable only when *every* leaf beneath it canonicalizes.
    ///
    /// *Rejected: rendering the literal back and comparing bytes where every
    /// leaf allows it* — a second code path whose decoded half, the half that
    /// has to be right when a leaf does not canonicalize, no oracle case
    /// covers.
    ///
    /// Boxed: a `NestedKey` carries three vectors, and this variant is the
    /// only large one in an enum that sits inside every comparing leaf.
    Nested(Box<NestedComparison>),
}

/// What a nested term compares: the column's plan, and the literal already
/// read through it.
#[derive(Debug, Clone)]
struct NestedComparison {
    plan: NestedCompare,
    bound: NestedKey,
}

/// Everything a comparing term needs, settled once when the block's schema
/// resolves rather than per row.
#[derive(Debug, Clone)]
struct ComparedTerm {
    column: String,
    comparison: Comparison,
    /// The declared PostgreSQL type, for `Error::FieldDecode`'s context and
    /// for [`ComparisonNote::message`].
    declared_type: String,
    /// The register's divergences for this column, already filtered to the
    /// ones that reach *this* term's operator
    /// ([`ComparisonDivergence::affects_equality`]).
    ///
    /// A list, because a nested column has a position per divergence: a
    /// composite can be on the database's collation twice, through two
    /// different fields. Each entry carries the position's own path and
    /// declared type (`docs/design/decisions.md`, "D59").
    divergences: Vec<(Option<String>, String, ComparisonDivergence)>,
    /// Which of a row group's statistics this term reads
    /// ([`ResolvedTerm::truths`]).
    statistics: BelievedStatistics,
}

/// Which of a row group's statistics a comparing term reads, settled once
/// with the rest of the term. A statistic this term does not read is not
/// wrong for it; it is one whose meaning the term's comparison does not
/// share.
#[derive(Debug, Clone)]
struct BelievedStatistics {
    /// The kind the term compares by, which names the stored set of bounds
    /// it reads in each block ([`crate::pgtype::bounds_set_keyed_by`]), and
    /// its literal read as a key — present only where the term's semantics
    /// believes bounds in that kind ([`ComparisonPlan::bounds_read_by`]).
    /// Carried for the equality operators too, whose [`Comparison`] keeps no
    /// kind but where it decodes. Boxed, as [`Comparison::Nested`] is, because every comparing
    /// leaf carries it.
    bounds: Option<Box<(CompareKind, OrderKey)>>,
    /// Whether a group's dictionary answers this term: one of the four
    /// equality operators, on a `Compared` column whose divergence, if any,
    /// does not reach equality — and in DataFusion's semantics not a
    /// `character(n)`, whose entries are stored unpadded
    /// ([`ComparisonPlan::dictionary_answers_in`]).
    dictionary: bool,
}

impl BelievedStatistics {
    /// A term no statistic but the counts answers: a nested column, and one
    /// compared as its text for want of a plan outside DataFusion's semantics.
    const NONE: Self = Self { bounds: None, dictionary: false };
}

/// One filter term resolved against one `COPY` block: the operator, the
/// field index it reads, and the comparison it will make.
///
/// The index is into the block's **unprojected** column list, which is what
/// the raw row's fields are numbered by: a term may name a column the
/// projection dropped.
///
/// It carries its own operator rather than being read back against the
/// [`Predicate`] it came from, which makes the resolved tree
/// ([`ResolvedExpr`]) evaluable without walking the caller's [`Expr`] in
/// lockstep.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTerm {
    op: PredicateOp,
    index: usize,
    /// `None` for the NULL and unrepresentable tests, the four operators that
    /// compare nothing.
    compared: Option<ComparedTerm>,
    /// How the query reads a value of this column its type cannot hold, where
    /// the block's column can hold one ([`ResolvedExpr::reading`]).
    unrepresentable: Option<UnrepresentableRead>,
    /// What an unrepresentable test tests its column's text by, where the
    /// block's count says the column holds such a value in the tiers the
    /// query's front end reads; `None` for every other operator, and for a
    /// test over a column holding none, where `IS UNREPRESENTABLE` is `False`
    /// on every row and `IS NOT UNREPRESENTABLE` `True`
    /// (`crate::unrepresentable::unrepresentable_tests`).
    tested: Option<UnrepresentableRead>,
    /// How the query reads a field its type's `*_in` refuses
    /// ([`ResolvedExpr::invalid_values`]).
    invalid: PostgresInvalidValues,
    /// The column's name, where its declaration makes it `NOT NULL`, so a
    /// NULL field is refused ([`Error::NullRefused`]) but under
    /// [`PostgresInvalidValues::Ignore`], whatever the operator.
    not_null: Option<String>,
}

/// The [`ResolvedTerm::not_null`] of column `index` of `resolved`, named
/// `column`.
fn not_null_column(resolved: &ResolvedSchema, index: usize, column: &str) -> Option<String> {
    resolved.not_null.get(index).copied().unwrap_or(false).then(|| column.to_string())
}

/// The [`Error::NullRefused`] a term or a membership raises over a NULL field
/// of `column`, where it is read under `invalid`, or `None`.
fn null_refused(
    not_null: Option<&str>,
    invalid: PostgresInvalidValues,
    table: &str,
    row_offset: u64,
) -> Option<Error> {
    let column = not_null.filter(|_| invalid != PostgresInvalidValues::Ignore)?;
    Some(Error::NullRefused {
        table: table.to_string(),
        column: column.to_string(),
        line: None,
        line_offset: row_offset,
    })
}

impl ResolvedTerm {
    /// This term's divergences from PostgreSQL's own comparison. Empty for
    /// the operators taking no value, and for every term whose column answers
    /// this operator the way the server does; more than one only for a
    /// nested column with more than one diverging position.
    pub(crate) fn comparison_notes(&self) -> Vec<ComparisonNote> {
        let Some(compared) = self.compared.as_ref() else { return Vec::new() };
        compared
            .divergences
            .iter()
            .map(|(path, declared_type, divergence)| ComparisonNote {
                column: compared.column.clone(),
                path: path.clone(),
                declared_type: declared_type.clone(),
                divergence: *divergence,
            })
            .collect()
    }
}

/// The refusal an ordering operator earns on a column that cannot carry one.
/// Four reasons, each a different fact about the column.
const NOT_MAPPED: &str = "the column's declared type did not resolve to an Arrow type, so it has no order of its own \
     (`--schema-mode strings` resolves no column, by design)";
const NESTED: &str = "the column is nested (array, composite, range or multirange), and an order over such a \
     literal is not defined here";

/// The refusal a nested column earns when its *shape* is compared here and
/// one position beneath it is not — an element, a field or a bound whose own
/// declared type has no order (`json`, `money`, `box`, an unrecognised name).
/// It names the position and its type, and says the server refuses too only
/// where the position's divergence is [`ComparisonDivergence::AsText`] —
/// `json`, which has no comparison proc for `array_cmp` or `record_cmp` to
/// find (I45). Any other position is one this build models no order for,
/// many of which the server orders (`money[]` through `cash_cmp`), so its
/// sentence makes no claim about PostgreSQL.
fn nested_refusal(path: &str, declared: &str, divergence: Option<ComparisonDivergence>) -> String {
    match divergence {
        Some(ComparisonDivergence::AsText) => format!(
            "the column is nested and `{path}` inside it is `{declared}`, which has no order here — \
             PostgreSQL refuses the same comparison, since a container is ordered by its element \
             type's own comparison and this type has none"
        ),
        _ => format!(
            "the column is nested and `{path}` inside it is `{declared}`, for which this build \
             models no order — a container is ordered by its element type's own comparison, so \
             the column has none here either"
        ),
    }
}
const NO_ORDER: &str = "this build defines no ordering for the column's declared type";
const NESTED_IN_DATAFUSION: &str = "the column is nested (array, composite, range or \
     multirange), and this build does not compare a nested value in DataFusion's semantics";

/// The sentence for a column the register can answer **no** operator on,
/// worded here rather than in `crate::pgtype` for the same reason
/// [`accepted_form`] is: the register carries the fact, this layer says it in
/// a sentence about the comparison a filter was going to make.
fn unanswerable_reason(reason: &UnanswerableReason) -> String {
    match reason {
        UnanswerableReason::RangeCanonical { range_type, function } => format!(
            "the range type `{range_type}` declares a canonical function (`{function}`), \
             which PostgreSQL applies to every value of it before storing or comparing \
             one — arbitrary server-side code this build cannot run, so two spellings the \
             server calls one value would be two values here"
        ),
    }
}

/// Resolve one filter term against the block's **unprojected**
/// [`ResolvedSchema`], at `index` — the column's position, already looked up
/// by the caller.
///
/// The NULL and unrepresentable tests need nothing here, the latter's test
/// being the block's to hand it ([`ResolvedExpr::testing`]). Every other
/// operator reads the column's
/// [`ComparisonPlan`] and, where it compares, decodes the filter's own literal
/// here, so a value
/// that is not of the column's type is a fault reported once rather than a
/// filter that matches nothing (`docs/design/decisions.md`, "D54").
///
/// The two operator families part company on a column with no plan, in
/// PostgreSQL's semantics. An ordering operator is *refused*, before a row of this block flows, unless
/// the column compares by its register plan ([`compares_as_declared`]: it
/// resolved `Mapped`, or the untyped mode reads it as its text) and that plan
/// gives it a comparison — a
/// nested column's included, where every position is compared; `Eq`/`Ne` fall
/// back to comparing the canonical `*_out` text the file holds. So a nested
/// column with an uncompared position still answers `=` and refuses `<`.
///
/// Nothing here reads the Arrow type: how a column compares is a conclusion
/// resolution already reached (`crate::pgtype::comparison_for`), carried in
/// [`ResolvedSchema::comparisons`].
///
/// **Under [`ComparisonSemantics::DataFusion`]** a scalar compares by its kind's
/// [`CompareKind::datafusion_order`], a column with no plan — every one of which
/// emits `Utf8View` — compares bytewise under every operator, every
/// comparing operator on a nested column is refused, statistics are read only where they are ordered or equated
/// that way, and no term announces a divergence from PostgreSQL, which is a
/// property of the column rather than of the term ([`column_divergences`];
/// `docs/design/decisions.md`, "D40").
pub(crate) fn resolve_term(
    predicate: &Predicate,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
) -> Result<ResolvedTerm> {
    if predicate.op.takes_no_value() {
        return Ok(ResolvedTerm {
            op: predicate.op,
            index,
            compared: None,
            unrepresentable: None,
            tested: None,
            invalid: PostgresInvalidValues::Default,
            not_null: not_null_column(resolved, index, &predicate.column),
        });
    }
    let ordering = predicate.op.is_ordering();
    let refuse = |reason: &str| Error::UnorderedPredicateColumn {
        header_offset,
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        reason: reason.to_string(),
    };
    let arrow = semantics == ComparisonSemantics::DataFusion;
    let refuse_nested_in_datafusion = || Error::UncomparablePredicateColumn {
        header_offset,
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        reason: NESTED_IN_DATAFUSION.to_string(),
    };
    let declared_type = resolved.notes[index].declared.clone().unwrap_or_default();
    // `value` is `Some` for every operator taking one. An
    // embedder that builds a `Gt` term without one is read as having stated
    // the empty string: a fault wherever the kind's decoder rejects it, and
    // silently a comparison against `""` for the text kinds, which refuse no
    // literal.
    let text = predicate.value.as_deref().unwrap_or_default();
    // `kind` is what knows which grammar was applied, so the refusal is built
    // where it is in scope.
    let refuse_literal = |kind: &CompareKind| Error::PredicateValueDecode {
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        value: text.to_string(),
        declared_type: declared_type.clone(),
        accepted: accepted_form(kind),
    };
    let mut plan = Some(&resolved.comparisons[index]);
    // The nested tree a column fell out of, kept so the bytewise `=` below
    // can say what that fallback costs at the position that refused the order.
    let mut fell_back: Option<&NestedCompare> = None;
    if !compares_as_declared(&resolved.columns[index]) {
        if ordering && !arrow {
            return Err(refuse(NOT_MAPPED));
        }
        plan = None;
    } else if let Some(ComparisonPlan::Unanswerable(reason)) = plan {
        // The one refusal in PostgreSQL's semantics that does not end by
        // offering `=`/`!=`: the file
        // says the server's equality is not a comparison of the text it
        // holds, so the fall-through below would be a wrong answer rather
        // than a weaker one. The operators taking no value have already returned.
        return Err(Error::UncomparablePredicateColumn {
            header_offset,
            column: predicate.column.clone(),
            op: predicate.op.symbol(),
            reason: unanswerable_reason(reason),
        });
    } else if let Some(ComparisonPlan::Nested(tree)) = plan {
        if arrow {
            return Err(refuse_nested_in_datafusion());
        }
        // A nested column whose *shape* is compared here but one of whose
        // positions is not: the ordering operators are refused naming that
        // position, and `=`/`!=` fall back to a byte comparison of the
        // container's whole text, announced rather than silent — `array_cmp`
        // raises for a `json` element rather than returning a comparison.
        if let Some((path, declared, divergence)) = tree.uncomparable() {
            if ordering {
                return Err(refuse(&nested_refusal(&path, &declared, divergence)));
            }
            fell_back = Some(tree);
            plan = None;
        }
    } else if !resolved.plans[index].is_scalar() {
        // A column the *resolver* calls nested and the register does not.
        // One shape reaches it — a range whose DDL stated no `subtype`, so
        // the resolver keeps the struct with `Utf8View` bounds and the
        // register has no bound type to name a refusal after — and it is also
        // where the two walks would land if they disagreed about a declared
        // type, the alternative to refusing being a silent scalar comparison.
        if arrow {
            return Err(refuse_nested_in_datafusion());
        }
        if ordering {
            return Err(refuse(NESTED));
        }
        plan = None;
    }
    let (comparison, divergences, statistics) = match plan {
        Some(compared @ ComparisonPlan::Compared { kind, divergence }) => {
            let kind = &match semantics {
                ComparisonSemantics::Postgres => kind.clone(),
                ComparisonSemantics::DataFusion => kind.datafusion_order(),
            };
            let comparison = if ordering {
                Comparison::Ordered {
                    kind: kind.clone(),
                    bound: literal_key(kind, text, semantics)
                        .ok_or_else(|| refuse_literal(kind))?,
                }
            } else {
                equality_comparison(kind, text, semantics).ok_or_else(|| refuse_literal(kind))?
            };
            let divergences: Vec<_> = divergence
                .filter(|d| ordering || d.affects_equality())
                .map(|d| (None, declared_type.clone(), d))
                .into_iter()
                .collect();
            let statistics = BelievedStatistics {
                bounds: believed_bounds(compared, semantics, kind, text),
                dictionary: !ordering && compared.dictionary_answers_in(semantics),
            };
            (comparison, divergences, statistics)
        }
        // A nested column, compared structurally: the literal is read once in
        // the `array_in`/`record_in` superset, the field per row in the strict
        // `*_out` grammar.
        Some(ComparisonPlan::Nested(tree)) => (
            Comparison::Nested(Box::new(NestedComparison {
                plan: tree.clone(),
                bound: nested_key(tree, text, Side::Literal).ok_or_else(|| {
                    Error::PredicateValueDecode {
                        column: predicate.column.clone(),
                        op: predicate.op.symbol(),
                        value: text.to_string(),
                        declared_type: declared_type.clone(),
                        accepted: nested_accepted_form(tree),
                    }
                })?,
            })),
            tree.divergences()
                .into_iter()
                .filter(|(_, _, d)| ordering || d.affects_equality())
                .map(|(path, declared, d)| (Some(path), declared, d))
                .collect(),
            BelievedStatistics::NONE,
        ),
        // No plan at all: an ordering operator is refused here, and `Eq`/`Ne`
        // on a column the register does not compare is a comparison of the
        // canonical `*_out` text the file holds. In DataFusion's semantics the
        // column is the `Utf8View` holding that text, which DataFusion orders
        // bytewise, and so is it ordered here, reading the bytewise bounds
        // gathered for it.
        _ if ordering && arrow => (
            Comparison::Ordered {
                kind: CompareKind::Text { length: None },
                bound: order_key(&CompareKind::Text { length: None }, text)
                    .ok_or_else(|| refuse_literal(&CompareKind::Text { length: None }))?,
            },
            Vec::new(),
            BelievedStatistics {
                bounds: believed_bounds(
                    &resolved.comparisons[index],
                    semantics,
                    &CompareKind::Text { length: None },
                    text,
                ),
                dictionary: false,
            },
        ),
        _ if ordering => return Err(refuse(NO_ORDER)),
        //
        // What it announces comes from one of two places, and which one is
        // whether a nested tree sent the column here.
        //
        // A column that *fell out of a tree* announces per position, as the
        // structural arm above does: the position that refused the order, and
        // any other whose divergence reaches equality. A position the
        // *resolver* declined instead (I22, I26) never reaches here; its
        // column is not `Mapped`.
        //
        // Every other column announces off its own **resolution**, and only
        // `UnknownType` and `OpaqueBaseType` do: the file *named* a type this
        // build models nothing for — `box`, `money`, a C-level base type —
        // and `box_eq` compares areas, so bytewise is a guess there
        // ([`ComparisonDivergence::UnmodelledType`]). Every other outcome is
        // silent, there being no declared type to qualify.
        _ => (
            Comparison::Canonical(Spellings::one(text.to_string())),
            match fell_back {
                Some(tree) => tree
                    .divergences()
                    .into_iter()
                    .filter(|(_, _, d)| d.affects_equality())
                    .map(|(path, declared, d)| (Some(path), declared, d))
                    .collect(),
                None => matches!(
                    resolved.columns[index],
                    ColumnResolution::UnknownType | ColumnResolution::OpaqueBaseType
                )
                .then(|| (None, declared_type.clone(), ComparisonDivergence::UnmodelledType))
                .into_iter()
                .collect(),
            },
            BelievedStatistics {
                bounds: believed_bounds(
                    &resolved.comparisons[index],
                    semantics,
                    &CompareKind::Text { length: None },
                    text,
                ),
                dictionary: false,
            },
        ),
    };
    // Arrow's answer is not the register's to qualify: a divergence from
    // PostgreSQL is the column's (`docs/design/decisions.md`, "D40").
    let divergences = if arrow { Vec::new() } else { divergences };
    Ok(ResolvedTerm {
        op: predicate.op,
        index,
        unrepresentable: None,
        tested: None,
        invalid: PostgresInvalidValues::Default,
        not_null: not_null_column(resolved, index, &predicate.column),
        compared: Some(ComparedTerm {
            column: predicate.column.clone(),
            comparison,
            declared_type,
            divergences,
            statistics,
        }),
    })
}

/// The kind a term comparing by `kind` in `semantics`, over a column of
/// `plan`, reads bounds by, with `text` read as its key — `None` where
/// `semantics` believes no bounds in that kind for the column, or the literal
/// does not key. Which stored set that is, is each block's to say: it is the
/// one gathering stored under `kind` (`docs/design/decisions.md`, "D79").
fn believed_bounds(
    plan: &ComparisonPlan,
    semantics: ComparisonSemantics,
    kind: &CompareKind,
    text: &str,
) -> Option<Box<(CompareKind, OrderKey)>> {
    (plan.bounds_read_by(semantics).as_ref() == Some(kind)).then_some(())?;
    literal_key(kind, text, semantics).map(|key| Box::new((kind.clone(), key)))
}

impl ResolvedTerm {
    /// Evaluate this term against `raw_row`, in SQL's three-valued domain.
    /// `table` and `row_offset` are context for the error a field raises that
    /// does not decode as its mapped type under a comparison that reads it,
    /// `Error::FieldDecode`, worded exactly as the typed build path words it,
    /// and for the `Error::NullRefused` a NULL its column's `NOT NULL`
    /// refuses raises under every operator; a field that does not unescape
    /// raises the row's own error.
    ///
    /// A NULL field is [`Truth::Unknown`] under every comparing operator. The
    /// six that answer two-valued instead are `IsNull`/`IsNotNull` and the two
    /// unrepresentable tests, which compare nothing, and the two
    /// `IS DISTINCT FROM` forms, which count NULL as a value — so
    /// `IsDistinctFrom` on a NULL field is `True` where `Ne` is `Unknown`.
    fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let field = timed!(Part::Locate, split.field(raw_row.bytes(), self.index));
        let located = field.is_some();
        let decoded = match field {
            Some(f) => timed!(Part::Unescape, raw_row.decode(f))?,
            None => None,
        };
        if located
            && decoded.is_none()
            && let Some(refused) =
                null_refused(self.not_null.as_deref(), self.invalid, table, row_offset)
        {
            return Err(refused);
        }
        self.eval_value(decoded.as_deref())
            .ok_or_else(|| self.field_decode(table, row_offset, decoded.as_deref()))
    }

    /// The `Error::FieldDecode` a comparing term raises over a `value` that
    /// is not of its column's type. A value of the type its column's Arrow
    /// type cannot hold keys in PostgreSQL's order, or is read as NULL, and
    /// never reaches here (`docs/design/decisions.md`, "D56", "D98").
    fn field_decode(&self, table: &str, row_offset: u64, value: Option<&str>) -> Error {
        let compared = self.compared.as_ref().expect("an operator taking no value decodes nothing");
        Error::FieldDecode {
            table: table.to_string(),
            column: compared.column.clone(),
            line_offset: row_offset,
            declared_type: compared.declared_type.clone(),
            value: value.unwrap_or_default().to_string(),
        }
    }

    /// `value`, or NULL where the query reads it as NULL.
    #[inline]
    fn as_read<'v>(&self, value: Option<&'v str>) -> Option<&'v str> {
        match &self.unrepresentable {
            Some(read) => value.filter(|text| !read.cannot_hold(text)),
            None => value,
        }
    }

    /// This term's answer for one **already unescaped** value of its column,
    /// `None` being SQL NULL — the row path's whole comparison, with no row
    /// in it. [`Self::eval`] is this after splitting and unescaping a field;
    /// a value that did not come from a row, such as a statistic's dictionary
    /// entry, is answered by the same code and so cannot be answered
    /// differently.
    ///
    /// `None` back is a non-NULL value that is not a value of the column's
    /// type under this term's comparison — what the row path reports as
    /// `Error::FieldDecode`, the only case in which it returns nothing. The
    /// operators taking no value and the two equalities comparing text never
    /// decode, so they always answer.
    ///
    /// **A value the query reads as NULL is answered as one**
    /// (`docs/design/decisions.md`, "D98") — but by the two unrepresentable
    /// tests, which ask of the text itself whether it is such a value.
    #[inline]
    pub(crate) fn eval_value(&self, value: Option<&str>) -> Option<Truth> {
        let Some(compared) = self.compared.as_ref() else {
            return Some(Truth::of(match self.op {
                PredicateOp::IsNull => self.as_read(value).is_none(),
                PredicateOp::IsNotNull => self.as_read(value).is_some(),
                op => {
                    let unrepresentable = value
                        .zip(self.tested.as_ref())
                        .is_some_and(|(text, test)| timed!(Part::Compare, test.cannot_hold(text)));
                    unrepresentable == (op == PredicateOp::IsUnrepresentable)
                }
            }));
        };
        let value = self.as_read(value);
        let Some(text) = value else {
            return Some(match self.op {
                PredicateOp::IsDistinctFrom => Truth::True,
                PredicateOp::IsNotDistinctFrom => Truth::False,
                _ => Truth::Unknown,
            });
        };
        // The key and the comparison are timed apart, where the instrument
        // is built in (`crate::instrument::timed!`), each key dropped where
        // it was when it was a temporary of the comparison.
        Some(Truth::of(match &compared.comparison {
            Comparison::Ordered { kind, bound } => {
                let ord = {
                    let key = timed!(Part::Key, read_key(kind, text, self.invalid))?;
                    timed!(Part::Compare, compare_keys(&key, bound))
                };
                match self.op {
                    PredicateOp::Lt => ord.is_lt(),
                    PredicateOp::Le => ord.is_le(),
                    PredicateOp::Gt => ord.is_gt(),
                    _ => ord.is_ge(),
                }
            }
            Comparison::Canonical(bound) => {
                timed!(Part::Compare, bound.matches(text)) == self.wants_equal()
            }
            Comparison::Trimmed(bound) => {
                timed!(Part::Compare, text.trim_end_matches(' ') == bound.as_str())
                    == self.wants_equal()
            }
            Comparison::Decoded { kind, bound } => {
                let key = timed!(Part::Key, read_key(kind, text, self.invalid))?;
                timed!(Part::Compare, compare_keys(&key, bound)).is_eq() == self.wants_equal()
            }
            // Two-valued throughout: a NULL *inside* the container is a value
            // of it, and only the whole field being NULL is unknown — which
            // was decided above, before any of this runs.
            Comparison::Nested(nested) => {
                let ord = {
                    let key = timed!(
                        Part::Key,
                        nested_key(&nested.plan, text, Side::Field(self.invalid))
                    )?;
                    timed!(Part::Compare, compare_nested(&key, &nested.bound))
                };
                match self.op {
                    PredicateOp::Lt => ord.is_lt(),
                    PredicateOp::Le => ord.is_le(),
                    PredicateOp::Gt => ord.is_gt(),
                    PredicateOp::Ge => ord.is_ge(),
                    _ => ord.is_eq() == self.wants_equal(),
                }
            }
        }))
    }

    /// The operator this term applies.
    pub(crate) fn op(&self) -> PredicateOp {
        self.op
    }

    /// The column this term reads, numbered by the block's unprojected
    /// column list.
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// The kind naming the stored set of bounds and row order this term
    /// reads, if any ([`BelievedStatistics::bounds`]).
    pub(crate) fn bounds_kind(&self) -> Option<&CompareKind> {
        self.compared.as_ref()?.statistics.bounds.as_deref().map(|(kind, _)| kind)
    }

    /// Whether `raw_row` makes this term [`Truth::False`] — answered where
    /// [`ResolvedExpr::matches`] may never have evaluated the term, so **it
    /// raises nothing**: a field that does not unescape or decode answers
    /// `false`, as a NULL or a missing field does, and the error stays where
    /// evaluation reaches it (`docs/design/decisions.md`, "D54").
    pub(crate) fn is_false(&self, raw_row: RawRow<'_>, split: &mut RowSplit) -> bool {
        let Some(field) = split.field(raw_row.bytes(), self.index) else { return false };
        let Ok(Some(value)) = raw_row.decode(field) else { return false };
        self.eval_value(Some(&value)) == Some(Truth::False)
    }

    /// Whether this term keeps the rows its comparison called equal — `Eq`
    /// and `IsNotDistinctFrom` do, `Ne` and `IsDistinctFrom` do not, and no
    /// other operator reaches it.
    fn wants_equal(&self) -> bool {
        matches!(self.op, PredicateOp::Eq | PredicateOp::IsNotDistinctFrom)
    }
}

/// An [`Expr::In`] resolved against one `COPY` block: the `=` term each
/// non-NULL value resolves to, and the lookup the row path answers from.
///
/// The terms are kept, in list order, because they are what the membership
/// *is*: a row group's statistics are read off them exactly as off the
/// [`Expr::Or`] of them, and the divergence notes off the first, every term's
/// being the same; only the per-row answer is
/// taken from [`Lookup`] instead — one decode and one probe where the `Or`
/// would fetch, unescape and compare the field once per term
/// (`docs/design/decisions.md`, "D53").
#[derive(Debug, Clone)]
pub(crate) struct ResolvedMembership {
    index: usize,
    terms: Vec<ResolvedTerm>,
    /// Whether the list holds a NULL, which answers `Unknown` wherever no
    /// other value matches.
    null: bool,
    lookup: Lookup,
    /// As [`ResolvedTerm`]'s.
    unrepresentable: Option<UnrepresentableRead>,
    /// As [`ResolvedTerm`]'s.
    invalid: PostgresInvalidValues,
    /// As [`ResolvedTerm`]'s.
    not_null: Option<String>,
}

/// Every non-NULL value of one membership, held the way its `=` terms
/// compare. One column under one semantics settles one [`Comparison`] shape
/// for every value, so the terms share one variant; were they ever not to,
/// [`Lookup::Each`] answers through the terms themselves, in order, as the
/// `Or` does.
// Deficiency register: `deficiency: KD55` — `Canonical` and `Trimmed` hash
// the field's text with std's `HashSet` hasher, SipHash-1-3 under a random
// key, on every row a membership reads; the costing input's account puts the
// lookup among the largest terms of a row's evaluation
// (`docs/design/measurements.md`, "What DataFusion's dynamic filters buy a
// query"). A fast hasher seeded once a process (`foldhash`'s, say) keeps a
// list flooded with colliding values from degrading the probe, which is what
// SipHash's key is for, at a fraction of its cost; keying the field instead
// adds back the parse dropping the implied bounds removes. **(c) unowned**;
// promoted by row evaluation going on by default, or a figure pricing
// `pgdt --where`'s `in (…)`.
#[derive(Debug, Clone)]
enum Lookup {
    /// [`Comparison::Canonical`]: the literals in every spelling the file
    /// can hold them in, the field probed as it stands.
    Canonical(std::collections::HashSet<String>),
    /// [`Comparison::Trimmed`]: the literals trimmed, the field trimmed
    /// before it is probed.
    Trimmed(std::collections::HashSet<String>),
    /// [`Comparison::Decoded`]: the literals' keys sorted by
    /// [`compare_keys`], the field keyed once and searched for among them.
    Decoded { kind: CompareKind, keys: Vec<OrderKey> },
    /// [`Comparison::Nested`]: as `Decoded`, by [`compare_nested`].
    Nested { plan: NestedCompare, keys: Vec<NestedKey> },
    /// Anything else: each term in list order.
    Each,
}

impl Lookup {
    fn of(terms: &[ResolvedTerm]) -> Self {
        let comparisons: Vec<&Comparison> =
            terms.iter().filter_map(|t| t.compared.as_ref().map(|c| &c.comparison)).collect();
        if comparisons.len() != terms.len() {
            return Self::Each;
        }
        let texts = |wanted: fn(&Comparison) -> Option<&String>| {
            comparisons.iter().map(|c| wanted(c).cloned()).collect::<Option<_>>()
        };
        let built = match comparisons.first() {
            None | Some(Comparison::Canonical(_)) => comparisons
                .iter()
                .map(|c| match c {
                    Comparison::Canonical(spellings) => Some(spellings.iter().cloned()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|spellings| Self::Canonical(spellings.into_iter().flatten().collect())),
            Some(Comparison::Trimmed(_)) => texts(|c| match c {
                Comparison::Trimmed(text) => Some(text),
                _ => None,
            })
            .map(Self::Trimmed),
            Some(Comparison::Decoded { kind, .. }) => comparisons
                .iter()
                .map(|c| match c {
                    Comparison::Decoded { kind: own, bound } if own == kind => Some(bound.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|mut keys| {
                    keys.sort_by(compare_keys);
                    Self::Decoded { kind: kind.clone(), keys }
                }),
            Some(Comparison::Nested(first)) => comparisons
                .iter()
                .map(|c| match c {
                    Comparison::Nested(own) if own.plan == first.plan => Some(own.bound.clone()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|mut keys| {
                    keys.sort_by(compare_nested);
                    Self::Nested { plan: first.plan.clone(), keys }
                }),
            Some(Comparison::Ordered { .. }) => None,
        };
        built.unwrap_or(Self::Each)
    }

    /// Whether `text` equals one of the values, `None` where it is not a
    /// value of the column's type — which the first `=` term would have
    /// raised on.
    fn contains(
        &self,
        text: &str,
        terms: &[ResolvedTerm],
        invalid: PostgresInvalidValues,
    ) -> Option<bool> {
        Some(match self {
            Self::Canonical(values) => timed!(Part::Lookup, values.contains(text)),
            Self::Trimmed(values) => {
                timed!(Part::Lookup, values.contains(text.trim_end_matches(' ')))
            }
            Self::Decoded { kind, keys } => {
                let key = timed!(Part::Key, read_key(kind, text, invalid))?;
                timed!(Part::Lookup, keys.binary_search_by(|k| compare_keys(k, &key)).is_ok())
            }
            Self::Nested { plan, keys } => {
                let key = timed!(Part::Key, nested_key(plan, text, Side::Field(invalid)))?;
                timed!(Part::Lookup, keys.binary_search_by(|k| compare_nested(k, &key)).is_ok())
            }
            Self::Each => {
                for term in terms {
                    if term.eval_value(Some(text))? == Truth::True {
                        return Some(true);
                    }
                }
                false
            }
        })
    }
}

/// Resolve `membership` against the block's unprojected `resolved` schema,
/// its column already found at `index`: each non-NULL value as the `=` term
/// [`resolve_term`] makes of it, in list order, so the first value that
/// refuses is the one refused — its error naming the operator `IN`.
pub(crate) fn resolve_membership(
    membership: &Membership,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
) -> Result<ResolvedMembership> {
    let terms = membership
        .values
        .iter()
        .flatten()
        .map(|value| {
            let equal = Predicate {
                column: membership.column.clone(),
                op: PredicateOp::Eq,
                value: Some(value.clone()),
            };
            resolve_term(&equal, index, resolved, header_offset, semantics).map_err(named_in)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ResolvedMembership {
        index,
        null: membership.values.iter().any(Option::is_none),
        lookup: Lookup::of(&terms),
        terms,
        unrepresentable: None,
        invalid: PostgresInvalidValues::Default,
        not_null: not_null_column(resolved, index, &membership.column),
    })
}

/// A refusal [`resolve_term`] raised for one value's `=`, re-worded as the
/// membership's.
fn named_in(error: Error) -> Error {
    const IN: &str = "IN";
    match error {
        Error::PredicateValueDecode { column, value, declared_type, accepted, .. } => {
            Error::PredicateValueDecode { column, op: IN, value, declared_type, accepted }
        }
        Error::UncomparablePredicateColumn { header_offset, column, reason, .. } => {
            Error::UncomparablePredicateColumn { header_offset, column, op: IN, reason }
        }
        other => other,
    }
}

impl ResolvedMembership {
    /// What the membership answers where no value matched: `Unknown` if the
    /// list holds a NULL, else `False`.
    fn unmatched(&self) -> Truth {
        if self.null { Truth::Unknown } else { Truth::False }
    }

    /// The membership's answer for one already unescaped value, `None`
    /// being SQL NULL — [`ResolvedTerm::eval_value`]'s counterpart, `None`
    /// back where the value is not of the column's type.
    pub(crate) fn eval_value(&self, value: Option<&str>) -> Option<Truth> {
        if self.terms.is_empty() {
            return Some(self.unmatched());
        }
        let value = match &self.unrepresentable {
            Some(read) => value.filter(|text| !read.cannot_hold(text)),
            None => value,
        };
        let Some(text) = value else { return Some(Truth::Unknown) };
        Some(if self.lookup.contains(text, &self.terms, self.invalid)? {
            Truth::True
        } else {
            self.unmatched()
        })
    }

    /// [`Self::eval_value`] over `raw_row`'s field, raising what the first
    /// `=` term would: the row's own error where the field does not unescape,
    /// `Error::FieldDecode` where it does not decode. A membership with no
    /// value to compare reads no field.
    fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let Some(first) = self.terms.first() else { return Ok(self.unmatched()) };
        let field = timed!(Part::Locate, split.field(raw_row.bytes(), self.index));
        let located = field.is_some();
        let decoded = match field {
            Some(field) => timed!(Part::Unescape, raw_row.decode(field))?,
            None => None,
        };
        if located
            && decoded.is_none()
            && let Some(refused) =
                null_refused(self.not_null.as_deref(), self.invalid, table, row_offset)
        {
            return Err(refused);
        }
        self.eval_value(decoded.as_deref())
            .ok_or_else(|| first.field_decode(table, row_offset, decoded.as_deref()))
    }

    /// Every value this membership could take over a row of `group`: its
    /// terms' sets combined as [`ResolvedExpr::Or`] combines them, a NULL in
    /// the list adding `Unknown` over any group a row starts in — the answer
    /// the `Or` of `=` over the same list gives, not a tighter one. Tighter,
    /// though a row equals at most one value, would break the per-group
    /// agreement with the `Or` its generated check proves, for pruning
    /// nobody has measured; so only the row path answers by one lookup — a
    /// hash, or a binary search over the sorted keys, but for
    /// [`Lookup::Each`] — a group being answered term by term.
    #[cfg(test)]
    fn truths(&self, group: &impl GroupStatistics) -> TruthSet {
        self.truths_keyed(group, &mut KeyedBounds::default())
    }

    /// [`Self::truths`], every term reading the group's bounds out of
    /// `keyed`, so they are keyed once for the whole list. The sorted list is
    /// not searched within the bounds, which would stop the cost scaling with
    /// the list: what remains per term against the bounds is a key
    /// comparison, no parse. A dictionary is still answered per term through
    /// the row path, which parses each entry of a decoded kind.
    fn truths_keyed<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        let set = self
            .terms
            .iter()
            .fold(TruthSet::of(Truth::False), |set, t| set.or(t.truths_keyed(group, keyed)));
        match (self.null, group.rows()) {
            (false, _) => set,
            // `= NULL` over a group no row starts in is the empty set, as
            // every term is there, and the empty set absorbs a disjunction.
            (true, 0) => TruthSet::EMPTY,
            (true, _) => set.or(TruthSet::of(Truth::Unknown)),
        }
    }

    /// Whether every row this membership makes `True` makes `term` `True`
    /// too: `term` is one of the four ordering operators over the same
    /// column, and every value in the list meets it as a row equal to that
    /// value would. Answered for the two lookups whose equality settles what
    /// an ordering term reads — the text the file spells, or a key in the
    /// ordering term's own kind — and `false` for any other.
    fn implies(&self, term: &ResolvedTerm) -> bool {
        if term.index != self.index || !term.op.is_ordering() || self.terms.is_empty() {
            return false;
        }
        let Some(Comparison::Ordered { kind: ordered, bound }) =
            term.compared.as_ref().map(|c| &c.comparison)
        else {
            return false;
        };
        let meets = |key: &OrderKey| {
            let ord = compare_keys(key, bound);
            match term.op {
                PredicateOp::Lt => ord.is_lt(),
                PredicateOp::Le => ord.is_le(),
                PredicateOp::Gt => ord.is_gt(),
                _ => ord.is_ge(),
            }
        };
        match &self.lookup {
            Lookup::Canonical(_) => self.terms.iter().all(|value| {
                matches!(
                    value.compared.as_ref().map(|c| &c.comparison),
                    Some(Comparison::Canonical(spellings)) if term.eval_value(Some(&spellings.first)) == Some(Truth::True)
                )
            }),
            Lookup::Decoded { kind, keys } => kind == ordered && keys.iter().all(meets),
            // So a join over `character(n)` keys keeps its bounds per row:
            // speed, not rows.
            Lookup::Trimmed(_) | Lookup::Nested { .. } | Lookup::Each => false,
        }
    }

    /// The notes its first term carries, every term's being the same
    /// column's under the same operator.
    fn comparison_notes(&self) -> Vec<ComparisonNote> {
        self.terms.first().map(ResolvedTerm::comparison_notes).unwrap_or_default()
    }
}

/// One [`Expr`] resolved against one `COPY` block: the same tree, with each
/// leaf replaced by the [`ResolvedTerm`] that block's schema produced. What a
/// block's `Active` state carries, and evaluable on its own.
#[derive(Debug, Clone)]
pub(crate) enum ResolvedExpr {
    Term(ResolvedTerm),
    In(ResolvedMembership),
    And(Vec<ResolvedExpr>),
    Or(Vec<ResolvedExpr>),
    Not(Box<ResolvedExpr>),
}

impl ResolvedExpr {
    /// Whether `raw_row` survives this filter: its root evaluates
    /// [`Truth::True`]. `Unknown` and `False` both drop the row, which is
    /// what makes the collapse at the root sound though it is not sound under
    /// a `Not` (`docs/design/decisions.md`, "D54").
    pub(crate) fn matches(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<bool> {
        Ok(self.eval(false, raw_row, split, table, row_offset)?.is_true())
    }

    /// Kleene evaluation, left to right, stopping as soon as **the root's**
    /// value is determined.
    ///
    /// `exact` is what makes that "the root's" rather than "this node's". A
    /// caller that cannot tell `False` from `Unknown` — [`Self::matches`], and
    /// any `And`/`Or` whose own caller cannot — lets this node return `False`
    /// for an unknown and stop at the first non-`True` conjunct. Only
    /// [`Self::Not`] distinguishes them, so it is the one node that evaluates
    /// its child exactly, and everything beneath a `Not` is exact too.
    ///
    /// A decode failure is therefore raised only where evaluation reaches it:
    /// which rows error depends on where the term sits in the tree, and on
    /// whether a `Not` sits above it (`docs/design/decisions.md`, "D54").
    fn eval(
        &self,
        exact: bool,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        Ok(match self {
            Self::Term(term) => term.eval(raw_row, split, table, row_offset)?,
            Self::In(membership) => membership.eval(raw_row, split, table, row_offset)?,
            Self::And(children) => {
                let mut unknown = false;
                for child in children {
                    match child.eval(exact, raw_row, split, table, row_offset)? {
                        Truth::True => {}
                        Truth::False => return Ok(Truth::False),
                        Truth::Unknown if exact => unknown = true,
                        Truth::Unknown => return Ok(Truth::False),
                    }
                }
                if unknown { Truth::Unknown } else { Truth::True }
            }
            Self::Or(children) => {
                let mut unknown = false;
                for child in children {
                    match child.eval(exact, raw_row, split, table, row_offset)? {
                        Truth::True => return Ok(Truth::True),
                        Truth::Unknown if exact => unknown = true,
                        Truth::False | Truth::Unknown => {}
                    }
                }
                if unknown { Truth::Unknown } else { Truth::False }
            }
            Self::Not(inner) => inner.eval(true, raw_row, split, table, row_offset)?.not(),
        })
    }

    /// **This tree with each leaf reading its column as the query does**
    /// ([`UnrepresentableRead`]): `reads` is per column of the block's
    /// unprojected schema, which a leaf's index numbers, and a leaf over a
    /// column it holds `None` for, or none at all, reads every value as
    /// itself (`docs/design/decisions.md`, "D98").
    pub(crate) fn reading(mut self, reads: &[Option<UnrepresentableRead>]) -> ResolvedExpr {
        if reads.iter().any(Option::is_some) {
            self.set_reads(reads);
        }
        self
    }

    fn set_reads(&mut self, reads: &[Option<UnrepresentableRead>]) {
        let read = |index: usize| reads.get(index).cloned().flatten();
        match self {
            Self::Term(term) => term.unrepresentable = read(term.index),
            Self::In(membership) => {
                membership.unrepresentable = read(membership.index);
                for term in &mut membership.terms {
                    term.unrepresentable = read(term.index);
                }
            }
            Self::And(children) | Self::Or(children) => {
                children.iter_mut().for_each(|child| child.set_reads(reads));
            }
            Self::Not(inner) => inner.set_reads(reads),
        }
    }

    /// **This tree with each unrepresentable test testing its column's text**
    /// ([`PredicateOp::IsUnrepresentable`]): `tests` is per column of the
    /// block's unprojected schema, as [`Self::reading`]'s `reads` is, and over
    /// a column it holds `None` for `IS UNREPRESENTABLE` is `False` on every row
    /// and `IS NOT UNREPRESENTABLE` `True` (`docs/design/decisions.md`, "D101").
    pub(crate) fn testing(mut self, tests: &[Option<UnrepresentableRead>]) -> ResolvedExpr {
        if tests.iter().any(Option::is_some) {
            self.set_tests(tests);
        }
        self
    }

    /// **This tree with each leaf reading a field its type's `*_in` refuses
    /// as the query does** ([`crate::QueryOptions::postgres_invalid_values`]);
    /// its literals were read when it was resolved, and stay so.
    pub(crate) fn invalid_values(mut self, invalid: PostgresInvalidValues) -> ResolvedExpr {
        if invalid != PostgresInvalidValues::Default {
            self.set_invalid(invalid);
        }
        self
    }

    fn set_invalid(&mut self, invalid: PostgresInvalidValues) {
        match self {
            Self::Term(term) => term.invalid = invalid,
            Self::In(membership) => {
                membership.invalid = invalid;
                for term in &mut membership.terms {
                    term.invalid = invalid;
                }
            }
            Self::And(children) | Self::Or(children) => {
                children.iter_mut().for_each(|child| child.set_invalid(invalid));
            }
            Self::Not(inner) => inner.set_invalid(invalid),
        }
    }

    fn set_tests(&mut self, tests: &[Option<UnrepresentableRead>]) {
        match self {
            Self::Term(term) if term.op.tests_unrepresentable() => {
                term.tested = tests.get(term.index).cloned().flatten();
            }
            Self::Term(_) | Self::In(_) => {}
            Self::And(children) | Self::Or(children) => {
                children.iter_mut().for_each(|child| child.set_tests(tests));
            }
            Self::Not(inner) => inner.set_tests(tests),
        }
    }

    /// This tree as a row is evaluated against it where only whether the root
    /// is `True` decides anything and a failing leaf keeps the row — a
    /// dynamic filter's ([`crate::stream::DynamicFilter`]) — **with every
    /// ordering term a membership in the same conjunction implies dropped**
    /// ([`ResolvedMembership::implies`]): a join's filter bounds its keys and
    /// lists them, and each bound reads, unescapes and keys a field the list
    /// answers alone (`docs/design/decisions.md`, "D54" and "D93").
    ///
    /// **Only where the evaluation is not exact**, reached from the root
    /// through `And` and `Or` alone: there a conjunction is `True` exactly
    /// where each conjunct is, and a membership `True` makes each term it
    /// implies `True`, so the root is `True` on the same rows. Beneath a
    /// `Not`, where a conjunction's `Unknown` and `False` are different
    /// answers, the subtree stands as it is. A conjunction nested in another
    /// is read as one, and one left with a single conjunct is that conjunct.
    ///
    /// A row the dropped term would have failed to key — a value that is
    /// not of its column's type, which a file's `*_out` text never is — is
    /// answered by the membership instead, as any row a leaf earlier in the
    /// tree rules out is (`docs/design/decisions.md`, "D54").
    pub(crate) fn for_rows(&self) -> ResolvedExpr {
        match self {
            Self::Term(_) | Self::In(_) | Self::Not(_) => self.clone(),
            Self::Or(children) => Self::Or(children.iter().map(Self::for_rows).collect()),
            Self::And(_) => {
                let mut conjuncts = Vec::new();
                self.conjuncts(&mut conjuncts);
                let memberships: Vec<&ResolvedMembership> = conjuncts
                    .iter()
                    .filter_map(|c| match c {
                        Self::In(membership) => Some(membership),
                        _ => None,
                    })
                    .collect();
                let mut kept: Vec<ResolvedExpr> = conjuncts
                    .into_iter()
                    .filter(|c| match c {
                        Self::Term(term) => !memberships.iter().any(|m| m.implies(term)),
                        _ => true,
                    })
                    .map(Self::for_rows)
                    .collect();
                if kept.len() == 1 { kept.pop().expect("one conjunct") } else { Self::And(kept) }
            }
        }
    }

    /// This node's conjuncts, in order, a conjunction nested in it read as
    /// part of it.
    fn conjuncts<'a>(&'a self, out: &mut Vec<&'a ResolvedExpr>) {
        match self {
            Self::And(children) => children.iter().for_each(|child| child.conjuncts(out)),
            other => out.push(other),
        }
    }

    /// The divergence notes for every term in this tree, in the order the
    /// caller wrote them — what `TableStream::comparison_notes` hands back.
    /// Derived from the resolved tree rather than stored beside it, so the two
    /// cannot disagree (`docs/design/decisions.md`, "D59").
    pub(crate) fn comparison_notes(&self) -> Vec<ComparisonNote> {
        let mut out = Vec::new();
        self.collect_notes(&mut out);
        out
    }

    /// Whether evaluating this tree reads a field at all. An empty
    /// conjunction — the `Expr::And(vec![])` a query with no filter carries —
    /// reads nothing, and a stream whose projection is also empty therefore
    /// decodes nothing and skips the bulk UTF-8 validation
    /// (`docs/design/decisions.md`, "D27").
    pub(crate) fn reads_fields(&self) -> bool {
        match self {
            Self::Term(_) => true,
            Self::In(membership) => !membership.terms.is_empty(),
            Self::And(children) | Self::Or(children) => children.iter().any(Self::reads_fields),
            Self::Not(inner) => inner.reads_fields(),
        }
    }

    /// The ordering terms every row this tree keeps must make `True` — the
    /// root when it is one, and each member of a conjunction at the root,
    /// conjunctions nested in it flattened — whose column's stored row order
    /// is ordered as the term's semantics orders it
    /// ([`BelievedStatistics::bounds`]). A term beneath an `Or` or a
    /// `Not` is not one: a row can be kept while it is `False`.
    pub(crate) fn required_ordering_terms(&self) -> Vec<&ResolvedTerm> {
        let mut out = Vec::new();
        let mut pending = vec![self];
        while let Some(node) = pending.pop() {
            match node {
                Self::Term(term)
                    if term.op.is_ordering()
                        && term
                            .compared
                            .as_ref()
                            .is_some_and(|c| c.statistics.bounds.is_some()) =>
                {
                    out.push(term);
                }
                Self::And(children) => pending.extend(children.iter().rev()),
                Self::Term(_) | Self::In(_) | Self::Or(_) | Self::Not(_) => {}
            }
        }
        out
    }

    fn collect_notes(&self, out: &mut Vec<ComparisonNote>) {
        match self {
            Self::Term(term) => out.extend(term.comparison_notes()),
            Self::In(membership) => out.extend(membership.comparison_notes()),
            Self::And(children) | Self::Or(children) => {
                for child in children {
                    child.collect_notes(out);
                }
            }
            Self::Not(inner) => inner.collect_notes(out),
        }
    }
}

/// A set of [`Truth`] values — what the rows of one row group could make a
/// term or a tree evaluate to, read off the group's statistics instead of its
/// rows ([`ResolvedExpr::truths`]).
///
/// A set rather than a "may match" flag because of [`Expr::Not`]: a flag
/// saying `True` is impossible under a `Not` says nothing about whether
/// `False` is, which is what the negation turns into `True`. Carrying all
/// three keeps every node's answer an over-approximation its parent can
/// combine soundly, `Unknown` included (`docs/design/decisions.md`, "D54").
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TruthSet(u8);

/// The members, as a set: `{True, Unknown}`.
impl std::fmt::Debug for TruthSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.members()).finish()
    }
}

impl TruthSet {
    /// No value at all — the answer over a group no row starts in.
    pub(crate) const EMPTY: Self = Self(0);
    /// `True` and `False`: a comparison over a non-NULL value it knows
    /// nothing more about.
    const TWO_VALUED: Self = Self(Self::bit(Truth::True) | Self::bit(Truth::False));
    const MEMBERS: [Truth; 3] = [Truth::True, Truth::False, Truth::Unknown];

    const fn bit(truth: Truth) -> u8 {
        match truth {
            Truth::True => 1,
            Truth::False => 2,
            Truth::Unknown => 4,
        }
    }

    pub(crate) fn of(truth: Truth) -> Self {
        Self(Self::bit(truth))
    }

    /// Whether some row could evaluate to `truth`. A group the evaluator
    /// answers without [`Truth::True`] holds no row a filter keeps.
    pub(crate) fn contains(self, truth: Truth) -> bool {
        self.0 & Self::bit(truth) != 0
    }

    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    fn members(self) -> impl Iterator<Item = Truth> {
        Self::MEMBERS.into_iter().filter(move |t| self.contains(*t))
    }

    /// Kleene's `f` lifted to sets: every value `f` takes on a member of each.
    fn lift(self, other: Self, f: impl Fn(Truth, Truth) -> Truth) -> Self {
        self.members()
            .flat_map(|a| other.members().map(move |b| (a, b)))
            .fold(Self::EMPTY, |set, (a, b)| set.union(Self::of(f(a, b))))
    }

    fn and(self, other: Self) -> Self {
        self.lift(other, |a, b| match (a, b) {
            (Truth::False, _) | (_, Truth::False) => Truth::False,
            (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
            _ => Truth::True,
        })
    }

    fn or(self, other: Self) -> Self {
        self.lift(other, |a, b| match (a, b) {
            (Truth::True, _) | (_, Truth::True) => Truth::True,
            (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
            _ => Truth::False,
        })
    }

    fn not(self) -> Self {
        self.members().fold(Self::EMPTY, |set, t| set.union(Self::of(t.not())))
    }

    fn from_flags(can_be_true: bool, can_be_false: bool) -> Self {
        let pick = |on: bool, truth| if on { Self::of(truth) } else { Self::EMPTY };
        pick(can_be_true, Truth::True).union(pick(can_be_false, Truth::False))
    }
}

/// One row group's statistics, as the truth-set evaluator reads them. A
/// column is numbered the way a raw row's fields are — by the block's
/// **unprojected** column list, which is what [`ResolvedTerm`]'s index is.
///
/// Every statistic is optional per column: a column the group gathered
/// nothing for answers `None` to each, and every term over it answers what
/// any row might.
pub(crate) trait GroupStatistics {
    /// How many rows start in the group. Zero is a group no row starts in,
    /// over which every term is [`TruthSet::EMPTY`].
    fn rows(&self) -> u64;

    /// How many of those rows hold NULL in `column`.
    fn null_count(&self, column: usize) -> Option<u64>;

    /// A lower and an upper bound on `column`'s non-NULL values, as unescaped
    /// field text, from the stored set ordered as `kind` orders
    /// ([`crate::pgtype::bounds_set_keyed_by`]): no value's key under `kind`
    /// is below `min`'s or above `max`'s.
    ///
    /// **A bound need not be a value the group holds**, and is never read as
    /// one: a bound truncated past a storage cap is read exactly as an exact
    /// one is, so it only has to be on the right side.
    fn bounds(&self, column: usize, kind: &CompareKind) -> Option<(&str, &str)>;

    /// Every distinct text among `column`'s non-NULL values, as unescaped
    /// field text — complete, or `None`.
    fn dictionary(&self, column: usize) -> Option<impl Iterator<Item = &str>>;

    /// How many of those rows hold, in `column`, a value its type cannot
    /// hold, per tier — `None` where the group did not count them.
    fn unrepresentable(&self, column: usize) -> Option<Unrepresentable>;
}

/// One row group's bounds on one column in one kind, read as keys once for
/// each run of terms reading them one after another — a membership's `=`
/// terms all read its column's, so a list is keyed against a group once
/// rather than once per value. It holds the last column and kind asked for, and lives no longer
/// than one group's answer, a group's bounds on a column in a kind being one
/// pair of texts.
#[derive(Default)]
struct KeyedBounds<'a> {
    /// The column and kind last asked for, `None` before the first ask.
    of: Option<(usize, &'a CompareKind)>,
    /// Their bounds as keys, `None` where either does not key.
    keys: Option<(OrderKey, OrderKey)>,
}

impl<'a> KeyedBounds<'a> {
    /// `min` and `max`, the group's bounds on `column` ordered as `kind`
    /// orders, as keys — `None` where either does not key.
    fn keys(
        &mut self,
        column: usize,
        kind: &'a CompareKind,
        min: &str,
        max: &str,
    ) -> Option<(&OrderKey, &OrderKey)> {
        if self.of != Some((column, kind)) {
            self.keys = order_key(kind, min).zip(order_key(kind, max));
            self.of = Some((column, kind));
        }
        self.keys.as_ref().map(|(low, high)| (low, high))
    }
}

impl ResolvedTerm {
    /// Every value this term could take over a row of `group` — a superset
    /// of what [`Self::eval`] answers over each of its rows, and exact where
    /// the statistics are. What each operator reads is
    /// `docs/design/decisions.md`, "D75".
    ///
    /// **A row whose value is not of the column's type is outside it**: the
    /// row path raises `Error::FieldDecode` there rather than answering, and
    /// no statistic records that such a row exists.
    pub(crate) fn truths(&self, group: &impl GroupStatistics) -> TruthSet {
        self.truths_keyed(group, &mut KeyedBounds::default())
    }

    /// [`Self::truths`], reading `group`'s bounds as keys out of `keyed`,
    /// which keys them once for every term of a tree reading them.
    fn truths_keyed<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        let rows = group.rows();
        // Off the group's own count, never its NULL count, which under the
        // null mode's view counts the values the test matches.
        if self.op.tests_unrepresentable() {
            let matched = match (&self.tested, group.unrepresentable(self.index)) {
                (None, _) => Some(0),
                (Some(test), Some(count)) => Some(test.counted(count)),
                (Some(_), None) => None,
            };
            let (some, all) = matched.map_or((rows > 0, false), |n| (n > 0, n >= rows));
            let (can_match, can_miss) = (some, rows > 0 && !all);
            return match self.op {
                PredicateOp::IsUnrepresentable => TruthSet::from_flags(can_match, can_miss),
                _ => TruthSet::from_flags(can_miss, can_match),
            };
        }
        let (nulls, values) = match group.null_count(self.index) {
            Some(nulls) => (nulls > 0, nulls < rows),
            None => (rows > 0, rows > 0),
        };
        let mut set = TruthSet::EMPTY;
        if nulls {
            set = set.union(TruthSet::of(self.eval_value(None).expect("a NULL always answers")));
        }
        if values {
            set = set.union(self.value_truths(group, keyed));
        }
        set
    }

    /// What this term could answer over a **non-NULL** value of `group`.
    ///
    /// The dictionary and the bounds each give a superset of the answer, so
    /// where the term reads both it takes what they agree on.
    fn value_truths<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        let Some(compared) = self.compared.as_ref() else {
            return TruthSet::of(Truth::of(self.op == PredicateOp::IsNotNull));
        };
        let mut set = TruthSet::TWO_VALUED;
        if compared.statistics.dictionary
            && let Some(entries) = group.dictionary(self.index)
        {
            // Through `eval_value`, the row path's own comparison, so an
            // entry is answered exactly as a row holding it is. An empty
            // dictionary beside a non-NULL row contradicts itself and is
            // read as none.
            let answered = entries
                .map(|entry| {
                    self.eval_value(Some(entry)).map_or(TruthSet::TWO_VALUED, TruthSet::of)
                })
                .reduce(TruthSet::union);
            if let Some(answered) = answered {
                set = set.intersection(answered);
            }
        }
        if let Some((kind, literal)) = compared.statistics.bounds.as_deref()
            && let Some((min, max)) = group.bounds(self.index, kind)
        {
            set = set.intersection(match keyed.keys(self.index, kind, min, max) {
                Some((low, high)) => self.bounded(literal, low, high),
                // A bound that does not key allows anything.
                None => TruthSet::TWO_VALUED,
            });
        }
        set
    }

    /// What bounds keyed as `low` and `high` allow this term to answer over
    /// a value between them.
    ///
    /// The four ordering operators are monotone in the value, so each end
    /// settles one truth. **The equality operators only ever rule out
    /// "equal"**, when the literal lies outside the bounds: that no value is
    /// *unequal* would need every value to be the literal, and bounds equal
    /// to it say so of keys, where a canonicalized comparison reads spellings
    /// — one per key only in text `*_out` wrote
    /// (`docs/design/decisions.md`, "D56").
    fn bounded(&self, literal: &OrderKey, low: &OrderKey, high: &OrderKey) -> TruthSet {
        let low = compare_keys(low, literal);
        let high = compare_keys(high, literal);
        let (can_be_true, can_be_false) = match self.op {
            PredicateOp::Lt => (low.is_lt(), high.is_ge()),
            PredicateOp::Le => (low.is_le(), high.is_gt()),
            PredicateOp::Gt => (high.is_gt(), low.is_le()),
            PredicateOp::Ge => (high.is_ge(), low.is_lt()),
            _ => {
                let can_be_equal = low.is_le() && high.is_ge();
                if self.wants_equal() { (can_be_equal, true) } else { (true, can_be_equal) }
            }
        };
        TruthSet::from_flags(can_be_true, can_be_false)
    }
}

impl ResolvedExpr {
    /// Every value this tree could take over a row of `group`, combining its
    /// terms' sets through Kleene's tables — so a group whose answer lacks
    /// [`Truth::True`] holds no row [`Self::matches`] keeps.
    ///
    /// Terms are combined as if independent, which is where the answer stops
    /// being exact: `v < 5 AND v >= 5` cannot be true of any one row, and
    /// each term alone can.
    pub(crate) fn truths(&self, group: &impl GroupStatistics) -> TruthSet {
        self.truths_keyed(group, &mut KeyedBounds::default())
    }

    /// [`Self::truths`], every term reading the group's bounds out of
    /// `keyed`, so terms reading a column's in one kind one after another
    /// key them once.
    fn truths_keyed<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        match self {
            Self::Term(term) => term.truths_keyed(group, keyed),
            Self::In(membership) => membership.truths_keyed(group, keyed),
            Self::And(children) => children.iter().fold(TruthSet::of(Truth::True), |set, child| {
                set.and(child.truths_keyed(group, keyed))
            }),
            Self::Or(children) => children.iter().fold(TruthSet::of(Truth::False), |set, child| {
                set.or(child.truths_keyed(group, keyed))
            }),
            Self::Not(inner) => inner.truths_keyed(group, keyed).not(),
        }
    }
}

#[cfg(test)]
mod tests;
