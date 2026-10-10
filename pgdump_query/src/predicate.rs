//! Post-parse row filtering (`docs/design/decisions.md`, "Predicates").

use std::cmp::Ordering;

use arrow::datatypes::i256;

use crate::copy::{RawRow, RowSplit};
use crate::datetime_in;
use crate::datetime_in::DateTimeInput;
use crate::decode;
use crate::decode::{NumericColumn, Read, Unread};
#[cfg(feature = "introspect")]
use crate::instrument::timed_span;
use crate::instrument::{EvaluationPart as Part, timed};
use crate::model::{Finding, PostgresInvalidValues, Severity, Unrepresentable};
use crate::nested;
use crate::pgtype::{
    CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics, Discrete, Geometric,
    IntervalQualifier, NestedCompare, Position, TextGrammar, UnanswerableReason,
    datafusion_position_divergences,
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

/// One side of a bare-`numeric` comparison: the sign, and the digits either
/// side of the point with the *insignificant* ones removed — leading zeros
/// from the integer part, trailing zeros from the fraction.
///
/// Normalizing is what makes this PostgreSQL's own order: `cmp_numerics`
/// compares by value and never by display scale (I33), so `1.5` and `1.50`
/// are one value that a bare `numeric` column writes two ways.
///
/// Digit *strings* rather than a big integer, because the column is
/// arbitrary-precision: the file may hold a thousand digits, past every
/// fixed-width type including `i256`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NumericKey {
    /// False for a zero of either written sign — PostgreSQL has one zero, and
    /// a filter literal is free to spell it `-0`.
    negative: bool,
    /// Integer digits, leading zeros stripped; empty for a magnitude below 1.
    int: String,
    /// Fraction digits, trailing zeros stripped; empty for an integer.
    frac: String,
}

impl NumericKey {
    /// The heap its two digit strings hold.
    fn heap_bytes(&self) -> u64 {
        (self.int.capacity() + self.frac.capacity()) as u64
    }

    /// `[-]digits[.digits]`: no exponent, no sign but `-`, at least one digit
    /// somewhere, and a point at either end or leading zeros allowed, which
    /// `numeric_out` never writes ([`accepted_form`] lists them). It is the same
    /// *lexical* grammar [`decode::typmod_unscaled_digits`] accepts for a
    /// typmod'd column's field, which is read through it and
    /// [`Self::from_unscaled`] instead; this reads a bare column's field and
    /// every literal. The storage format's bounds are the caller's
    /// ([`decode::numeric_in_stores`]), a typmod'd column's field never
    /// reaching them.
    fn parse(text: &str) -> Option<Self> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self::from_parts(negative, int, frac))
    }

    /// An unscaled digit string at `scale` places, as
    /// [`decode::typmod_unscaled_digits`] writes it, split at its point.
    fn from_unscaled(unscaled: &str, scale: i16) -> Self {
        let (negative, digits) = match unscaled.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, unscaled),
        };
        match usize::try_from(scale) {
            Ok(places) if places >= digits.len() => Self::from_parts(
                negative,
                "",
                &format!("{}{digits}", "0".repeat(places - digits.len())),
            ),
            Ok(places) => {
                let (int, frac) = digits.split_at(digits.len() - places);
                Self::from_parts(negative, int, frac)
            }
            Err(_) => {
                let zeros = "0".repeat(usize::from(scale.unsigned_abs()));
                Self::from_parts(negative, &format!("{digits}{zeros}"), "")
            }
        }
    }

    /// Drop the insignificant digits from an already-split sign/integer part/
    /// fraction, which is the whole of the normalization. Shared: a `numeric`
    /// field is split straight out of the text, while a `jsonb` number's
    /// point has to be moved by its exponent first.
    fn from_parts(negative: bool, int: &str, frac: &str) -> Self {
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        Self {
            negative: negative && !(int.is_empty() && frac.is_empty()),
            int: int.to_string(),
            frac: frac.to_string(),
        }
    }

    /// Sign first, then magnitude: how many integer digits, then those digits,
    /// then the fraction. Each stage is a byte comparison over ASCII digits,
    /// which is why the normalization above has to have happened — with
    /// trailing zeros stripped, a fraction that is a prefix of another is the
    /// smaller of the two.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => return Ordering::Greater,
            (true, false) => return Ordering::Less,
            _ => {}
        }
        let magnitude = self
            .int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.as_bytes().cmp(other.int.as_bytes()))
            .then_with(|| self.frac.as_bytes().cmp(other.frac.as_bytes()));
        if self.negative { magnitude.reverse() } else { magnitude }
    }
}

/// One side of an `inet`/`cidr` comparison: the family, the netmask length,
/// and the address left-aligned in sixteen bytes.
///
/// Not a byte key, and it cannot be made into one: `network_cmp_internal`
/// compares the *shorter* netmask's worth of address bits first (I40), so how
/// many bits are significant depends on the value being compared against.
/// The pair is compared, not two independently sortable keys.
///
/// `v6` is the family; `PGSQL_AF_INET6` is `PGSQL_AF_INET + 1`, so an IPv4
/// address sorts below every IPv6 one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkKey {
    v6: bool,
    bits: u8,
    /// The address, left-aligned: an IPv4 address occupies the first four
    /// bytes and the rest are zero, which is what `bitncmp` reads, never
    /// looking past `maxbits`.
    addr: [u8; 16],
}

impl NetworkKey {
    /// `ip_maxbits` — the family's address width.
    fn maxbits(&self) -> u8 {
        if self.v6 { 128 } else { 32 }
    }

    /// `network_cmp_internal` (I40): family, then the shorter netmask's
    /// worth of address bits, then the netmask length, then the whole
    /// address.
    fn cmp(&self, other: &Self) -> Ordering {
        if self.v6 != other.v6 {
            return self.v6.cmp(&other.v6);
        }
        let shared = self.bits.min(other.bits);
        bitncmp(&self.addr, &other.addr, shared)
            .then_with(|| self.bits.cmp(&other.bits))
            .then_with(|| bitncmp(&self.addr, &other.addr, self.maxbits()))
    }
}

/// PostgreSQL's `bitncmp`: the first `n` bits of two addresses, most
/// significant first. Whole bytes by `memcmp`, then the straddling byte's
/// remaining bits under a high-bit mask — the same answer as `bitncmp`'s
/// bit-at-a-time loop, which stops at the first differing bit.
fn bitncmp(left: &[u8; 16], right: &[u8; 16], n: u8) -> Ordering {
    let whole = usize::from(n / 8);
    let full = left[..whole].cmp(&right[..whole]);
    let spare = n % 8;
    if full != Ordering::Equal || spare == 0 {
        return full;
    }
    let mask = 0xffu8 << (8 - spare);
    (left[whole] & mask).cmp(&(right[whole] & mask))
}

/// One `jsonb` value, in the shape PostgreSQL stores and compares one in.
///
/// The variants are `JsonbValue`'s own type codes, in their order, which is
/// the fallback order `compareJsonbContainers` uses whenever two positions
/// hold different kinds (I41): an object outranks an array, an array outranks
/// every scalar, and a boolean outranks a number.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Jsonb {
    Null,
    String(String),
    /// A JSON number is stored as a `numeric` and compared by `numeric_cmp`,
    /// which is exactly [`NumericKey`] — so `1`, `1.0` and `1e0` are one
    /// value, as they are for a bare `numeric` column.
    Number(NumericKey),
    Bool(bool),
    /// `raw_scalar` marks the pseudo-array a top-level scalar is stored in.
    /// `compareJsonbContainers` tests it before the element count and lets
    /// the count *overwrite* the answer, so a scalar sorts below a one- or
    /// many-element array and **above an empty one** (I41). Only
    /// [`jsonb_key`] ever sets it; a nested array is a real array at every
    /// depth.
    Array {
        raw_scalar: bool,
        items: Vec<Jsonb>,
    },
    /// Pairs in **storage** order — key length first, then bytes — which is
    /// the order the walk visits them in and is *not* the order the keys are
    /// compared in ([`Jsonb::cmp`]).
    Object(Vec<(String, Jsonb)>),
}

impl Jsonb {
    /// The heap this document holds, every nested container and string
    /// included.
    fn heap_bytes(&self) -> u64 {
        match self {
            Self::Null | Self::Bool(_) => 0,
            Self::String(text) => text.capacity() as u64,
            Self::Number(numeric) => numeric.heap_bytes(),
            Self::Array { items, .. } => {
                (items.capacity() * std::mem::size_of::<Jsonb>()) as u64
                    + items.iter().map(Self::heap_bytes).sum::<u64>()
            }
            Self::Object(pairs) => {
                (pairs.capacity() * std::mem::size_of::<(String, Jsonb)>()) as u64
                    + pairs
                        .iter()
                        .map(|(key, value)| key.capacity() as u64 + value.heap_bytes())
                        .sum::<u64>()
            }
        }
    }

    /// `JsonbValue.type`, which is the type-defined order itself (I41).
    fn rank(&self) -> u8 {
        match self {
            Self::Null => 0x0,
            Self::String(_) => 0x1,
            Self::Number(_) => 0x2,
            Self::Bool(_) => 0x3,
            Self::Array { .. } => 0x10,
            Self::Object(_) => 0x11,
        }
    }

    /// `compareJsonbContainers`, as a recursion rather than as a lockstep walk
    /// over two token streams; the two agree because both stop at the first
    /// position where the values differ. An object is ordered by its pair
    /// *count* before its first key, and its keys are compared by
    /// `varstr_cmp` while being *stored* by length-then-bytes (I41).
    ///
    /// A string leaf is where this stops being PostgreSQL's answer:
    /// `compareJsonbScalarValue` orders it by the database's collation, which
    /// a plain dump does not record (I32), and
    /// `ComparisonDivergence::JsonbStringCollation` announces that.
    fn cmp(&self, other: &Self) -> Ordering {
        if self.rank() != other.rank() {
            return self.rank().cmp(&other.rank());
        }
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::String(a), Self::String(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (
                Self::Array { raw_scalar: raw_a, items: a },
                Self::Array { raw_scalar: raw_b, items: b },
            ) => {
                // Written in the source's own order, overwrite included: the
                // flag is decided first and the length then replaces the
                // answer rather than refining it.
                let mut container = Ordering::Equal;
                if raw_a != raw_b {
                    container = if *raw_a { Ordering::Less } else { Ordering::Greater };
                }
                if a.len() != b.len() {
                    container = a.len().cmp(&b.len());
                }
                container.then_with(|| first_difference(a.iter().zip(b).map(|(x, y)| x.cmp(y))))
            }
            (Self::Object(a), Self::Object(b)) => a.len().cmp(&b.len()).then_with(|| {
                first_difference(a.iter().zip(b).map(|((ka, va), (kb, vb))| {
                    ka.as_bytes().cmp(kb.as_bytes()).then_with(|| va.cmp(vb))
                }))
            }),
            _ => unreachable!("two `Jsonb` values of one rank are of one variant"),
        }
    }
}

/// The first non-`Equal` answer, or `Equal` when there is none — the
/// member-wise half of [`Jsonb::cmp`].
fn first_difference(mut answers: impl Iterator<Item = Ordering>) -> Ordering {
    answers.find(|answer| answer.is_ne()).unwrap_or(Ordering::Equal)
}

/// How deep [`parse_jsonb`] will descend before refusing; a fixed number
/// rather than a stack-depth check (`docs/design/decisions.md`, "D55").
/// Nothing a `jsonb_out` field of a real dump holds comes near it.
const JSONB_MAX_DEPTH: usize = 1000;

/// The largest exponent `numeric_in` reads in a `jsonb` number, either sign:
/// `PG_INT32_MAX / 2` (I63).
const NUMERIC_EXPONENT_MAX: i64 = i32::MAX as i64 / 2;

/// A recursive-descent reader over one JSON document, implementing what
/// `jsonb_in` accepts and nothing wider (I41): RFC 8259 with PostgreSQL's two
/// extra refusals — `\u0000`, which cannot be part of a `text` value, and a
/// lone surrogate half.
struct JsonCursor<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
    /// Whether the read failed at a refusal of the server's, a marked one,
    /// rather than anywhere else ([`Unread`]).
    refused: bool,
}

impl<'a> JsonCursor<'a> {
    /// The four bytes RFC 8259 calls whitespace, which is also what the
    /// server's lexer skips — not `char::is_whitespace`, and not
    /// `array_isspace` either.
    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.bytes.get(self.at) == Some(&byte);
        self.at += usize::from(found);
        found
    }

    fn keyword(&mut self, word: &str) -> Option<()> {
        let end = self.at + word.len();
        (self.bytes.get(self.at..end)? == word.as_bytes()).then(|| self.at = end)
    }

    /// A run of one or more ASCII digits, as the number grammar's three
    /// positions all want.
    fn digits(&mut self) -> Option<&'a str> {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        if self.at == start {
            return None;
        }
        // ASCII digits, so this can only be valid UTF-8.
        std::str::from_utf8(&self.bytes[start..self.at]).ok()
    }

    /// One value, and the only place the depth is counted.
    fn value(&mut self) -> Option<Jsonb> {
        self.depth += 1;
        if self.depth > JSONB_MAX_DEPTH {
            return None;
        }
        let value = match self.bytes.get(self.at)? {
            b'{' => {
                self.at += 1;
                self.object()?
            }
            b'[' => {
                self.at += 1;
                self.array()?
            }
            b'"' => Jsonb::String(self.string()?),
            b't' => {
                self.keyword("true")?;
                Jsonb::Bool(true)
            }
            b'f' => {
                self.keyword("false")?;
                Jsonb::Bool(false)
            }
            b'n' => {
                self.keyword("null")?;
                Jsonb::Null
            }
            // Anything else is a number or nothing: a leading `+`, a bare
            // `.5` and `NaN` all fail inside, as the server's lexer fails
            // them, so every failure there is a refusal.
            _ => match self.number() {
                Some(number) => Jsonb::Number(number),
                None => return self.refuse(),
            },
        };
        self.depth -= 1;
        Some(value)
    }

    /// The body of an array, its `[` already eaten.
    fn array(&mut self) -> Option<Jsonb> {
        let mut items = Vec::new();
        self.skip_ws();
        if !self.eat(b']') {
            loop {
                self.skip_ws();
                items.push(self.value()?);
                self.skip_ws();
                if self.eat(b',') {
                    continue;
                }
                if self.eat(b']') {
                    break;
                }
                return None;
            }
        }
        Some(Jsonb::Array { raw_scalar: false, items })
    }

    /// The body of an object, its `{` already eaten. The pairs come out in
    /// storage order with duplicate keys resolved, which is the state a
    /// stored `jsonb` is always in.
    fn object(&mut self) -> Option<Jsonb> {
        let mut pairs: Vec<(String, Jsonb)> = Vec::new();
        self.skip_ws();
        if !self.eat(b'}') {
            loop {
                self.skip_ws();
                let key = self.string()?;
                self.skip_ws();
                if !self.eat(b':') {
                    return None;
                }
                self.skip_ws();
                pairs.push((key, self.value()?));
                self.skip_ws();
                if self.eat(b',') {
                    continue;
                }
                if self.eat(b'}') {
                    break;
                }
                return None;
            }
        }
        Some(Jsonb::Object(storage_order(pairs)))
    }

    /// A JSON string, its escapes resolved. An unescaped byte below `0x20` is
    /// refused, which is what the server refuses and what `escape_json` never
    /// writes — it renders every one of them as `\b`/`\f`/`\n`/`\r`/`\t` or a
    /// `\u00xx` (I41).
    // pg-refuses: I41 — an unescaped byte below `0x20`.
    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let byte = *self.bytes.get(self.at)?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let escape = *self.bytes.get(self.at)?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut buffer = [0u8; 4];
                            out.extend_from_slice(
                                self.unicode_escape()?.encode_utf8(&mut buffer).as_bytes(),
                            );
                        }
                        _ => return None,
                    }
                }
                0x00..=0x1f => return self.refuse(),
                // Every other byte is copied verbatim, and each is ASCII or
                // part of a multi-byte sequence copied whole — the input is a
                // `&str`, so `from_utf8` above never fails.
                _ => out.push(byte),
            }
        }
    }

    /// `\uXXXX`, its `\u` already eaten. Surrogates must come as a matched
    /// high-then-low pair, and `\u0000` is refused outright: `jsonb` stores
    /// strings as `text`, which cannot hold a NUL (I41).
    // pg-refuses: I41 — `\u0000`, and a surrogate out of its pair.
    fn unicode_escape(&mut self) -> Option<char> {
        let first = self.hex4()?;
        if first == 0 {
            return self.refuse();
        }
        if (0xd800..0xdc00).contains(&first) {
            if !(self.eat(b'\\') && self.eat(b'u')) {
                return self.refuse();
            }
            let second = self.hex4()?;
            if !(0xdc00..0xe000).contains(&second) {
                return self.refuse();
            }
            return char::from_u32(0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00));
        }
        // A low surrogate with no high half in front of it.
        if (0xdc00..0xe000).contains(&first) {
            return self.refuse();
        }
        char::from_u32(first)
    }

    /// Fail the read as the server refuses it.
    fn refuse<T>(&mut self) -> Option<T> {
        self.refused = true;
        None
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.at..self.at.checked_add(4)?)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        self.at += 4;
        u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
    }

    /// A JSON number: an optional `-`, then `0` or a digit run that does not
    /// start with `0`, then an optional `.` with at least one digit, then an
    /// optional `e`/`E` exponent with an optional sign. `01`, `+1`, `.5`,
    /// `1.` and `NaN` are refused, as the server refuses them (I41).
    ///
    /// The result is the [`NumericKey`] the stored `numeric` would compare by,
    /// so the exponent is applied by moving the decimal point rather than
    /// kept: `1e2`, `100` and `100.00` are one value (I41). `numeric_in`'s
    /// bounds are checked before the point moves, which keeps the digit string
    /// this builds within them (I63).
    // pg-refuses: I41 — `01`, `+1`, `.5`, `1.` and `NaN`.
    fn number(&mut self) -> Option<NumericKey> {
        let negative = self.eat(b'-');
        let int = self.digits()?;
        if int.len() > 1 && int.starts_with('0') {
            return None;
        }
        let frac = if self.eat(b'.') { self.digits()? } else { "" };
        let mut exponent: i64 = 0;
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            let signed = if self.eat(b'-') {
                true
            } else {
                self.eat(b'+');
                false
            };
            let digits = self.digits()?;
            // Parsed as `i64` and then bounded, so a run of digits too long
            // for the integer is refused rather than wrapping.
            exponent = digits.parse::<i64>().ok()?;
            if signed {
                exponent = -exponent;
            }
        }
        // The display scale is the fraction's digits less the exponent, and
        // the weight counts from the first non-zero digit: a zero has none,
        // and is stored whatever its exponent moves.
        // pg-refuses: I63 — an exponent past `PG_INT32_MAX / 2`, or a display
        // scale or a weight past the storage format's.
        if exponent.abs() > NUMERIC_EXPONENT_MAX
            || i64::try_from(frac.len()).ok()? - exponent > decode::NUMERIC_DSCALE_MAX as i64
        {
            return None;
        }
        let digits = format!("{int}{frac}");
        let Some(first) = digits.bytes().position(|b| b != b'0') else {
            return Some(NumericKey::from_parts(false, "", ""));
        };
        // Where the point lands, counted in digits from the left of
        // `int ++ frac`. The two bounds above bound the padding either side.
        let point = i64::try_from(int.len()).ok()? + exponent;
        if point - i64::try_from(first).ok()? > decode::NUMERIC_INTEGER_DIGITS_MAX as i64 {
            return None;
        }
        let width = i64::try_from(digits.len()).ok()?;
        Some(match point {
            _ if point <= 0 => {
                NumericKey::from_parts(negative, "", &format!("{}{digits}", zeros(-point)))
            }
            _ if point >= width => {
                NumericKey::from_parts(negative, &format!("{digits}{}", zeros(point - width)), "")
            }
            _ => {
                let split = usize::try_from(point).ok()?;
                NumericKey::from_parts(negative, &digits[..split], &digits[split..])
            }
        })
    }
}

/// `n` zeros, for the padding a moved decimal point needs on either side.
fn zeros(n: i64) -> String {
    "0".repeat(usize::try_from(n).unwrap_or(0))
}

/// `uniqueifyJsonbObject`: sort the pairs into storage order and drop every
/// duplicate key but the **last** one written.
///
/// Storage order is `lengthCompareJsonbString` — key length first, then
/// `memcmp` — so `{"z":1,"aa":2}` is stored, printed and walked in that order
/// rather than alphabetically (I41). The duplicate rule comes out of
/// `lengthCompareJsonbPair` breaking a tie on *descending* insertion order
/// while the uniqueify pass keeps the first of each run, which the `reverse`
/// here reproduces against a stable sort.
fn storage_order(mut pairs: Vec<(String, Jsonb)>) -> Vec<(String, Jsonb)> {
    pairs.reverse();
    pairs.sort_by(|(a, _), (b, _)| {
        a.len().cmp(&b.len()).then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });
    pairs.dedup_by(|(a, _), (b, _)| a == b);
    pairs
}

/// One `jsonb` document, parsed. `Err` for text the server's own parser would
/// refuse — which for a *field* is `Error::FieldDecode` and for a filter's
/// literal `Error::PredicateValueDecode`, the same two faults every other
/// comparison raises — and for a document nested past [`JSONB_MAX_DEPTH`],
/// [`Unread::Unparsed`], the server's bound being its `max_stack_depth`.
fn parse_jsonb(text: &str) -> Read<Jsonb> {
    let mut cursor = JsonCursor { bytes: text.as_bytes(), at: 0, depth: 0, refused: false };
    cursor.skip_ws();
    let value = cursor.value();
    cursor.skip_ws();
    match value {
        Some(value) if cursor.at == cursor.bytes.len() => Ok(value),
        _ if cursor.refused => Err(Unread::Refused),
        _ => Err(jsonb_unread(text)),
    }
}

/// Why `jsonb_in` reads no value here from `text`, [`JsonCursor`] having read
/// none and refused nothing: `jsonb_in`'s grammar is `json_in`'s, which the
/// cursor reads but past its depth, with refusals of its own besides, so a
/// text `json_in` refuses is refused ([`decode::json_in`]).
// pg-refuses: I41 — every text `json_in` refuses.
fn jsonb_unread(text: &str) -> Unread {
    if decode::json_in(text) { Unread::Unparsed } else { Unread::Refused }
}

/// A `jsonb` comparison key: the document, with a **top-level scalar wrapped
/// in the one-element pseudo-array PostgreSQL stores it in** (I41). Modelling
/// the wrapper is what gives `1 < [1]`, `1 < [1,2]` and `1 > []`.
fn jsonb_key(text: &str) -> Read<OrderKey> {
    Ok(OrderKey::Jsonb(match parse_jsonb(text)? {
        container @ (Jsonb::Array { .. } | Jsonb::Object(_)) => container,
        scalar => Jsonb::Array { raw_scalar: true, items: vec![scalar] },
    }))
}

/// One side of an ordering comparison, decoded from text per the column's
/// [`CompareKind`]. Both sides of any one comparison come from the same kind,
/// so a *finite* variant mismatch is unreachable by construction.
///
/// Three variants are not values of the column's Arrow type at all:
/// `infinity`, `-infinity` and `numeric`'s `NaN` are legal values of their
/// declared PostgreSQL types with a total order (I34), and are carried as
/// their position in that order (`docs/design/decisions.md`, "D56"). A
/// `real`/`double precision` special is not here: IEEE has all three, so the
/// column's own decoder yields them inside `Float` and [`pg_float_cmp`]
/// orders them PostgreSQL's way.
#[derive(Debug, Clone, PartialEq)]
enum OrderKey {
    /// PostgreSQL's `-infinity`, below every finite value of its type.
    NegativeInfinity,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(i256),
    /// A bare `numeric`, whose digits do not fit any fixed-width integer.
    Numeric(NumericKey),
    /// An `interval`'s span in microseconds, which is 128 bits wide because
    /// PostgreSQL's own `interval_cmp_value` is (I40).
    Interval(i128),
    /// An `interval`'s three fields as `Interval(MonthDayNano)` holds them —
    /// months, days, microseconds — compared one after another, as Arrow
    /// compares that type.
    IntervalFields(i32, i32, i64),
    /// A `time with time zone`: the UTC-equivalent instant, then the stored
    /// zone as PostgreSQL stores it — seconds *west* of GMT, the negation of
    /// the sign the value displays.
    TimeTz {
        utc: i64,
        zone: i64,
    },
    Network(NetworkKey),
    /// A `jsonb` document, with a top-level scalar already wrapped in the
    /// pseudo-array PostgreSQL stores it in — see [`jsonb_key`].
    Jsonb(Jsonb),
    Bytes(Vec<u8>),
    Text(String),
    /// PostgreSQL's `infinity`, above every finite value of its type.
    PositiveInfinity,
    /// `numeric`'s `NaN`, which orders above `infinity` and equals itself
    /// (I34). Reached through [`CompareKind::Decimal`] and
    /// [`CompareKind::Numeric`].
    NotANumber,
}

/// The rank of a finite value — the second of the four [`OrderKey::rank`]
/// classes, above `-infinity` and below `infinity` and `NaN`, and the only one whose members are compared by value.
const FINITE: u8 = 1;

impl OrderKey {
    /// Where this key sits in PostgreSQL's total order relative to the finite
    /// values of its own type. Ranks are compared before values are, so a
    /// special value never collides with a finite one a sentinel could have —
    /// `i64::MAX` micros since 1970 is a date PostgreSQL itself accepts
    /// (`docs/design/decisions.md`, "D56").
    fn rank(&self) -> u8 {
        match self {
            Self::NegativeInfinity => 0,
            Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Decimal(_)
            | Self::Numeric(_)
            | Self::Interval(_)
            | Self::IntervalFields(..)
            | Self::TimeTz { .. }
            | Self::Network(_)
            | Self::Jsonb(_)
            | Self::Bytes(_)
            | Self::Text(_) => FINITE,
            Self::PositiveInfinity => 2,
            Self::NotANumber => 3,
        }
    }
}

/// PostgreSQL's special values, for the kinds whose columns can hold one and
/// in the exact spelling that type's own `*_out` writes (I34): `date`,
/// `timestamp`, `timestamptz` and `interval` write `infinity`/`-infinity`,
/// and a `numeric` writes `NaN`, a bare one its infinities too (below).
/// Nothing else is accepted — a `date` field or
/// literal reading `Infinity` is a decode failure
/// (`docs/design/decisions.md`, "D55").
///
/// `interval`'s two are read on every file: they are v17 values no older
/// server could have written, so accepting the spelling unconditionally is
/// the union rule (I35) rather than a claim about the file's own major.
/// `real`/`double precision` are absent because IEEE represents all three and
/// [`decode::decode_f64`] already returns them.
///
/// The two `numeric` kinds differ only about the infinities (I34): a
/// [`CompareKind::Decimal`] column, and a `numeric(p,s)` past 76 digits, can
/// hold a `NaN` and never an infinity, while a *bare* `numeric` has all three
/// in `numeric_out`'s own spellings, which capitalize where `date_out`'s do
/// not.
fn special_order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    match kind {
        CompareKind::Date
        | CompareKind::Timestamp { .. }
        | CompareKind::Interval { .. }
        | CompareKind::IntervalFields { .. } => match text {
            "infinity" => Some(OrderKey::PositiveInfinity),
            "-infinity" => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        CompareKind::Decimal { .. } => (text == "NaN").then_some(OrderKey::NotANumber),
        CompareKind::Numeric { infinities, .. } => match text {
            "NaN" => Some(OrderKey::NotANumber),
            "Infinity" if *infinities => Some(OrderKey::PositiveInfinity),
            "-Infinity" if *infinities => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        _ => None,
    }
}

/// `interval_cmp_value`'s span, in microseconds: months collapse to 30 days,
/// days to 86400 seconds, and the time field is added on (I40), so `1 mon`,
/// `30 days` and `720:00:00` are one value written three ways — which is why
/// an `interval` cannot canonicalize once and compare bytewise.
///
/// The walk over the text is [`decode::interval_parts`], shared with the
/// decoder, so a field the decoder refuses includes every literal this
/// refuses — and a time part past what Arrow's nanoseconds hold, besides. The
/// fusing is this function's alone: a decoder that did it would lose the
/// fields Arrow carries separately.
fn interval_span(text: &str) -> Read<i128> {
    let (months, days, time) = decode::interval_parts(text)?;
    let whole_days = i128::from(months) * 30 + i128::from(days);
    Ok(whole_days * 86_400_000_000 + i128::from(time))
}

/// A `time with time zone`, split into the UTC-equivalent instant and the
/// zone PostgreSQL stores — seconds *west* of GMT, the negation of the offset
/// the value displays. `timetz_cmp_internal` sorts by the first and breaks
/// ties with the second (I40), so `00:00:00+00` and `01:00:00+01` are the
/// same instant and still not equal.
fn timetz_key(text: &str) -> Read<OrderKey> {
    let (time_only, displayed) = decode::extract_offset(text)?;
    let (seconds, micros) = decode::parse_time_of_day(time_only)?;
    let zone = -displayed;
    // Both parts are bounded above, so neither sum can overflow.
    let utc = seconds * 1_000_000 + micros + zone * 1_000_000;
    Ok(OrderKey::TimeTz { utc, zone })
}

/// An `inet` or `cidr` value, as `network_in` reads it
/// ([`decode::network_in`]): `pg_inet_net_ntop`'s `<address>[/<bits>]`, with
/// `cidr_out` always appending the netmask and `inet_out` omitting it when it
/// is the family's full width (I40), and every other spelling the server
/// reads, a `cidr`'s abbreviated network among them. Every refusal is the
/// server's (I67), a `cidr`'s bit set below its netmask included: the *only*
/// thing separating the two types, their comparison being identical.
fn network_key(text: &str, cidr: bool) -> Read<OrderKey> {
    let (v6, bits, addr) = decode::network_in(text, cidr).ok_or(Unread::Refused)?;
    Ok(OrderKey::Network(NetworkKey { v6, bits, addr }))
}

/// A `macaddr`/`macaddr8` value: `octets` hex pairs of either case joined by
/// colons, `macaddr_out` and `macaddr8_out` writing lowercase (I40). The
/// server's input function takes other separator conventions and this takes
/// none of them (`docs/design/decisions.md`, "D55").
fn macaddr_key(text: &str, octets: usize) -> Option<OrderKey> {
    let mut bytes = Vec::with_capacity(octets);
    for part in text.split(':') {
        // The digit check is not what `from_str_radix` does: it accepts a
        // leading sign, so `+f` would otherwise pass as a two-character pair.
        if part.len() != 2 || bytes.len() == octets || !part.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        bytes.push(u8::from_str_radix(part, 16).ok()?);
    }
    (bytes.len() == octets).then_some(OrderKey::Bytes(bytes))
}

/// Decode one already-COPY-unescaped value into a comparable key. `None` when
/// the text is not a value of that type — for a *field* `Error::FieldDecode`,
/// for the filter's own literal `Error::PredicateValueDecode`, raised before
/// a row is read.
///
/// A special value is answered by [`special_order_key`] first: it is a legal
/// value of the declared type that the *Arrow* type cannot hold, a separate
/// population from text that is malformed for the column.
///
/// **A key is a function of the kind and the text alone**, and nothing here
/// or in [`compare_keys`] reads a row: two values that never shared one — a
/// field and a filter's literal, a stored bound and a literal, two neighbours
/// being gathered — are ordered by the one key the filter uses.
fn order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    field_key(kind, text).ok()
}

/// A field's key as a query reads the field: [`order_key`]'s, but for a float
/// its type's `*_in` refuses, keyed under [`PostgresInvalidValues::Ignore`] as
/// the typed read emits it ([`decode::float_field`]), so a filter and the
/// batch agree on it.
fn read_key(kind: &CompareKind, text: &str, invalid: PostgresInvalidValues) -> Option<OrderKey> {
    match (field_key(kind, text), kind) {
        (Ok(key), _) => Some(key),
        (Err(Unread::Refused), CompareKind::Float32) => {
            Some(OrderKey::Float(f64::from(decode::float_field::<f32>(text, invalid)?)))
        }
        (Err(Unread::Refused), CompareKind::Float64) => {
            Some(OrderKey::Float(decode::float_field::<f64>(text, invalid)?))
        }
        (Err(_), _) => None,
    }
}

/// Which side of a comparison [`nested_key`] reads: a filter's literal, or a
/// field, read as the query reads one its type's `*_in` refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Literal,
    Field(PostgresInvalidValues),
}

/// `NAMEDATALEN`: `enum_in` refuses a text of this many bytes or more before
/// looking it up, no label being so long (I70).
const ENUM_LABEL_LIMIT: usize = 64;

/// [`order_key`], saying why a text has no key: refused by the type's `*_in`,
/// which fails the parse reading it as a field, or not read here.
fn field_key(kind: &CompareKind, text: &str) -> Read<OrderKey> {
    if let Some(special) = special_order_key(kind, text) {
        return Ok(special);
    }
    let unparsed = Unread::Unparsed;
    Ok(match kind {
        CompareKind::Bool => {
            OrderKey::Bool(decode::decode_bool(text).ok_or_else(|| decode::bool_unread(text))?)
        }
        // A literal in DataFusion's semantics is read as an `i64` instead, by
        // [`literal_key`].
        CompareKind::Int { bytes } => OrderKey::Int(int_in(text, *bytes)?),
        // `u32`, and the width *is* the refusal: `oidin` reads `-1` as
        // 4294967295 and this build does not implement that wrap, so a
        // negative literal is `Error::PredicateValueDecode`. Every value the
        // column can hold widens into `i64` unchanged.
        CompareKind::UnsignedInt => {
            OrderKey::Int(text.parse::<u32>().map_err(|_| decode::oid_unread(text))?.into())
        }
        CompareKind::Float32 => OrderKey::Float(f64::from(decode::float_in::<f32>(text)?)),
        CompareKind::Float64 => OrderKey::Float(decode::float_in::<f64>(text)?),
        // A field is put through its typmod, rounded to the scale and refused
        // past the precision as `COPY` refuses it (I51). A literal is read
        // exactly, by [`literal_key`].
        CompareKind::Decimal { precision, scale } => OrderKey::Decimal(
            i256::from_string(&decode::typmod_unscaled_digits(
                text,
                *precision,
                i16::from(*scale),
            )?)
            .ok_or(unparsed)?,
        ),
        CompareKind::Numeric { typmod: Some(typmod), .. } => {
            OrderKey::Numeric(NumericKey::from_unscaled(
                &decode::typmod_unscaled_digits(text, typmod.precision, typmod.scale)?,
                typmod.scale,
            ))
        }
        // A bare column's field is bounded by `numeric_in` (I63) once read:
        // the bounds count the digits a grammar with no exponent reads, and
        // `numeric_in` stores a text whose exponent brings its scale back
        // within them. A typmod'd one never reaches the bounds, the typmod
        // rounding first. A literal in PostgreSQL's semantics is bounded by
        // [`literal_key`].
        CompareKind::Numeric { infinities: bare, typmod: None } => {
            let key = NumericKey::parse(text).ok_or_else(|| {
                let column = if *bare { NumericColumn::Bare } else { NumericColumn::UnreadTypmod };
                decode::numeric_unread(text, column)
            })?;
            if *bare && !decode::numeric_in_stores(text) {
                return Err(Unread::Refused);
            }
            OrderKey::Numeric(key)
        }
        // A label the type does not declare is no value of the column, which
        // `enum_in` refuses: where the declared labels are all the type
        // holds, or the text is longer than any label can be, and otherwise
        // it may be one this build did not read. The linear scan is over a
        // label list, a handful of entries in practice.
        // pg-refuses: I70
        CompareKind::Enum { labels, exact } => {
            let position = labels.iter().position(|l| l == text);
            let refused = *exact || text.len() >= ENUM_LABEL_LIMIT;
            OrderKey::Int(position.ok_or(if refused { Unread::Refused } else { unparsed })? as i64)
        }
        CompareKind::Date => {
            OrderKey::Int(datetime_read(decode::date_days(text), DateTimeInput::Date, text)?.into())
        }
        CompareKind::Time => OrderKey::Int(datetime_read(
            decode::time_of_day_micros(text),
            DateTimeInput::Time,
            text,
        )?),
        // From PostgreSQL's epoch, not Arrow's: every value the server admits
        // keys, those past what `i64` counts from 1970 among them (I49).
        CompareKind::Timestamp { with_tz } => {
            let input =
                if *with_tz { DateTimeInput::TimestampTz } else { DateTimeInput::Timestamp };
            OrderKey::Int(datetime_read(
                decode::timestamp_postgres_micros(text, *with_tz),
                input,
                text,
            )?)
        }
        // A field the reader does not read is refused where `interval_in`
        // refuses it under the column's qualifier. A literal is read under
        // none ([`literal_key`]).
        CompareKind::Interval { qualifier } => {
            OrderKey::Interval(interval_read(interval_span(text), text, *qualifier)?)
        }
        CompareKind::IntervalFields { qualifier } => {
            let (months, days, time) =
                interval_read(decode::interval_parts(text), text, *qualifier)?;
            OrderKey::IntervalFields(months, days, time)
        }
        CompareKind::TimeTz => datetime_read(timetz_key(text), DateTimeInput::TimeTz, text)?,
        CompareKind::Network { cidr } => network_key(text, *cidr)?,
        CompareKind::MacAddr { octets } => {
            macaddr_key(text, *octets).ok_or_else(|| match octets {
                8 if decode::macaddr8_in(text).is_none() => Unread::Refused,
                8 => unparsed,
                _ => decode::macaddr_unread(text),
            })?
        }
        CompareKind::Jsonb => jsonb_key(text)?,
        // Every refusal of the reader is `uuid_in`'s (I64).
        CompareKind::Uuid => {
            OrderKey::Bytes(decode::decode_uuid(text).ok_or(Unread::Refused)?.to_vec())
        }
        CompareKind::Bytea => {
            OrderKey::Bytes(decode::decode_bytea(text).ok_or_else(|| decode::bytea_unread(text))?)
        }
        // A field past its typmod's length is refused, and one within it is
        // keyed as written. A literal is coerced with no typmod
        // ([`literal_key`]).
        CompareKind::Text { length } => {
            if length.is_some_and(|length| decode::char_typmod_refuses(text, length)) {
                return Err(Unread::Refused);
            }
            OrderKey::Text(text.to_string())
        }
        // `bcTruelen` on both sides, which is what makes this the server's
        // comparison rather than one over the padding (I38). The blank is
        // ASCII `0x20` and nothing else; a tab is a value byte.
        CompareKind::PaddedText { length } => {
            if length.is_some_and(|length| decode::char_typmod_refuses(text, length)) {
                return Err(Unread::Refused);
            }
            OrderKey::Text(text.trim_end_matches(' ').to_string())
        }
    })
}

/// A date, time or timestamp reader's answer, a text it reads no value from
/// refused where no major's grammar reads it under any setting
/// ([`datetime_in::datetime_reads`]). The readers read `*_out`'s form and
/// little more, splitting a text otherwise than `ParseDateTime` does where it
/// is not that form, so a refusal of theirs is put to the grammar too.
fn datetime_read<T>(read: Read<T>, input: DateTimeInput, text: &str) -> Read<T> {
    read.map_err(|_| {
        if datetime_in::datetime_reads(input, text) { Unread::Unparsed } else { Unread::Refused }
    })
}

/// [`datetime_read`] for an `interval` of a column qualified by `qualifier`
/// ([`datetime_in::interval_reads`]).
fn interval_read<T>(read: Read<T>, text: &str, qualifier: Option<IntervalQualifier>) -> Read<T> {
    read.map_err(|_| {
        if datetime_in::interval_reads(text, qualifier) {
            Unread::Unparsed
        } else {
            Unread::Refused
        }
    })
}

/// A filter literal's key, which is [`order_key`]'s — a field's — but for an
/// integer in DataFusion's semantics, read as an `i64` whatever the column's
/// width, a float in DataFusion's semantics, read as the parse rounds it, an
/// infinity past the type's range, and a `numeric` in either: read within `numeric_in`'s bounds in
/// PostgreSQL's, and exactly, neither rounded to the scale nor refused past
/// the precision, the server coercing a literal with no typmod where it puts a
/// field through the typmod (I63) — and a text of any length and an `interval`
/// under no field qualifier for the same reason. In DataFusion's semantics a literal is
/// DataFusion's value, not the server's (`roadmap.md`, "A literal is
/// guaranteed in `*_out`'s form and never read past `*_in`'s").
fn literal_key(kind: &CompareKind, text: &str, semantics: ComparisonSemantics) -> Option<OrderKey> {
    match (semantics, kind) {
        (ComparisonSemantics::DataFusion, CompareKind::Int { .. }) => {
            Some(OrderKey::Int(text.parse::<i64>().ok()?))
        }
        (ComparisonSemantics::DataFusion, CompareKind::Float32) => {
            Some(OrderKey::Float(f64::from(text.parse::<f32>().ok()?)))
        }
        (ComparisonSemantics::DataFusion, CompareKind::Float64) => {
            Some(OrderKey::Float(text.parse::<f64>().ok()?))
        }
        (
            ComparisonSemantics::Postgres,
            CompareKind::Decimal { .. } | CompareKind::Numeric { .. },
        ) if !decode::numeric_in_stores(text) => None,
        (_, CompareKind::Decimal { scale, .. }) => special_order_key(kind, text).or_else(|| {
            let unscaled = decode::decimal_unscaled_digits(text, *scale)?;
            Some(OrderKey::Decimal(i256::from_string(&unscaled)?))
        }),
        (_, CompareKind::Numeric { typmod: Some(_), .. }) => special_order_key(kind, text)
            .or_else(|| Some(OrderKey::Numeric(NumericKey::parse(text)?))),
        (_, CompareKind::Text { .. }) => order_key(&CompareKind::Text { length: None }, text),
        (_, CompareKind::PaddedText { .. }) => {
            order_key(&CompareKind::PaddedText { length: None }, text)
        }
        (_, CompareKind::Interval { .. }) => {
            order_key(&CompareKind::Interval { qualifier: None }, text)
        }
        (_, CompareKind::IntervalFields { .. }) => {
            order_key(&CompareKind::IntervalFields { qualifier: None }, text)
        }
        _ => order_key(kind, text),
    }
}

/// A `smallint`, `integer` or `bigint`, of `bytes` bytes, field or literal,
/// read as `int2in`, `int4in` and `int8in` read it. Rust's parse is the
/// grammar: the digits `*_out` writes, and of `*_in`'s a leading `+` and
/// leading zeros. A blank around the number, an underscore between digits and
/// a `0x`, `0o` or `0b` prefix, which the server reads, are unparsed
/// (`docs/design/decisions.md`, "D55"), and every other text it does not read
/// is refused ([`decode::int_unread`]).
///
/// **A value past the column's width is refused, as the server refuses it**
/// (I60): `70000` is no value of a `smallint` column, where a literal in
/// DataFusion's semantics is read as an `i64` ([`literal_key`]).
// pg-refuses: I60 — out of range for the column's width.
fn int_in(text: &str, bytes: u8) -> Read<i64> {
    let value = text.parse::<i64>().map_err(|e| match e.kind() {
        std::num::IntErrorKind::PosOverflow | std::num::IntErrorKind::NegOverflow => {
            Unread::Refused
        }
        _ => decode::int_unread(text, int_range(bytes)),
    })?;
    if int_range(bytes).contains(&value) { Ok(value) } else { Err(Unread::Refused) }
}

/// The values a signed integer of `bytes` bytes holds.
fn int_range(bytes: u8) -> std::ops::RangeInclusive<i64> {
    match bytes {
        2 => i16::MIN.into()..=i16::MAX.into(),
        4 => i32::MIN.into()..=i32::MAX.into(),
        _ => i64::MIN..=i64::MAX,
    }
}

/// PostgreSQL's float order, not Rust's: `NaN` is greater than every other
/// value, infinities included, and `NaN = NaN` is true (I33), where Rust's
/// `partial_cmp` answers `None`. `real`/`double precision` are the only
/// columns whose key is a float `NaN`: a `NaN` in a `numeric(p,s)` column is
/// keyed as a special value ([`special_order_key`]) before any float is read,
/// having no `Decimal128` representation (I4).
fn pg_float_cmp(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).expect("neither side is NaN"),
    }
}

/// Rank first, value second. A special value on either side is decided by
/// [`OrderKey::rank`] alone, so two of the same special are equal —
/// `-infinity = -infinity`, `infinity = infinity`, `NaN = NaN` (I34) — and a
/// special against a finite never has to name a number.
///
/// Over the keys of one kind this is a **total order** — reflexive,
/// antisymmetric and transitive — which a running minimum, maximum or
/// sortedness flag depends on and a single comparison does not; the oracle
/// test `the_key_is_a_total_order_over_every_committed_value` pins it.
fn compare_keys(a: &OrderKey, b: &OrderKey) -> Ordering {
    let (rank_a, rank_b) = (a.rank(), b.rank());
    if rank_a != rank_b {
        return rank_a.cmp(&rank_b);
    }
    match (a, b) {
        (OrderKey::Bool(x), OrderKey::Bool(y)) => x.cmp(y),
        (OrderKey::Int(x), OrderKey::Int(y)) => x.cmp(y),
        (OrderKey::Float(x), OrderKey::Float(y)) => pg_float_cmp(*x, *y),
        (OrderKey::Decimal(x), OrderKey::Decimal(y)) => x.cmp(y),
        (OrderKey::Numeric(x), OrderKey::Numeric(y)) => x.cmp(y),
        (OrderKey::Interval(x), OrderKey::Interval(y)) => x.cmp(y),
        (OrderKey::IntervalFields(m, d, t), OrderKey::IntervalFields(n, e, u)) => {
            (m, d, t).cmp(&(n, e, u))
        }
        (
            OrderKey::TimeTz { utc: utc_x, zone: zone_x },
            OrderKey::TimeTz { utc: utc_y, zone: zone_y },
        ) => utc_x.cmp(utc_y).then_with(|| zone_x.cmp(zone_y)),
        (OrderKey::Network(x), OrderKey::Network(y)) => x.cmp(y),
        (OrderKey::Jsonb(x), OrderKey::Jsonb(y)) => x.cmp(y),
        (OrderKey::Bytes(x), OrderKey::Bytes(y)) => x.cmp(y),
        (OrderKey::Text(x), OrderKey::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ if rank_a != FINITE => Ordering::Equal,
        _ => unreachable!("both sides of a comparison decode through one column's `CompareKind`"),
    }
}

/// A value's place in its column's order, opaque outside this module: the key
/// a filter orders a row by, for a caller ordering values that meet no filter
/// — the statistics gatherer's running bounds and sortedness.
#[derive(Debug, Clone)]
pub(crate) struct ValueKey(OrderKey);

impl ValueKey {
    /// The key of one unescaped value, `None` when the text is not a value of
    /// `kind` ([`order_key`]).
    pub(crate) fn of(kind: &CompareKind, text: &str) -> Option<Self> {
        order_key(kind, text).map(Self)
    }

    /// [`Self::of`] a field, saying why it has no key: the statistics
    /// gatherer fails the parse on one the type's `*_in` refuses.
    pub(crate) fn of_field(kind: &CompareKind, text: &str) -> Read<Self> {
        field_key(kind, text).map(Self)
    }

    /// [`compare_keys`]: a total order over the keys of one kind, the one a
    /// filter compares by — what a test holds [`Self::stored_order`] against.
    #[cfg(test)]
    pub(crate) fn compare(&self, other: &Self) -> Ordering {
        compare_keys(&self.0, &other.0)
    }

    /// **The order a stored bound and a block's row order are kept in**:
    /// [`compare_keys`], the order a filter compares by, with a float's `-0`
    /// below `0` where it ties them, as IEEE `totalOrder` — the order Arrow
    /// sorts a float by and DataFusion takes its `MIN`/`MAX` in — places
    /// them. A refinement, so an extreme in it is one in the filter's order
    /// and a block sorted in it is sorted in that order too
    /// (`docs/design/decisions.md`, "D79").
    pub(crate) fn stored_order(&self, other: &Self) -> Ordering {
        compare_keys(&self.0, &other.0).then_with(|| match (&self.0, &other.0) {
            (OrderKey::Float(x), OrderKey::Float(y)) => x.total_cmp(y),
            _ => Ordering::Equal,
        })
    }

    /// The heap this key holds, as the sizes the allocator was asked for:
    /// what gathering charges a keyed bound or row-order value
    /// (`crate::statistics::StatisticsAccount`).
    pub(crate) fn heap_bytes(&self) -> u64 {
        match &self.0 {
            OrderKey::Numeric(numeric) => numeric.heap_bytes(),
            OrderKey::Jsonb(jsonb) => jsonb.heap_bytes(),
            OrderKey::Bytes(bytes) => bytes.capacity() as u64,
            OrderKey::Text(text) => text.capacity() as u64,
            OrderKey::NegativeInfinity
            | OrderKey::Bool(_)
            | OrderKey::Int(_)
            | OrderKey::Float(_)
            | OrderKey::Decimal(_)
            | OrderKey::Interval(_)
            | OrderKey::IntervalFields(..)
            | OrderKey::TimeTz { .. }
            | OrderKey::Network(_)
            | OrderKey::PositiveInfinity
            | OrderKey::NotANumber => 0,
        }
    }
}

/// One side of a **nested** comparison, decoded from a container literal per
/// the column's [`NestedCompare`]. Both sides of any one comparison come from
/// the same plan, so a variant mismatch is unreachable by construction.
///
/// `None` in an element or field position is SQL NULL, a value of the
/// container rather than the absence of one: `{1,NULL}` is a two-element
/// array. The whole field being NULL is a different fact — the `\N` the row
/// carries — and is [`Truth::Unknown`] as it is for a scalar column.
#[derive(Debug, Clone, PartialEq)]
enum NestedKey {
    /// A scalar position, through the leaf type's own [`OrderKey`].
    Leaf(OrderKey),
    /// An `array_out` value: its elements flattened row-major, plus the shape
    /// they were written in, because `array_cmp` decides on the shape once
    /// the elements agree (I45).
    Array { elements: Vec<Option<NestedKey>>, dims: Vec<usize>, lower_bounds: Vec<i32> },
    /// A `record_out` value: one entry per declared field, in declaration
    /// order.
    Record(Vec<Option<NestedKey>>),
    /// A range value, already through [`make_range`] — the form the server
    /// would have stored, not the form it was written in. Boxed because a
    /// bound is itself a `NestedKey`; the vector the multirange holds is
    /// indirection enough on its own.
    Range(Box<RangeKey>),
    /// A multirange value, already sorted, coalesced and emptied out
    /// ([`canonical_multirange`]), so its members are never empty and never
    /// touch.
    Multirange(Vec<RangeKey>),
}

/// A range value in the form PostgreSQL itself stores, which is the only form
/// two ranges may be compared in: `range_in` runs every literal through
/// `make_range`, so `int4range '[1,10]'` and `int4range '(0,11)'` are one
/// value the file can only hold as `[1,11)` (I46).
#[derive(Debug, Clone, PartialEq)]
struct RangeKey {
    /// The empty range, which `range_cmp` sorts below every other value and
    /// which is *not* the same as a range with two absent bounds — `empty`
    /// and `(,)` are different values (I46).
    empty: bool,
    lower: RangeBoundKey,
    upper: RangeBoundKey,
}

/// One bound of a range: PostgreSQL's `RangeBound`, minus the type it is a
/// bound of.
#[derive(Debug, Clone, PartialEq)]
struct RangeBoundKey {
    /// `None` is an **unbounded** bound — the server's `infinite` flag, and
    /// not a value at all. A bound holding `infinity` is a different thing and
    /// lives in the [`OrderKey`] beneath: `daterange '[2020-01-01,infinity]'`
    /// has a finite upper bound, which is why `daterange_canonical` leaves it
    /// alone (I34, I46).
    value: Option<NestedKey>,
    inclusive: bool,
    /// Which end this bound is. It decides the answer whenever two bounds
    /// hold the same value, so it travels with the bound rather than being
    /// inferred from the caller — `bounds_adjacent` relabels a pair before
    /// comparing it, as the server does.
    lower: bool,
}

impl RangeKey {
    /// The empty range. Its bounds are never read, and are the shape
    /// `range_serialize` leaves behind: absent and exclusive.
    fn empty() -> Self {
        RangeKey {
            empty: true,
            lower: RangeBoundKey { value: None, inclusive: false, lower: true },
            upper: RangeBoundKey { value: None, inclusive: false, lower: false },
        }
    }
}

/// Read one side of a nested comparison out of `text`.
///
/// `side` is which grammar to read it in, and it is the whole of what
/// separates the two sides: a *field* is read with [`crate::nested`]'s strict
/// `decode_*`, a *literal* with the `array_in`/`record_in` supersets
/// (`parse_*`), which take `{a, b}` and `{ 1 , 2 }` (I44).
///
/// The leaf grammar does not widen with it (`docs/design/decisions.md`,
/// "D58"): a leaf is read by its type's own reader, which takes no space its
/// `*_out` form does not write, so `--filter 'p=( 1 , a )'` is refused.
///
/// `None` is "not a value of this type", which is
/// `Error::PredicateValueDecode` for a literal and `Error::FieldDecode` for a
/// field.
fn nested_key(plan: &NestedCompare, text: &str, side: Side) -> Option<NestedKey> {
    let input = side == Side::Literal;
    Some(match plan {
        // A nested comparison is PostgreSQL's alone, DataFusion's semantics
        // refusing one.
        NestedCompare::Leaf { kind, .. } => NestedKey::Leaf(match side {
            Side::Literal => literal_key(kind, text, ComparisonSemantics::Postgres)?,
            Side::Field(invalid) => read_key(kind, text, invalid)?,
        }),
        // Refused before any value is read: `resolve_term` never builds a
        // comparison over a tree holding one.
        NestedCompare::Uncomparable { .. } => return None,
        NestedCompare::Array(element) => {
            let literal =
                if input { nested::parse_array(text) } else { nested::decode_array(text) }?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            for value in &literal.elements {
                elements.push(match value {
                    Some(value) => Some(nested_key(&element.plan, value, side)?),
                    None if position_refuses_null(element, side) => return None,
                    None => None,
                });
            }
            NestedKey::Array { elements, dims: literal.dims, lower_bounds: literal.lower_bounds }
        }
        // `int2vector`'s own grammar on both sides; the elements are built
        // here rather than through `order_key`, `int2vectorout` writing an
        // `int16` and nothing else.
        //
        // `dims` and `lower_bounds` are `[n]` and `[0]` for every value, the
        // empty vector included: `int2vectorin` sets `ndim = 1` and
        // `lbound1 = 0` unconditionally, where `array_out`'s `{}` is
        // zero-dimensional, so an empty `int2vector` is not the empty array
        // (I47).
        NestedCompare::Int2Vector => {
            let values = if input {
                nested::parse_int2vector(text)
            } else {
                nested::decode_int2vector(text)
            }?;
            NestedKey::Array {
                elements: values
                    .iter()
                    .map(|v| Some(NestedKey::Leaf(OrderKey::Int(i64::from(*v)))))
                    .collect(),
                dims: vec![values.len()],
                lower_bounds: vec![0],
            }
        }
        NestedCompare::Record(plans) => {
            let mut fields = if input {
                nested::parse_record(text, plans.len())?.fields
            } else {
                let mut fields = nested::decode_record(text)?.fields;
                // A zero-field composite is written `()`, and so is a
                // one-field composite holding NULL — the literal cannot tell
                // them apart, so the declared field list decides (I23).
                // `parse_record` has the arity in hand; `decode_record` does
                // not, so it is asked here.
                if plans.is_empty() && fields == [None] {
                    fields.clear();
                }
                fields
            };
            if fields.len() != plans.len() {
                return None;
            }
            let mut out = Vec::with_capacity(plans.len());
            for ((_, field), value) in plans.iter().zip(fields.drain(..)) {
                out.push(match value {
                    Some(value) => Some(nested_key(&field.plan, &value, side)?),
                    None if position_refuses_null(field, side) => return None,
                    None => None,
                });
            }
            NestedKey::Record(out)
        }
        NestedCompare::Range { bound, discrete } => {
            let literal =
                if input { nested::parse_range(text) } else { nested::decode_range(text) }?;
            NestedKey::Range(Box::new(range_key(bound, &literal, *discrete, side)?))
        }
        NestedCompare::Multirange { bound, discrete } => {
            let literals = if input {
                nested::parse_multirange(text)
            } else {
                nested::decode_multirange(text)
            }?;
            let mut members = Vec::with_capacity(literals.len());
            for literal in &literals {
                members.push(range_key(bound, literal, *discrete, side)?);
            }
            NestedKey::Multirange(canonical_multirange(members, *discrete)?)
        }
    })
}

/// Whether [`nested_key`] refuses a NULL at `position` read on `side`: a
/// literal's always and a field's but under
/// [`PostgresInvalidValues::Ignore`], where the position's type is a `NOT
/// NULL` domain, as `array_in` and `record_in` refuse it (I76).
fn position_refuses_null(position: &Position<NestedCompare>, side: Side) -> bool {
    position.not_null && side != Side::Field(PostgresInvalidValues::Ignore)
}

/// One range value, read through its bound's plan and then put into the form
/// the server stores it in.
///
/// Both sides go through [`make_range`], not only the literal
/// (`docs/design/decisions.md`, "D58"): it is idempotent on a `range_out`
/// field, the server having already applied it, so one code path serves both
/// grammars.
fn range_key(
    bound: &NestedCompare,
    literal: &nested::RangeLiteral,
    discrete: Option<Discrete>,
    reading: Side,
) -> Option<RangeKey> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        Some(RangeBoundKey {
            value: match text {
                Some(text) => Some(nested_key(bound, text, reading)?),
                None => None,
            },
            inclusive,
            lower,
        })
    };
    make_range(
        side(&literal.lower, literal.lower_inclusive, true)?,
        side(&literal.upper, literal.upper_inclusive, false)?,
        literal.empty,
        discrete,
    )
}

/// **Whether a marked check finds PostgreSQL's input function refuses `text`
/// as a field of a column compared by `plan`**: what a strict parse checks
/// every field by ([`PostgresInvalidValues::Strict`]). A scalar is [`field_key`]'s refusal;
/// a nested value is read as the server reads one on its way in, through each
/// container's input grammar, every element as a field of its own type and a
/// range's bounds in order ([`checked_key`]). `json`, held as its text, is
/// read by `json_in`'s grammar ([`decode::json_in`]), and a position holding
/// a type ordered by nothing by its [`TextGrammar`] where it has one; any
/// other plan naming no type this build reads by — another held as its text,
/// or a range declaring its own canonical function — refuses nothing, no
/// reader here decoding its fields. A column whose own type is ordered by
/// nothing is [`ComparisonPlan::Refused`] and is read by [`grammar_refuses`]
/// instead.
pub(crate) fn field_refused(plan: &ComparisonPlan, text: &str) -> bool {
    match plan {
        // `AS_TEXT`'s one member is `json`.
        ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. } => {
            !decode::json_in(text)
        }
        // Every text within its length is a value of these, and keying one
        // copies it.
        ComparisonPlan::Compared {
            kind: CompareKind::Text { length } | CompareKind::PaddedText { length },
            ..
        } => length.is_some_and(|length| decode::char_typmod_refuses(text, length)),
        ComparisonPlan::Compared { kind, .. } => {
            matches!(field_key(kind, text), Err(Unread::Refused))
        }
        ComparisonPlan::Nested(nested) => checked_key(nested, text).is_err(),
        ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_) => false,
    }
}

/// Whether PostgreSQL's input function refuses `text` as a field read by
/// `grammar`, of a type this build holds as its text and orders by nothing.
pub(crate) fn grammar_refuses(grammar: TextGrammar, text: &str) -> bool {
    let reads = match grammar {
        TextGrammar::Bit { varying, length } => decode::bit_in(text, varying, length),
        TextGrammar::Geometric(Geometric::Point) => decode::point_in(text),
        TextGrammar::Geometric(Geometric::Line) => decode::line_in(text),
        TextGrammar::Geometric(Geometric::Lseg) => decode::lseg_in(text),
        TextGrammar::Geometric(Geometric::Box) => decode::box_in(text),
        TextGrammar::Geometric(Geometric::Path) => decode::path_in(text),
        TextGrammar::Geometric(Geometric::Polygon) => decode::poly_in(text),
        TextGrammar::Geometric(Geometric::Circle) => decode::circle_in(text),
        TextGrammar::BoxArray { not_null } => {
            let element = NestedCompare::Uncomparable {
                declared: "box".into(),
                divergence: None,
                grammar: Some(TextGrammar::Geometric(Geometric::Box)),
                delimiter: Some(b';'),
            };
            let element = Position { plan: element, not_null };
            checked_key(&NestedCompare::Array(Box::new(element)), text).is_ok()
        }
        TextGrammar::RefusesAll => false,
        TextGrammar::RefusesNothing => true,
    };
    !reads
}

/// The `typdelim` an array of `element` is split at, which the plan carries
/// off the preamble (I22) — `None` where the split is not this build's to
/// make: a delimiter the preamble could not read, and one `array_in` reads
/// otherwise than [`nested::parse_array_delimited`] models at some supported
/// major, which is any byte but a visible ASCII one, the space not among them,
/// other than a brace, a double quote or a backslash.
fn array_delimiter(element: &NestedCompare) -> Option<u8> {
    let delimiter = match element {
        NestedCompare::Uncomparable { delimiter, .. } => (*delimiter)?,
        _ => b',',
    };
    (delimiter.is_ascii_graphic() && !b"{}\"\\".contains(&delimiter)).then_some(delimiter)
}

/// A nested field's key, built as [`nested_key`] builds a literal's — each
/// container read by its input grammar, the one `COPY` reads a field by — with
/// every element read as a field: `Err` where a marked check on the way
/// refuses it, and `Ok(None)` where it is read but some part of it has no key
/// here, a position this build does not order or a spelling it does not read.
/// **Every element is read whatever an earlier one answered**, so a refusal
/// anywhere in the value is found.
fn checked_key(plan: &NestedCompare, text: &str) -> Read<Option<NestedKey>> {
    let refused = Unread::Refused;
    Ok(match plan {
        NestedCompare::Leaf { kind, .. } => match field_key(kind, text) {
            Ok(key) => Some(NestedKey::Leaf(key)),
            Err(Unread::Refused) => return Err(refused),
            Err(Unread::Unparsed) => None,
        },
        // `json`, the one position diverging `AsText` (`NestedCompare::Uncomparable`).
        NestedCompare::Uncomparable { divergence: Some(ComparisonDivergence::AsText), .. }
            if !decode::json_in(text) =>
        {
            return Err(refused);
        }
        NestedCompare::Uncomparable { grammar: Some(grammar), .. }
            if grammar_refuses(*grammar, text) =>
        {
            return Err(refused);
        }
        NestedCompare::Uncomparable { .. } => None,
        NestedCompare::Array(element) => {
            let Some(delimiter) = array_delimiter(&element.plan) else { return Ok(None) };
            let literal = nested::parse_array_delimited(text, delimiter).ok_or(refused)?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            let mut keyed = true;
            for value in &literal.elements {
                match value {
                    Some(value) => match checked_key(&element.plan, value)? {
                        Some(key) => elements.push(Some(key)),
                        None => keyed = false,
                    },
                    None if element.not_null => return Err(refused),
                    None => elements.push(None),
                }
            }
            keyed.then_some(NestedKey::Array {
                elements,
                dims: literal.dims,
                lower_bounds: literal.lower_bounds,
            })
        }
        NestedCompare::Int2Vector => {
            let values = nested::parse_int2vector(text).ok_or(refused)?;
            Some(NestedKey::Array {
                dims: vec![values.len()],
                elements: values
                    .into_iter()
                    .map(|v| Some(NestedKey::Leaf(OrderKey::Int(i64::from(v)))))
                    .collect(),
                lower_bounds: vec![0],
            })
        }
        NestedCompare::Record(plans) => {
            let fields = nested::parse_record(text, plans.len()).ok_or(refused)?.fields;
            let mut out = Vec::with_capacity(plans.len());
            let mut keyed = true;
            for ((_, field), value) in plans.iter().zip(fields) {
                match value {
                    Some(value) => match checked_key(&field.plan, &value)? {
                        Some(key) => out.push(Some(key)),
                        None => keyed = false,
                    },
                    None if field.not_null => return Err(refused),
                    None => out.push(None),
                }
            }
            keyed.then_some(NestedKey::Record(out))
        }
        NestedCompare::Range { bound, discrete } => {
            let literal = nested::parse_range(text).ok_or(refused)?;
            checked_range(bound, &literal, *discrete)?
                .map(|range| NestedKey::Range(Box::new(range)))
        }
        NestedCompare::Multirange { bound, discrete } => {
            let literals = nested::parse_multirange(text).ok_or(refused)?;
            let mut members = Vec::with_capacity(literals.len());
            let mut keyed = true;
            for literal in &literals {
                match checked_range(bound, literal, *discrete)? {
                    Some(member) => members.push(member),
                    None => keyed = false,
                }
            }
            // `multirange_in` refuses nothing past its members' `range_in`.
            keyed
                .then(|| canonical_multirange(members, *discrete))
                .flatten()
                .map(NestedKey::Multirange)
        }
    })
}

/// [`range_key`] for [`checked_key`]: both bounds read, then [`make_range`]'s
/// refusals where both have a key.
fn checked_range(
    bound: &NestedCompare,
    literal: &nested::RangeLiteral,
    discrete: Option<Discrete>,
) -> Read<Option<RangeKey>> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        let value = match text {
            Some(text) => match checked_key(bound, text)? {
                Some(key) => Some(key),
                None => return Ok(None),
            },
            None => None,
        };
        Ok(Some(RangeBoundKey { value, inclusive, lower }))
    };
    let lower = side(&literal.lower, literal.lower_inclusive, true)?;
    let upper = side(&literal.upper, literal.upper_inclusive, false)?;
    let (Some(lower), Some(upper)) = (lower, upper) else { return Ok(None) };
    make_range(lower, upper, literal.empty, discrete).map(Some).ok_or(Unread::Refused)
}

/// `make_range`: `range_serialize`'s type-independent checks, then the range
/// type's canonical function where it has one, then those checks again — the
/// order the server applies them in, and the reason `int4range '(1,2)'` is
/// `empty` (I46).
///
/// `None` is the server's `22000` — a lower bound above its upper, a *semantic*
/// refusal the container grammar cannot see (I44), so it is raised here where
/// the bounds have been decoded — or a discrete bound whose successor leaves
/// the subtype, past [`Discrete::largest`], the canonical function's
/// "out of range".
fn make_range(
    lower: RangeBoundKey,
    upper: RangeBoundKey,
    empty: bool,
    discrete: Option<Discrete>,
) -> Option<RangeKey> {
    let serialized = serialize_range(lower, upper, empty)?;
    let Some(discrete) = discrete.filter(|_| !serialized.empty) else {
        return Some(serialized);
    };
    // `int4range_canonical` and its two siblings, which differ only in the
    // value past which they refuse a successor: an exclusive lower bound
    // becomes inclusive at the successor, an inclusive upper becomes
    // exclusive at the successor.
    //
    // The `Int` pattern is `daterange_canonical`'s `DATE_NOT_FINITE` guard: a
    // date `infinity` decodes to `OrderKey::PositiveInfinity` rather than to
    // a day count, so it matches no arm here and is left as written, which is
    // what makes `[2020-01-01,infinity]` keep its inclusive upper (I34, I46).
    // pg-refuses: I46 — a successor past the subtype's largest value.
    let successor = |bound: &RangeBoundKey| match &bound.value {
        Some(NestedKey::Leaf(OrderKey::Int(n))) => (*n < discrete.largest()).then(|| {
            Some(RangeBoundKey {
                value: Some(NestedKey::Leaf(OrderKey::Int(n + 1))),
                inclusive: !bound.inclusive,
                lower: bound.lower,
            })
        }),
        // Not a finite integer bound, so the canonical function skips it.
        _ => Some(None),
    };
    let mut lower = serialized.lower;
    let mut upper = serialized.upper;
    if !lower.inclusive
        && let Some(shifted) = successor(&lower)?
    {
        lower = shifted;
    }
    if upper.inclusive
        && let Some(shifted) = successor(&upper)?
    {
        upper = shifted;
    }
    serialize_range(lower, upper, false)
}

/// `range_serialize`'s type-independent half: the out-of-order refusal, the
/// collapse to `empty`, and "an infinite boundary is never inclusive".
fn serialize_range(
    mut lower: RangeBoundKey,
    mut upper: RangeBoundKey,
    empty: bool,
) -> Option<RangeKey> {
    if empty {
        return Some(RangeKey::empty());
    }
    match compare_bound_values(&lower, &upper) {
        // pg-refuses: I46 — `22000`, a lower bound above its upper.
        Ordering::Greater => return None,
        // Equal bounds are a value only when both ends include it: `[1,1]` is
        // one point and `[1,1)`, `(1,1]` and `(1,1)` are all `empty`. This
        // runs before canonicalization *and* after it, which is what makes
        // `int4range '(1,2)'` empty.
        Ordering::Equal if !(lower.inclusive && upper.inclusive) => {
            return Some(RangeKey::empty());
        }
        _ => {}
    }
    if lower.value.is_none() {
        lower.inclusive = false;
    }
    if upper.value.is_none() {
        upper.inclusive = false;
    }
    Some(RangeKey { empty: false, lower, upper })
}

/// `range_cmp_bound_values`: the bounds' held values alone, with an absent
/// bound settled by which end it is. Inclusivity is not consulted, which is
/// what separates this from [`compare_bounds`] — the emptiness and adjacency
/// tests both need the values without it.
fn compare_bound_values(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    match (&a.value, &b.value) {
        (None, None) if a.lower == b.lower => Ordering::Equal,
        (None, None) | (None, Some(_)) => {
            if a.lower {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (Some(_), None) => {
            if b.lower {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `range_cmp_bounds`: infinity, then the held value, then inclusivity — and
/// inclusivity is where which *end* a bound is starts to matter. An exclusive
/// **lower** bound is above an inclusive one at the same value ("just
/// after"), an exclusive **upper** below it ("just before") (I46).
fn compare_bounds(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    let by_value = compare_bound_values(a, b);
    if a.value.is_none() || b.value.is_none() || by_value.is_ne() {
        return by_value;
    }
    match (a.inclusive, b.inclusive) {
        (true, true) => Ordering::Equal,
        (false, false) if a.lower == b.lower => Ordering::Equal,
        (false, _) if a.lower => Ordering::Greater,
        (false, _) => Ordering::Less,
        (true, false) if b.lower => Ordering::Less,
        (true, false) => Ordering::Greater,
    }
}

/// `range_cmp`: `empty` below everything, then lower bound, then upper.
fn compare_range(a: &RangeKey, b: &RangeKey) -> Ordering {
    match (a.empty, b.empty) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => {
            compare_bounds(&a.lower, &b.lower).then_with(|| compare_bounds(&a.upper, &b.upper))
        }
    }
}

/// `multirange_canonicalize`: sort the members, drop the empty ones, and
/// merge any two that overlap or touch. After it no member is empty and no
/// two members meet, so the sequence is the value and the comparison is a
/// plain sequence walk (I46).
///
/// `None` propagates a bound the union could not re-serialize, which the
/// shapes reaching here cannot produce; it is carried rather than unwrapped.
fn canonical_multirange(
    mut members: Vec<RangeKey>,
    discrete: Option<Discrete>,
) -> Option<Vec<RangeKey>> {
    members.sort_by(compare_range);
    let mut out: Vec<RangeKey> = Vec::with_capacity(members.len());
    for current in members {
        if current.empty {
            continue;
        }
        let Some(last) = out.last() else {
            out.push(current);
            continue;
        };
        // The server's own order. The first test needs the sort:
        // `range_adjacent_internal` answers true for "either meets the
        // other", and only sorting rules out the second direction.
        if ranges_adjacent(last, &current, discrete) {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        } else if range_before(last, &current) {
            out.push(current);
        } else {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        }
    }
    Some(out)
}

/// `range_before_internal`: every point of `a` is below every point of `b`,
/// with an empty range neither before nor after anything.
fn range_before(a: &RangeKey, b: &RangeKey) -> bool {
    !a.empty && !b.empty && compare_bounds(&a.upper, &b.lower).is_lt()
}

/// `range_adjacent_internal`: the two ranges touch without overlapping,
/// in either direction.
fn ranges_adjacent(a: &RangeKey, b: &RangeKey, discrete: Option<Discrete>) -> bool {
    !a.empty
        && !b.empty
        && (bounds_adjacent(&a.upper, &b.lower, discrete)
            || bounds_adjacent(&b.upper, &a.lower, discrete))
}

/// `bounds_adjacent`: whether an upper bound and a lower bound meet with no
/// point between them.
///
/// Equal values are adjacent exactly when one end includes the point and the
/// other does not. Values that differ are adjacent only in a discrete range,
/// which the server decides by building the range *between* them with both
/// inclusivities flipped and asking whether it came out empty.
fn bounds_adjacent(
    upper: &RangeBoundKey,
    lower: &RangeBoundKey,
    discrete: Option<Discrete>,
) -> bool {
    match compare_bound_values(upper, lower) {
        Ordering::Equal => upper.inclusive != lower.inclusive,
        Ordering::Greater => false,
        Ordering::Less => {
            discrete.is_some()
                && make_range(
                    RangeBoundKey {
                        value: upper.value.clone(),
                        inclusive: !upper.inclusive,
                        lower: true,
                    },
                    RangeBoundKey {
                        value: lower.value.clone(),
                        inclusive: !lower.inclusive,
                        lower: false,
                    },
                    false,
                    discrete,
                )
                .is_some_and(|between| between.empty)
        }
    }
}

/// `range_union_internal` for two ranges already known to overlap or touch:
/// the lower of the two lower bounds, the upper of the two uppers, back
/// through [`make_range`].
fn range_union(a: &RangeKey, b: &RangeKey, discrete: Option<Discrete>) -> Option<RangeKey> {
    let lower =
        if compare_bounds(&a.lower, &b.lower).is_lt() { a.lower.clone() } else { b.lower.clone() };
    let upper =
        if compare_bounds(&a.upper, &b.upper).is_gt() { a.upper.clone() } else { b.upper.clone() };
    make_range(lower, upper, false, discrete)
}

/// One NULL rule, at every level: two NULLs are equal, and NULL sorts above
/// not-NULL (I45). It covers equality and ordering alike, which is what makes
/// a nested comparison two-valued throughout, never [`Truth::Unknown`].
fn compare_slot(a: Option<&NestedKey>, b: Option<&NestedKey>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `array_cmp`/`record_cmp`, structurally.
///
/// An array compares its elements first and its shape only afterwards (I45):
/// up to the shorter array's length, then element count, then dimension
/// count, then the dimensions, then the lower bounds — not the order
/// `array_eq` uses. So `{1,2}` is above `[0:1]={1,2}` and below
/// `{{1,2},{3,4}}`, whose extra elements are never reached.
///
/// A record's two sides always have the same arity: [`nested_key`] checks it
/// against the composite's own declared field list before building either.
fn compare_nested(a: &NestedKey, b: &NestedKey) -> Ordering {
    match (a, b) {
        (NestedKey::Leaf(a), NestedKey::Leaf(b)) => compare_keys(a, b),
        (
            NestedKey::Array { elements: ae, dims: ad, lower_bounds: al },
            NestedKey::Array { elements: be, dims: bd, lower_bounds: bl },
        ) => first_difference(
            ae.iter()
                .zip(be.iter())
                .map(|(a, b)| compare_slot(a.as_ref(), b.as_ref()))
                .chain(std::iter::once(ae.len().cmp(&be.len())))
                .chain(std::iter::once(ad.len().cmp(&bd.len())))
                .chain(ad.iter().zip(bd.iter()).map(|(a, b)| a.cmp(b)))
                .chain(al.iter().zip(bl.iter()).map(|(a, b)| a.cmp(b))),
        ),
        (NestedKey::Record(a), NestedKey::Record(b)) => first_difference(
            a.iter().zip(b.iter()).map(|(a, b)| compare_slot(a.as_ref(), b.as_ref())),
        ),
        (NestedKey::Range(a), NestedKey::Range(b)) => compare_range(a, b),
        // `multirange_cmp`: member-wise, and the shorter one first where the
        // members it has all agree — the server treating a missing member as
        // an empty range and `empty` as the lowest value there is.
        (NestedKey::Multirange(a), NestedKey::Multirange(b)) => first_difference(
            a.iter()
                .zip(b.iter())
                .map(|(a, b)| compare_range(a, b))
                .chain(std::iter::once(a.len().cmp(&b.len()))),
        ),
        _ => unreachable!("both sides of a comparison are read through one column's plan"),
    }
}

/// A `macaddr`/`macaddr8` literal in `macaddr_out`'s own spelling: `octets`
/// lowercase hex pairs joined by colons (I40). Those two rules are the whole
/// of that output function, which is why this type canonicalizes where the
/// two beside it decode per row — see [`equality_comparison`].
fn render_macaddr(text: &str, octets: usize) -> Option<String> {
    let OrderKey::Bytes(bytes) = macaddr_key(text, octets)? else {
        unreachable!("`macaddr_key` yields its octets")
    };
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"))
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

/// The form a **nested** literal has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence: the container's own grammar, then
/// how to spell what is inside it — as the dump does, which is each leaf
/// type's own output form.
///
/// That second half is advice rather than the boundary of what is accepted:
/// [`order_key`]'s integer arms are `str::parse`, so they take a leading `+`
/// and leading zeros no `*_out` writes, one of the exceptions [`accepted_form`]
/// lists (`docs/design/decisions.md`, "D55"). There is no per-leaf clause list, [`nested_key`]
/// answering only "not a value of this type".
fn nested_accepted_form(plan: &NestedCompare) -> String {
    // The one form with no leaf clause to add, because it has no leaf: an
    // `int2vector`'s elements are read by `int2vectorin` itself, so the
    // superset reaches all the way down and the sentence below would be false
    // here.
    if matches!(plan, NestedCompare::Int2Vector) {
        return "as whole numbers from -32768 to 32767 separated by spaces, and as nothing at \
                all for the empty vector"
            .to_string();
    }
    let container = match plan {
        NestedCompare::Array(element) if element.not_null => {
            "as an array literal — `{a,b}`, `{}` — with no `NULL` element, which its element \
             type, a `NOT NULL` domain, refuses"
        }
        NestedCompare::Array(_) => "as an array literal — `{a,b}`, `{}`, a bare `NULL` element",
        NestedCompare::Record(fields) if fields.iter().any(|(_, field)| field.not_null) => {
            "as a composite literal — `(a,b)`, a field left empty for NULL but where its type is \
             a `NOT NULL` domain"
        }
        NestedCompare::Record(_) => "as a composite literal — `(a,b)`, a field left empty for NULL",
        NestedCompare::Range { .. } => {
            "as a range literal — `[a,b)`, `empty`, a bound left empty for unbounded — whose \
             lower bound is not above its upper"
        }
        NestedCompare::Multirange { .. } => {
            "as a multirange literal — `{[a,b),[c,d)}`, `{}` — each member a range whose lower \
             bound is not above its upper"
        }
        // Answered above, before the leaf clause this arm cannot carry.
        NestedCompare::Int2Vector => "as an int2vector literal",
        // Neither is reachable: a leaf plan is never a column's whole
        // comparison, and an uncomparable one refuses before a literal is
        // read.
        NestedCompare::Leaf { .. } | NestedCompare::Uncomparable { .. } => "as a nested literal",
    };
    format!(
        "{container} — with each element, field or bound spelled as the dump spells it, in that \
         type's own output form"
    )
}

/// The form a literal of `kind` has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence — read after "which is written".
///
/// It lives beside the grammar rather than beside [`CompareKind`] because it
/// describes what [`literal_key`] and [`equality_comparison`] accept, which is
/// each type's `*_out` form, widened by an integer's sign and leading zeros,
/// a float's sign, bare point, exponent case and `inf` or `nan` in any case,
/// a `uuid` or `macaddr` hex digit's case, a `uuid`'s omitted hyphens,
/// hyphens after any group of four and enclosing braces,
/// either `numeric` kind's leading or trailing point and leading zeros, a
/// network value's full-width or zero-padded netmask, an IPv6 address's
/// uncompressed, zero-padded or upper-case groups, a `cidr` written with no
/// netmask, and a time's trailing fractional zeros (`docs/design/decisions.md`, "D55").
/// `jsonb` needs the least here, its grammar being the whole of `jsonb_in`.
///
/// Two arms answer with the kind's own payload, because there the payload
/// *is* the answer: an enum's declared labels, and a `numeric(p,s)`'s scale
/// — see [`ENUM_LABELS_SHOWN`] and [`decimal_accepted_form`].
///
/// [`CompareKind::Text`] and [`CompareKind::PaddedText`] never refuse a
/// literal, and an enum with no labels is refused by `pgtype::comparison_for`
/// outright, so both arms are unreachable; they are written out rather than
/// `unreachable!` so a future kind's mistake is not a panic on a diagnostic
/// path.
fn accepted_form(kind: &CompareKind) -> String {
    use CompareKind as K;
    match kind {
        K::Bool => "`t` or `f`".into(),
        // The width is half the sentence, as for `oid`: past it is the
        // refusal a well-formed literal meets (I60).
        K::Int { bytes } => {
            let range = int_range(*bytes);
            format!("as a whole number from {} to {}", range.start(), range.end())
        }
        // The width *is* the refusal: `oidin` wraps a negative and this does
        // not, so the range is the useful half of the sentence.
        K::UnsignedInt => "as a whole number from 0 to 4294967295".into(),
        K::Float32 => {
            "as a number within `real`'s range, or `Infinity`, `-Infinity` or `NaN`".into()
        }
        K::Float64 => {
            "as a number within `double precision`'s range, or `Infinity`, `-Infinity` or `NaN`"
                .into()
        }
        // A typmod rejects an infinity (I34), so a `numeric(p,s)` — and a
        // `numeric` past 76 digits — has `NaN` and nothing else. Only the
        // typed arm carries a scale to be finer than: a `p > 76` column is
        // held as text and compared by value through `NumericKey`, which
        // normalizes rather than rescaling.
        // The digit bounds are `numeric_in`'s storage format's (I63), which
        // a typed arm's scale and `i256` already fall inside.
        K::Decimal { scale, .. } => decimal_accepted_form(*scale),
        K::Numeric { infinities: false, .. } => format!("{NUMERIC_DIGITS}, or `NaN`"),
        K::Numeric { infinities: true, .. } => {
            format!("{NUMERIC_DIGITS}, or `NaN`, `Infinity` or `-Infinity`")
        }
        K::Enum { labels, .. } if !labels.is_empty() => enum_accepted_form(labels),
        K::Enum { .. } => "as one of the type's own declared labels".into(),
        // The range is half the sentence, as for an integer: a day the
        // calendar lacks, or one past it, is the refusal a well-formed
        // literal meets (I61).
        K::Date => "`YYYY-MM-DD`, optionally suffixed ` BC`, a day from `4714-11-24 BC` to \
                    `5874897-12-31`, or `infinity`/`-infinity`"
            .into(),
        K::Time => {
            "`HH:MM:SS`, optionally with a fractional second, no later than `24:00:00`".into()
        }
        K::Timestamp { with_tz: false } => {
            "`YYYY-MM-DD HH:MM:SS`, optionally with a fractional second and suffixed ` BC`, from \
             `4714-11-24 00:00:00 BC` to `294276-12-31 23:59:59.999999`, or \
             `infinity`/`-infinity`"
                .into()
        }
        K::Timestamp { with_tz: true } => {
            "`YYYY-MM-DD HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, \
             optionally with a fractional second and suffixed ` BC`, from `4714-11-24 00:00:00+00 \
             BC` to `294276-12-31 23:59:59.999999+00`, or `infinity`/`-infinity`"
                .into()
        }
        K::TimeTz => {
            "`HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, optionally \
             with a fractional second, no later than `24:00:00`"
                .into()
        }
        // `interval_out` under `IntervalStyle = postgres` (I4), which is the
        // only style a `pg_dump` connection writes; the bounds are I62's.
        K::Interval { .. } | K::IntervalFields { .. } => {
            "the way `interval` prints it — `1 year 2 mons 3 days`, `-01:00:00`, `00:00:00` for \
             zero — each unit once, its minutes under `60` and seconds at most `60`, or \
             `infinity`/`-infinity`"
                .into()
        }
        // `network_in`'s grammar (I67), which reads more than this says.
        K::Network { cidr: false } => {
            "as an IPv4 or IPv6 address, optionally followed by `/bits`, an IPv4 one omitting no \
             octet but those past the netmask's whole bytes"
                .into()
        }
        K::Network { cidr: true } => {
            "as an IPv4 or IPv6 network, optionally followed by `/bits`, with no bit set below \
             the netmask, an IPv4 one omitting any trailing octets, which are zero"
                .into()
        }
        K::MacAddr { octets: 8 } => "as eight colon-separated hex pairs".into(),
        K::MacAddr { .. } => "as six colon-separated hex pairs".into(),
        K::Uuid => {
            "as 32 hex digits, grouped `8-4-4-4-12`, a hyphen allowed only after a group of four \
             but the last, the whole optionally in braces"
                .into()
        }
        K::Bytea => {
            "as `\\x` followed by hex pairs, or as `bytea_output = escape` writes it".into()
        }
        K::Jsonb => "as a JSON document".into(),
        K::Text { .. } | K::PaddedText { .. } => "as any text".into(),
    }
}

/// How many of an enum's labels [`accepted_form`] names before it stops
/// counting them out.
///
/// A count cap rather than a length cap (`map.rs`'s `SPAN_STORED_TEXT_MAX_BYTES` is the other
/// shape): every label a message prints is printed whole, where a length cap
/// would hand the user a spelling that is not a label. Nothing bounds how
/// many labels a type declares.
///
/// The overflow clause has somewhere to send the reader: `pgdt info --detail`
/// prints every label of an enum column *and* lists every user-defined type
/// with its labels, both uncapped (`docs/design/decisions.md`, "The CLI").
const ENUM_LABELS_SHOWN: usize = 12;

/// The enum clause: the declared labels themselves, which are the whole of
/// what a refused enum literal is missing.
///
/// Each label is single-quoted with any interior quote doubled — the spelling
/// the dump's own `CREATE TYPE … AS ENUM (…)` writes and the CLI's `dequote`
/// accepts — so a printed label pastes straight back into
/// `--filter "col=<label>"`. The CLI's own `label_list` renders the same way
/// and is not shared with it, sitting a layer above.
fn enum_accepted_form(labels: &[String]) -> String {
    let shown = labels.len().min(ENUM_LABELS_SHOWN);
    let list = labels[..shown]
        .iter()
        .map(|label| format!("'{}'", label.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    match labels.len() - shown {
        0 => format!("as one of the type's declared labels: {list}"),
        more => format!(
            "as one of the type's declared labels: {list}, and {more} more; see `info --detail`"
        ),
    }
}

/// A scale-free `numeric` literal's shape: `numeric_in`'s storage bounds
/// (I63), stated as the digits either side of the point.
const NUMERIC_DIGITS: &str =
    "as a number of at most 131072 digits before the point and 16383 after it";

/// The `numeric(p,s)` clause, which is about the **scale** because that is
/// what the refusal is about: `decode::decimal_unscaled_digits` drops a
/// trailing digit only when it is zero, so `--filter 'price>1.005'` on a
/// `numeric(10,2)` is refused.
///
/// Precision says nothing here, binding a field alone: a literal wider than
/// the column can hold still compares against every value in it, as the
/// server coerces it with no typmod (I63). A negative scale is legal from
/// PostgreSQL 15 and means the column stores multiples of a power of ten,
/// which the literal's reader enforces by refusing to drop a non-zero digit
/// off the integer part.
fn decimal_accepted_form(scale: i8) -> String {
    match scale {
        0 => "as a whole number, or `NaN`".into(),
        s if s > 0 => {
            let plural = if s == 1 { "" } else { "s" };
            format!("as a number with at most {s} decimal place{plural}, or `NaN`")
        }
        s => {
            let step = format!("1{}", "0".repeat(usize::from(s.unsigned_abs())));
            format!("as a whole number that is a multiple of {step}, or `NaN`")
        }
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
