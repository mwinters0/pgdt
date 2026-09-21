//! Post-parse row filtering (`docs/design/decisions.md`, "Predicates").

use std::cmp::Ordering;

use arrow::datatypes::i256;

use crate::copy::{RawRow, RowSplit};
use crate::decode;
use crate::diagnostic::{Finding, Severity};
use crate::nested;
use crate::pgtype::{
    CompareKind, ComparisonDivergence, ComparisonPlan, ComparisonSemantics, NestedCompare,
    NestedPlan, UnanswerableReason, arrow_position_divergences,
};
use crate::resolve::{ColumnResolution, ResolvedSchema};
use crate::{Error, Result};

/// Comparison operator for [`Predicate`].
///
/// Every operator but the two NULL tests compares typed, through the column's
/// own [`ComparisonPlan`]: the four ordering operators decode both sides and
/// compare the values, `Eq`/`Ne` take the cheapest of three canonicalizations
/// that gives the server's answer for that column ([`equality_comparison`]).
/// A nested column with a plan is compared structurally under every operator.
/// Where the register has no plan for a column — it did not resolve, one of
/// its nested positions is not compared, or this build orders its type not at
/// all — `Eq`/`Ne` compare the canonical `*_out` text the file holds and the
/// ordering operators are refused.
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
    /// as text — the four that need a `Mapped` column the register gives an
    /// order.
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
        }
    }
}

/// A single-column post-parse filter: `column <op> value`, and the **leaf**
/// of an [`Expr`] — a query carries one expression and keeps a row only if
/// its root evaluates [`Truth::True`] (`QueryOptions::filter`). `column` is
/// matched against the queried table's column names, the `COPY` header's
/// list. `value` is `None` for `IsNull`/`IsNotNull`, which need no
/// comparison value; it is always `Some` for every other operator.
///
/// `value` is read with the column's own decoder wherever the register gives
/// the column a comparison, whatever the operator, once when the block's schema
/// resolves rather than per row — so a literal that
/// is not a value of the column's type is `Error::PredicateValueDecode`
/// before any row is read, and both sides of a `numeric(p,s)` comparison
/// carry that column's scale. An ordering operator keeps the decoded key and
/// decodes the field to match; `Eq`/`Ne` usually keep the literal rendered
/// back into the `*_out` spelling the file holds and compare bytes
/// ([`equality_comparison`]).
///
/// A NULL field is [`Truth::Unknown`] under every comparing operator, and a
/// row survives only where the root is `True`. The four operators that are
/// two-valued on a NULL field are `IsNull`/`IsNotNull`, which ask about the
/// NULL directly, and `IsDistinctFrom`/`IsNotDistinctFrom`, which count it as
/// a value (`docs/design/decisions.md`, "D53").
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
/// Nothing here is parsed: `Expr` is a struct an embedder fills in field by
/// field, and the `--where` grammar that builds one from text lives in the
/// CLI (`docs/design/decisions.md`, "D60").
#[derive(Debug, Clone)]
pub enum Expr {
    Term(Predicate),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

impl Expr {
    /// The conjunction of `terms` — the shape a repeated `--filter` builds.
    /// Over an empty iterator it is the filter that keeps every row.
    pub fn all(terms: impl IntoIterator<Item = Predicate>) -> Self {
        Self::And(terms.into_iter().map(Self::Term).collect())
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
/// `crate::diagnostic::DiagnosticSink` beside them as a
/// [`crate::diagnostic::Finding`].
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
                 value — the orders differ, and so does equality: a literal matches only the \
                 spelling the server writes (`'12:00+00'` misses `12:00:00+00`, `'10.0.0.1/32'` \
                 misses an inet's `10.0.0.1`), and some types write one value two ways (`1.5` \
                 and `1.50`)",
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
            ComparisonDivergence::NestedArrowOrder => format!(
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
/// **Under [`ComparisonSemantics::Arrow`]** they say how DataFusion's
/// comparison of the emitted value differs from the server's, whether or not
/// this build answers a term on the column — `ORDER BY`, `MIN`/`MAX` and every
/// filter a plan keeps reach it too, so a nested column whose every term is
/// refused here still reports [`ComparisonDivergence::NestedArrowOrder`] for
/// its order, and each position inside it its own divergence under its path
/// ([`crate::pgtype::NestedCompare::arrow_divergences`]). A
/// kind [`CompareKind::arrow_order`] moves reports
/// [`CompareKind::arrow_divergence`], beside any collation the register
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
            ComparisonSemantics::Arrow => arrow_divergences(resolved, index, &declared),
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
    if resolved.columns[index] != ColumnResolution::Mapped {
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
        _ if resolved.plans[index] != NestedPlan::Scalar => Vec::new(),
        ComparisonPlan::Compared { divergence, .. } => divergence.map(column).unwrap_or_default(),
        ComparisonPlan::Refused => Vec::new(),
    }
}

/// [`column_divergences`] under Arrow's semantics, read off what the column
/// emits rather than off which terms this build answers.
///
/// A column that did not resolve says nothing here: its fall-back
/// [`crate::resolve::ColumnNote`] is already a `Warning` saying the value is
/// the file's text, which is all DataFusion compares.
fn arrow_divergences(
    resolved: &ResolvedSchema,
    index: usize,
    declared: &str,
) -> Vec<DivergingPosition> {
    let column = |divergence| (None, declared.to_string(), divergence);
    if resolved.columns[index] != ColumnResolution::Mapped {
        return Vec::new();
    }
    if resolved.plans[index] != NestedPlan::Scalar {
        // The container's order, then each position's own divergence; a
        // plan with no tree has no position to name.
        let positions = match &resolved.comparisons[index] {
            ComparisonPlan::Nested(tree) => tree.arrow_divergences(),
            _ => Vec::new(),
        };
        return std::iter::once(column(ComparisonDivergence::NestedArrowOrder))
            .chain(positions.into_iter().map(|(path, declared, d)| (Some(path), declared, d)))
            .collect();
    }
    match &resolved.comparisons[index] {
        ComparisonPlan::Compared { kind, divergence } => {
            arrow_position_divergences(kind, *divergence).into_iter().map(column).collect()
        }
        // A scalar column's register never answers either; kept rather than
        // assumed away, the fallback being the emitted text.
        ComparisonPlan::Nested(_) | ComparisonPlan::Unanswerable(_) => {
            vec![column(ComparisonDivergence::NestedArrowOrder)]
        }
        // A mapped scalar with no comparison, which no declared type reaches
        // today: DataFusion compares its text, which is what this says.
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

    /// `[-]digits[.digits]`, the only shape `numeric_out` writes: no
    /// exponent, no sign but `-`, and at least one digit somewhere — the same
    /// *lexical* grammar [`decode::decimal_unscaled_digits`] accepts for a
    /// typmod'd column. The two differ only on the typmod, which a bare
    /// `numeric` has none of.
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

/// The furthest a `jsonb` number's exponent may move the decimal point,
/// bounding the digit string this builds. Ours rather than PostgreSQL's —
/// `numeric` reaches further, and what is bounded is where the decimal point
/// lands rather than the exponent itself, so a field holding a number whose
/// integer part runs past this many digits reaches it with no exponent
/// written at all.
const JSONB_MAX_EXPONENT: i64 = 100_000;

/// A recursive-descent reader over one JSON document, implementing what
/// `jsonb_in` accepts and nothing wider (I41): RFC 8259 with PostgreSQL's two
/// extra refusals — `\u0000`, which cannot be part of a `text` value, and a
/// lone surrogate half.
struct JsonCursor<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
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
            // them.
            _ => Jsonb::Number(self.number()?),
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
                0x00..=0x1f => return None,
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
    fn unicode_escape(&mut self) -> Option<char> {
        let first = self.hex4()?;
        if first == 0 {
            return None;
        }
        if (0xd800..0xdc00).contains(&first) {
            if !(self.eat(b'\\') && self.eat(b'u')) {
                return None;
            }
            let second = self.hex4()?;
            if !(0xdc00..0xe000).contains(&second) {
                return None;
            }
            return char::from_u32(0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00));
        }
        // A low surrogate with no high half in front of it.
        char::from_u32(first).filter(|_| !(0xdc00..0xe000).contains(&first))
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
    /// kept: `1e2`, `100` and `100.00` are one value (I41).
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
        // Where the point lands, counted in digits from the left of
        // `int ++ frac`. Both directions need padding, and both are bounded.
        let point = i64::try_from(int.len()).ok()? + exponent;
        if !(-JSONB_MAX_EXPONENT..=JSONB_MAX_EXPONENT).contains(&point) {
            return None;
        }
        let digits = format!("{int}{frac}");
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

/// One `jsonb` document, parsed. `None` for text the server's own parser would
/// refuse — which for a *field* is `Error::FieldDecode` and for a filter's
/// literal `Error::PredicateValueDecode`, the same two faults every other
/// comparison raises.
fn parse_jsonb(text: &str) -> Option<Jsonb> {
    let mut cursor = JsonCursor { bytes: text.as_bytes(), at: 0, depth: 0 };
    cursor.skip_ws();
    let value = cursor.value()?;
    cursor.skip_ws();
    (cursor.at == cursor.bytes.len()).then_some(value)
}

/// A `jsonb` comparison key: the document, with a **top-level scalar wrapped
/// in the one-element pseudo-array PostgreSQL stores it in** (I41). Modelling
/// the wrapper is what gives `1 < [1]`, `1 < [1,2]` and `1 > []`.
fn jsonb_key(text: &str) -> Option<OrderKey> {
    Some(OrderKey::Jsonb(match parse_jsonb(text)? {
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
    IntervalFields(i64, i64, i128),
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

/// The rank of a finite value — the middle of the four [`OrderKey::rank`]
/// classes, and the only one whose members are compared by value.
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
/// and a `numeric` writes `NaN`. Nothing else is accepted — a `date` field or
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
        | CompareKind::Interval
        | CompareKind::IntervalFields => match text {
            "infinity" => Some(OrderKey::PositiveInfinity),
            "-infinity" => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        CompareKind::Decimal(_) => (text == "NaN").then_some(OrderKey::NotANumber),
        CompareKind::Numeric { infinities } => match text {
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
/// refuses — and a month or day count outside `i32`, or a time part past what
/// Arrow's nanoseconds hold, besides. The fusing is this function's alone: a
/// decoder that did it would
/// lose the fields Arrow carries separately.
fn interval_span(text: &str) -> Option<i128> {
    let (months, days, time) = decode::interval_parts(text)?;
    let whole_days = i128::from(months.checked_mul(30)?.checked_add(days)?);
    whole_days.checked_mul(86_400_000_000)?.checked_add(time)
}

/// A `time with time zone`, split into the UTC-equivalent instant and the
/// zone PostgreSQL stores — seconds *west* of GMT, the negation of the offset
/// the value displays. `timetz_cmp_internal` sorts by the first and breaks
/// ties with the second (I40), so `00:00:00+00` and `01:00:00+01` are the
/// same instant and still not equal.
fn timetz_key(text: &str) -> Option<OrderKey> {
    let (time_only, displayed) = decode::extract_offset(text)?;
    let (seconds, micros) = decode::parse_time_of_day(time_only)?;
    let zone = -displayed;
    let utc = seconds.checked_mul(1_000_000)?.checked_add(micros)?.checked_add(zone * 1_000_000)?;
    Some(OrderKey::TimeTz { utc, zone })
}

/// An `inet` or `cidr` value: `<address>[/<bits>]`, the form
/// `pg_inet_net_ntop` writes, with `cidr_out` always appending the netmask
/// and `inet_out` omitting it when it is the family's full width (I40).
///
/// The address grammar is Rust's, which is narrower than `inet_in`'s: an
/// abbreviated IPv4 address — `10`, meaning `10.0.0.0/8` — is refused
/// (`docs/design/decisions.md`, "D55").
///
/// `cidr` additionally refuses a value with a bit set below its netmask,
/// because `cidr_in` does: the *only* thing separating the two types, their
/// comparison being identical.
fn network_key(text: &str, cidr: bool) -> Option<OrderKey> {
    let (address, netmask) = match text.split_once('/') {
        Some((address, netmask)) => (address, Some(netmask)),
        None => (text, None),
    };
    let mut addr = [0u8; 16];
    let v6 = match address.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(v4) => {
            addr[..4].copy_from_slice(&v4.octets());
            false
        }
        std::net::IpAddr::V6(v6) => {
            addr.copy_from_slice(&v6.octets());
            true
        }
    };
    let maxbits: u8 = if v6 { 128 } else { 32 };
    let bits = match netmask {
        None => maxbits,
        Some(digits) => {
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            digits.parse::<u8>().ok()?
        }
    };
    if bits > maxbits {
        return None;
    }
    if cidr && (bits..maxbits).any(|bit| addr[usize::from(bit / 8)] & (0x80 >> (bit % 8)) != 0) {
        return None;
    }
    Some(OrderKey::Network(NetworkKey { v6, bits, addr }))
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
    if let Some(special) = special_order_key(kind, text) {
        return Some(special);
    }
    Some(match kind {
        CompareKind::Bool => OrderKey::Bool(decode::decode_bool(text)?),
        // Parsed as `i64` whatever the column's width: a literal outside a
        // `smallint`'s range still orders correctly against every value the
        // column can hold.
        CompareKind::Int => OrderKey::Int(text.parse::<i64>().ok()?),
        // `u32`, and the width *is* the refusal: `oidin` reads `-1` as
        // 4294967295 and this build does not implement that wrap, so a
        // signed literal is `Error::PredicateValueDecode`. Every value the
        // column can hold widens into `i64` unchanged.
        CompareKind::UnsignedInt => OrderKey::Int(text.parse::<u32>().ok()?.into()),
        CompareKind::Float32 => OrderKey::Float(f64::from(decode::decode_f32(text)?)),
        CompareKind::Float64 => OrderKey::Float(decode::decode_f64(text)?),
        CompareKind::Decimal(scale) => {
            OrderKey::Decimal(i256::from_string(&decode::decimal_unscaled_digits(text, *scale)?)?)
        }
        CompareKind::Numeric { .. } => OrderKey::Numeric(NumericKey::parse(text)?),
        // A label the type does not declare is not a value of the column, so
        // it is the same fault an unparseable number is. The linear scan is
        // over a label list, a handful of entries in practice.
        CompareKind::Enum(labels) => OrderKey::Int(labels.iter().position(|l| l == text)? as i64),
        CompareKind::Date => OrderKey::Int(decode::decode_date32(text)?.into()),
        CompareKind::Time => OrderKey::Int(decode::decode_time64_micros(text)?),
        CompareKind::Timestamp { with_tz } => {
            OrderKey::Int(decode::decode_timestamp_micros(text, *with_tz)?)
        }
        CompareKind::Interval => OrderKey::Interval(interval_span(text)?),
        CompareKind::IntervalFields => {
            let (months, days, time) = decode::interval_parts(text)?;
            OrderKey::IntervalFields(months, days, time)
        }
        CompareKind::TimeTz => timetz_key(text)?,
        CompareKind::Network { cidr } => network_key(text, *cidr)?,
        CompareKind::MacAddr { octets } => macaddr_key(text, *octets)?,
        CompareKind::Jsonb => jsonb_key(text)?,
        CompareKind::Uuid => OrderKey::Bytes(decode::decode_uuid(text)?.to_vec()),
        CompareKind::Bytea => OrderKey::Bytes(decode::decode_bytea(text)?),
        CompareKind::Text => OrderKey::Text(text.to_string()),
        // `bcTruelen` on both sides, which is what makes this the server's
        // comparison rather than one over the padding (I38). The blank is
        // ASCII `0x20` and nothing else; a tab is a value byte.
        CompareKind::PaddedText => OrderKey::Text(text.trim_end_matches(' ').to_string()),
    })
}

/// PostgreSQL's float order, not Rust's: `NaN` is greater than every other
/// value, infinities included, and `NaN = NaN` is true (I33), where Rust's
/// `partial_cmp` answers `None`. `real`/`double precision` are the only
/// columns whose decoder yields a NaN at all: a `NaN` in a `numeric(p,s)`
/// column has no `Decimal128` representation and fails to decode first (I4).
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

    /// [`compare_keys`]: a total order over the keys of one kind.
    pub(crate) fn compare(&self, other: &Self) -> Ordering {
        compare_keys(&self.0, &other.0)
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
/// `input` is which grammar to read it in, and it is the whole of what
/// separates the two sides: a *field* is read with [`crate::nested`]'s strict
/// `decode_*`, a *literal* with the `array_in`/`record_in` supersets
/// (`parse_*`), which take `{a, b}` and `{ 1 , 2 }` (I44).
///
/// The leaf grammar does not widen with it (`docs/design/decisions.md`,
/// "D58"): a leaf is read by [`order_key`], which implements that type's
/// `*_out` form and no more, so `--filter 'p=( 1 , a )'` is refused.
///
/// `None` is "not a value of this type", which is
/// `Error::PredicateValueDecode` for a literal and `Error::FieldDecode` for a
/// field.
fn nested_key(plan: &NestedCompare, text: &str, input: bool) -> Option<NestedKey> {
    Some(match plan {
        NestedCompare::Leaf { kind, .. } => NestedKey::Leaf(order_key(kind, text)?),
        // Refused before any value is read: `resolve_term` never builds a
        // comparison over a tree holding one.
        NestedCompare::Uncomparable { .. } => return None,
        NestedCompare::Array(element) => {
            let literal =
                if input { nested::parse_array(text) } else { nested::decode_array(text) }?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            for value in &literal.elements {
                elements.push(match value {
                    Some(value) => Some(nested_key(element, value, input)?),
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
            for ((_, plan), value) in plans.iter().zip(fields.drain(..)) {
                out.push(match value {
                    Some(value) => Some(nested_key(plan, &value, input)?),
                    None => None,
                });
            }
            NestedKey::Record(out)
        }
        NestedCompare::Range { bound, discrete } => {
            let literal =
                if input { nested::parse_range(text) } else { nested::decode_range(text) }?;
            NestedKey::Range(Box::new(range_key(bound, &literal, *discrete, input)?))
        }
        NestedCompare::Multirange { bound, discrete } => {
            let literals = if input {
                nested::parse_multirange(text)
            } else {
                nested::decode_multirange(text)
            }?;
            let mut members = Vec::with_capacity(literals.len());
            for literal in &literals {
                members.push(range_key(bound, literal, *discrete, input)?);
            }
            NestedKey::Multirange(canonical_multirange(members, *discrete)?)
        }
    })
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
    discrete: bool,
    input: bool,
) -> Option<RangeKey> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        Some(RangeBoundKey {
            value: match text {
                Some(text) => Some(nested_key(bound, text, input)?),
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

/// `make_range`: `range_serialize`'s type-independent checks, then the range
/// type's canonical function where it has one, then those checks again — the
/// order the server applies them in, and the reason `int4range '(1,2)'` is
/// `empty` (I46).
///
/// `None` is the server's `22000` — a lower bound above its upper, a *semantic*
/// refusal the container grammar cannot see (I44), so it is raised here where
/// the bounds have been decoded — or a discrete bound whose successor
/// overflows its subtype.
fn make_range(
    lower: RangeBoundKey,
    upper: RangeBoundKey,
    empty: bool,
    discrete: bool,
) -> Option<RangeKey> {
    let serialized = serialize_range(lower, upper, empty)?;
    if !discrete || serialized.empty {
        return Some(serialized);
    }
    // `int4range_canonical` and its two siblings, which differ only in the
    // width they overflow at: an exclusive lower bound becomes inclusive at
    // the successor, an inclusive upper becomes exclusive at the successor.
    //
    // The `Int` pattern is `daterange_canonical`'s `DATE_NOT_FINITE` guard: a
    // date `infinity` decodes to `OrderKey::PositiveInfinity` rather than to
    // a day count, so it matches no arm here and is left as written, which is
    // what makes `[2020-01-01,infinity]` keep its inclusive upper (I34, I46).
    let successor = |bound: &RangeBoundKey| match &bound.value {
        Some(NestedKey::Leaf(OrderKey::Int(n))) => n.checked_add(1).map(|n| {
            Some(RangeBoundKey {
                value: Some(NestedKey::Leaf(OrderKey::Int(n))),
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
fn canonical_multirange(mut members: Vec<RangeKey>, discrete: bool) -> Option<Vec<RangeKey>> {
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
        // The server's own order. The middle test needs the sort:
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
fn ranges_adjacent(a: &RangeKey, b: &RangeKey, discrete: bool) -> bool {
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
fn bounds_adjacent(upper: &RangeBoundKey, lower: &RangeBoundKey, discrete: bool) -> bool {
    match compare_bound_values(upper, lower) {
        Ordering::Equal => upper.inclusive != lower.inclusive,
        Ordering::Greater => false,
        Ordering::Less => {
            discrete
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
fn range_union(a: &RangeKey, b: &RangeKey, discrete: bool) -> Option<RangeKey> {
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
/// "D57"):
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
///   `real`/`double precision`, which have two zeros. `time with time zone`
///   and `inet`/`cidr` decode for a reason about this build instead, and
///   `macaddr` renders (`docs/design/decisions.md`, "D57").
///
/// `None` when the literal is not a value of the column's type at all, which
/// is `Error::PredicateValueDecode` — the same refusal an ordering operator
/// makes, on the same output-form-only grammar (`docs/design/decisions.md`,
/// "D55").
fn equality_comparison(kind: &CompareKind, text: &str) -> Option<Comparison> {
    use CompareKind as K;
    let rendered = match kind {
        // Both sides per row. `order_key` reads a special value on the way,
        // so `NaN = NaN` and `Infinity = Infinity` come out of the same rank
        // rule the ordering operators use (I34).
        K::Float32
        | K::Float64
        | K::Numeric { .. }
        | K::Interval
        | K::IntervalFields
        | K::Jsonb
        | K::TimeTz
        | K::Network { .. } => {
            return Some(Comparison::Decoded { kind: kind.clone(), bound: order_key(kind, text)? });
        }
        K::PaddedText => return Some(Comparison::Trimmed(text.trim_end_matches(' ').to_string())),
        // A special value is written in its own type's `*_out` spelling on
        // both sides, so it renders to itself. Only `date`, `timestamp` and
        // `numeric(p,s)` reach this arm and admit one.
        _ if special_order_key(kind, text).is_some() => text.to_string(),
        K::Bool => decode::render_bool(decode::decode_bool(text)?).to_string(),
        K::Int => text.parse::<i64>().ok()?.to_string(),
        K::UnsignedInt => text.parse::<u32>().ok()?.to_string(),
        K::Decimal(scale) => {
            decode::render_decimal(&decode::decimal_unscaled_digits(text, *scale)?, *scale)
        }
        // A label is its own canonical form; what the lookup buys is the
        // refusal, a string the type does not declare being no value of the
        // column.
        K::Enum(labels) => labels.iter().find(|label| label.as_str() == text)?.clone(),
        K::Date => decode::render_date32(decode::decode_date32(text)?),
        K::Time => decode::render_time64_micros(decode::decode_time64_micros(text)?),
        K::Timestamp { with_tz } => decode::render_timestamp_micros(
            decode::decode_timestamp_micros(text, *with_tz)?,
            *with_tz,
        ),
        K::MacAddr { octets } => render_macaddr(text, *octets)?,
        K::Uuid => decode::render_uuid(&decode::decode_uuid(text)?),
        K::Bytea => decode::render_bytea(&decode::decode_bytea(text)?),
        // The identity: `=` on a text column is a byte comparison, with no
        // per-row work added.
        K::Text => text.to_string(),
    };
    Some(Comparison::Canonical(rendered))
}

/// The form a **nested** literal has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence: the container's own grammar, then
/// how to spell what is inside it — as the dump does, which is each leaf
/// type's own output form.
///
/// That second half is advice rather than the boundary of what is accepted:
/// [`order_key`]'s integer arms are `str::parse`, so they take a leading `+`
/// and leading zeros no `*_out` writes (`docs/design/decisions.md`, "D55",
/// whose exception this is). There is no per-leaf clause list, [`nested_key`]
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
        NestedCompare::Array(_) => "as an array literal — `{a,b}`, `{}`, a bare `NULL` element",
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
/// describes what [`order_key`] and [`equality_comparison`] accept, which is
/// each type's `*_out` form, widened by an integer's sign and leading zeros,
/// a `uuid` or `macaddr` hex digit's case, a `uuid`'s hyphen placement, and
/// either `numeric` kind's leading or trailing point and leading zeros
/// (`docs/design/decisions.md`, "D55").
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
        K::Int => "as an optionally signed whole number".into(),
        // The width *is* the refusal: `oidin` wraps a negative and this does
        // not, so the range is the useful half of the sentence.
        K::UnsignedInt => "as a whole number from 0 to 4294967295".into(),
        K::Float32 | K::Float64 => "as a number, or `Infinity`, `-Infinity` or `NaN`".into(),
        // A typmod rejects an infinity (I34), so a `numeric(p,s)` — and a
        // `numeric` past 76 digits — has `NaN` and nothing else. Only the
        // typed arm carries a scale to be finer than: a `p > 76` column is
        // compared as text through `NumericKey`, which normalizes rather than
        // rescaling.
        K::Decimal(scale) => decimal_accepted_form(*scale),
        K::Numeric { infinities: false } => "as a number, or `NaN`".into(),
        K::Numeric { infinities: true } => {
            "as a number, or `NaN`, `Infinity` or `-Infinity`".into()
        }
        K::Enum(labels) if !labels.is_empty() => enum_accepted_form(labels),
        K::Enum(_) => "as one of the type's own declared labels".into(),
        K::Date => "`YYYY-MM-DD`, optionally suffixed ` BC`, or `infinity`/`-infinity`".into(),
        K::Time => "`HH:MM:SS`, optionally with a fractional second".into(),
        K::Timestamp { with_tz: false } => {
            "`YYYY-MM-DD HH:MM:SS`, optionally with a fractional second and suffixed ` BC`, or \
             `infinity`/`-infinity`"
                .into()
        }
        K::Timestamp { with_tz: true } => {
            "`YYYY-MM-DD HH:MM:SS+HH`, the offset required, optionally with a fractional second \
             and suffixed ` BC`, or `infinity`/`-infinity`"
                .into()
        }
        K::TimeTz => {
            "`HH:MM:SS+HH`, the offset required, optionally with a fractional second".into()
        }
        // `interval_out` under `IntervalStyle = postgres` (I4), which is the
        // only style a `pg_dump` connection writes.
        K::Interval | K::IntervalFields => {
            "the way `interval` prints it — `1 year 2 mons 3 days`, `-01:00:00`, `00:00:00` for \
             zero — or `infinity`/`-infinity`"
                .into()
        }
        K::Network { cidr: false } => {
            "as a full IPv4 or IPv6 address, optionally followed by `/bits`".into()
        }
        K::Network { cidr: true } => {
            "as a full IPv4 or IPv6 address followed by `/bits`, with no bit set below the netmask"
                .into()
        }
        K::MacAddr { octets: 8 } => "as eight colon-separated hex pairs".into(),
        K::MacAddr { .. } => "as six colon-separated hex pairs".into(),
        K::Uuid => "as 32 hex digits, grouped `8-4-4-4-12`".into(),
        K::Bytea => "as `\\x` followed by hex pairs".into(),
        K::Jsonb => "as a JSON document".into(),
        K::Text | K::PaddedText => "as any text".into(),
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

/// The `numeric(p,s)` clause, which is about the **scale** because that is
/// what the refusal is about: `decode::decimal_unscaled_digits` drops a
/// trailing digit only when it is zero, so `--filter 'price>1.005'` on a
/// `numeric(10,2)` is refused.
///
/// Precision says nothing here and is not carried by
/// [`CompareKind::Decimal`]: a literal wider than the column can hold still
/// compares against every value in it. A negative scale is legal from
/// PostgreSQL 15 and means the column stores multiples of a power of ten,
/// which the decoder enforces by refusing to drop a non-zero digit off the
/// integer part.
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
    /// `=`/`!=` against the literal already rendered into the `*_out` form the
    /// file holds — a byte comparison per row (see [`equality_comparison`]).
    Canonical(String),
    /// `=`/`!=` on a `character(n)` column, against the literal with its own
    /// trailing blanks already gone.
    Trimmed(String),
    /// `=`/`!=` where the field has to be decoded per row too, because the
    /// file's spelling of a value is not unique.
    Decoded { kind: CompareKind, bound: OrderKey },
    /// A nested column, under **any** comparing operator: both sides read
    /// into a [`NestedKey`] and compared structurally
    /// (`docs/design/decisions.md`, "D58"). One variant for both operator
    /// families, where a scalar has three, because a nested value's `=` is
    /// byte-comparable only when *every* leaf beneath it canonicalizes.
    ///
    /// *Rejected: rendering the literal back and comparing bytes where every
    /// leaf allows it* — a second code path whose decoded half, the half that
    /// has to be right when a leaf does not canonicalize, no oracle case
    /// covers.
    ///
    /// Boxed: a `NestedKey` carries three vectors, and this variant is the
    /// only large one in an enum that sits inside every resolved leaf.
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
    /// The column's kind and the term's literal read as a key, present only
    /// where the column's plan orders **exactly** — a `Compared` plan with no
    /// divergence, which is the only order a gathered bound is taken under.
    /// Carried for the equality operators too, whose [`Comparison`] keeps no
    /// kind. Boxed, as [`Comparison::Nested`] is, because every resolved
    /// leaf carries it.
    bounds: Option<Box<(CompareKind, OrderKey)>>,
    /// Whether a group's dictionary answers this term: one of the four
    /// equality operators, on a `Compared` column whose divergence, if any,
    /// does not reach equality.
    dictionary: bool,
}

impl BelievedStatistics {
    /// A term no statistic but the counts answers: a nested column, and a
    /// column compared as its text for want of a plan.
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
    /// `None` for `IS NULL`/`IS NOT NULL`, the two operators that compare
    /// nothing.
    compared: Option<ComparedTerm>,
}

impl ResolvedTerm {
    /// This term's divergences from PostgreSQL's own comparison. Empty for
    /// the two NULL tests, and for every term whose column answers this
    /// operator the way the server does; more than one only for a nested
    /// column with more than one diverging position.
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
/// declared type has no order (`json`, `box`, an unrecognised name). It names
/// the position and its type.
fn nested_refusal(path: &str, declared: &str) -> String {
    format!(
        "the column is nested and `{path}` inside it is `{declared}`, which has no order here — \
         PostgreSQL refuses the same comparison, since a container is ordered by its element \
         type's own comparison and this type has none"
    )
}
const NO_ORDER: &str = "this build defines no ordering for the column's declared type";
const NESTED_IN_ARROW: &str = "the column is nested (array, composite, range or multirange), and \
     this build does not compare a nested value in Arrow's semantics";

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
/// The two NULL tests need nothing. Every other operator reads the column's
/// [`ComparisonPlan`] and, where it compares, decodes the filter's own literal
/// here, so a value
/// that is not of the column's type is a fault reported once rather than a
/// filter that matches nothing (`docs/design/decisions.md`, "D54").
///
/// The two operator families part company on a column with no plan, in
/// PostgreSQL's semantics. An ordering operator is *refused*, before a row of this block flows, unless
/// the column resolved `Mapped` and the register gave it a comparison — a
/// nested column's included, where every position is compared; `Eq`/`Ne` fall
/// back to comparing the canonical `*_out` text the file holds. So a nested
/// column with an uncompared position still answers `=` and refuses `<`.
///
/// Nothing here reads the Arrow type: how a column compares is a conclusion
/// resolution already reached (`crate::pgtype::comparison_for`), carried in
/// [`ResolvedSchema::comparisons`].
///
/// **Under [`ComparisonSemantics::Arrow`]** a scalar compares by its kind's
/// [`CompareKind::arrow_order`], a column with no plan — every one of which
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
    if matches!(predicate.op, PredicateOp::IsNull | PredicateOp::IsNotNull) {
        return Ok(ResolvedTerm { op: predicate.op, index, compared: None });
    }
    let ordering = predicate.op.is_ordering();
    let refuse = |reason: &str| Error::UnorderedPredicateColumn {
        header_offset,
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        reason: reason.to_string(),
    };
    let arrow = semantics == ComparisonSemantics::Arrow;
    let refuse_nested_in_arrow = || Error::UncomparablePredicateColumn {
        header_offset,
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        reason: NESTED_IN_ARROW.to_string(),
    };
    let declared_type = resolved.notes[index].declared.clone().unwrap_or_default();
    // `value` is `Some` for every operator but the two NULL tests. An
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
    if resolved.columns[index] != ColumnResolution::Mapped {
        if ordering && !arrow {
            return Err(refuse(NOT_MAPPED));
        }
        plan = None;
    } else if let Some(ComparisonPlan::Unanswerable(reason)) = plan {
        // The one refusal that does not end by offering `=`/`!=`: the file
        // says the server's equality is not a comparison of the text it
        // holds, so the fall-through below would be a wrong answer rather
        // than a weaker one. The two NULL tests have already returned.
        return Err(Error::UncomparablePredicateColumn {
            header_offset,
            column: predicate.column.clone(),
            op: predicate.op.symbol(),
            reason: unanswerable_reason(reason),
        });
    } else if let Some(ComparisonPlan::Nested(tree)) = plan {
        if arrow {
            return Err(refuse_nested_in_arrow());
        }
        // A nested column whose *shape* is compared here but one of whose
        // positions is not: the ordering operators are refused naming that
        // position, and `=`/`!=` fall back to a byte comparison of the
        // container's whole text, announced rather than silent — `array_cmp`
        // raises for a `json` element rather than returning a comparison.
        if let Some((path, declared)) = tree.uncomparable() {
            if ordering {
                return Err(refuse(&nested_refusal(&path, &declared)));
            }
            fell_back = Some(tree);
            plan = None;
        }
    } else if resolved.plans[index] != NestedPlan::Scalar {
        // A column the *resolver* calls nested and the register does not.
        // One shape reaches it — a range whose DDL stated no `subtype`, so
        // the resolver keeps the struct with `Utf8View` bounds and the
        // register has no bound type to name a refusal after — and it is also
        // where the two walks would land if they disagreed about a declared
        // type, the alternative to refusing being a silent scalar comparison.
        if arrow {
            return Err(refuse_nested_in_arrow());
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
                ComparisonSemantics::Arrow => kind.arrow_order(),
            };
            let comparison = if ordering {
                Comparison::Ordered {
                    kind: kind.clone(),
                    bound: order_key(kind, text).ok_or_else(|| refuse_literal(kind))?,
                }
            } else {
                equality_comparison(kind, text).ok_or_else(|| refuse_literal(kind))?
            };
            let divergences: Vec<_> = divergence
                .filter(|d| ordering || d.affects_equality())
                .map(|d| (None, declared_type.clone(), d))
                .into_iter()
                .collect();
            let statistics = BelievedStatistics {
                // A literal that does not key reads no bounds.
                bounds: compared
                    .bounds_ordered_in(semantics)
                    .then(|| order_key(kind, text).map(|key| Box::new((kind.clone(), key))))
                    .flatten(),
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
                bound: nested_key(tree, text, true).ok_or_else(|| Error::PredicateValueDecode {
                    column: predicate.column.clone(),
                    op: predicate.op.symbol(),
                    value: text.to_string(),
                    declared_type: declared_type.clone(),
                    accepted: nested_accepted_form(tree),
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
        // canonical `*_out` text the file holds. In Arrow's semantics the
        // column is the `Utf8View` holding that text, which DataFusion orders
        // bytewise, and so is it ordered here; nothing was gathered for it.
        _ if ordering && arrow => (
            Comparison::Ordered {
                kind: CompareKind::Text,
                bound: order_key(&CompareKind::Text, text)
                    .ok_or_else(|| refuse_literal(&CompareKind::Text))?,
            },
            Vec::new(),
            BelievedStatistics::NONE,
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
            Comparison::Canonical(text.to_string()),
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
            BelievedStatistics::NONE,
        ),
    };
    // Arrow's answer is not the register's to qualify: a divergence from
    // PostgreSQL is the column's (`docs/design/decisions.md`, "D40").
    let divergences = if arrow { Vec::new() } else { divergences };
    Ok(ResolvedTerm {
        op: predicate.op,
        index,
        compared: Some(ComparedTerm {
            column: predicate.column.clone(),
            comparison,
            declared_type,
            divergences,
            statistics,
        }),
    })
}

impl ResolvedTerm {
    /// Evaluate this term against `raw_row`, in SQL's three-valued domain.
    /// `table` and `row_offset` are context for the one error this can raise:
    /// a field that does not decode as its mapped type under a comparison
    /// that reads it, which is `Error::FieldDecode`, worded exactly as the
    /// typed build path words it.
    ///
    /// A NULL field is [`Truth::Unknown`] under every comparing operator. The
    /// four that answer two-valued instead are `IsNull`/`IsNotNull`, which
    /// compare nothing, and the two `IS DISTINCT FROM` forms, which count
    /// NULL as a value — so `IsDistinctFrom` on a NULL field is `True` where
    /// `Ne` is `Unknown`.
    fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let field = split.field(raw_row.bytes(), self.index);
        let decoded = match field {
            Some(f) => raw_row.decode(f)?,
            None => None,
        };
        self.eval_value(decoded.as_deref()).ok_or_else(|| {
            let compared = self.compared.as_ref().expect("a NULL test decodes nothing");
            Error::FieldDecode {
                table: table.to_string(),
                column: compared.column.clone(),
                row_offset,
                declared_type: compared.declared_type.clone(),
                value: decoded.as_deref().unwrap_or_default().to_string(),
            }
        })
    }

    /// This term's answer for one **already unescaped** value of its column,
    /// `None` being SQL NULL — the row path's whole comparison, with no row
    /// in it. [`Self::eval`] is this after splitting and unescaping a field;
    /// a value that did not come from a row, such as a statistic's stored
    /// bound or dictionary entry, is answered by the same code and so cannot
    /// be answered differently.
    ///
    /// `None` back is a non-NULL value that is not a value of the column's
    /// type under this term's comparison — what the row path reports as
    /// `Error::FieldDecode`, the only case in which it returns nothing. The
    /// NULL tests and the two equalities comparing text never decode, so
    /// they always answer.
    #[inline]
    pub(crate) fn eval_value(&self, value: Option<&str>) -> Option<Truth> {
        let Some(compared) = self.compared.as_ref() else {
            return Some(Truth::of(match self.op {
                PredicateOp::IsNull => value.is_none(),
                _ => value.is_some(),
            }));
        };
        let Some(text) = value else {
            return Some(match self.op {
                PredicateOp::IsDistinctFrom => Truth::True,
                PredicateOp::IsNotDistinctFrom => Truth::False,
                _ => Truth::Unknown,
            });
        };
        Some(Truth::of(match &compared.comparison {
            Comparison::Ordered { kind, bound } => {
                let ord = compare_keys(&order_key(kind, text)?, bound);
                match self.op {
                    PredicateOp::Lt => ord.is_lt(),
                    PredicateOp::Le => ord.is_le(),
                    PredicateOp::Gt => ord.is_gt(),
                    _ => ord.is_ge(),
                }
            }
            Comparison::Canonical(bound) => (text == bound.as_str()) == self.wants_equal(),
            Comparison::Trimmed(bound) => {
                (text.trim_end_matches(' ') == bound.as_str()) == self.wants_equal()
            }
            Comparison::Decoded { kind, bound } => {
                compare_keys(&order_key(kind, text)?, bound).is_eq() == self.wants_equal()
            }
            // Two-valued throughout: a NULL *inside* the container is a value
            // of it, and only the whole field being NULL is unknown — which
            // was decided above, before any of this runs.
            Comparison::Nested(nested) => {
                let ord = compare_nested(&nested_key(&nested.plan, text, false)?, &nested.bound);
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

/// One [`Expr`] resolved against one `COPY` block: the same tree, with each
/// leaf replaced by the [`ResolvedTerm`] that block's schema produced. What a
/// block's `Active` state carries, and evaluable on its own.
#[derive(Debug, Clone)]
pub(crate) enum ResolvedExpr {
    Term(ResolvedTerm),
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
            Self::And(children) | Self::Or(children) => children.iter().any(Self::reads_fields),
            Self::Not(inner) => inner.reads_fields(),
        }
    }

    /// The ordering terms every row this tree keeps must make `True` — the
    /// root when it is one, and each member of a conjunction at the root,
    /// conjunctions nested in it flattened — whose column's plan orders
    /// **exactly**, the only order a block's stored row order is gathered
    /// under ([`BelievedStatistics::bounds`]). A term beneath an `Or` or a
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
                Self::Term(_) | Self::Or(_) | Self::Not(_) => {}
            }
        }
        out
    }

    fn collect_notes(&self, out: &mut Vec<ComparisonNote>) {
        match self {
            Self::Term(term) => out.extend(term.comparison_notes()),
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
    /// field text: no value's key is below `min`'s or above `max`'s.
    ///
    /// **A bound need not be a value the group holds**, and is never read as
    /// one: a bound truncated past a storage cap is read exactly as an exact
    /// one is, so it only has to be on the right side.
    fn bounds(&self, column: usize) -> Option<(&str, &str)>;

    /// Every distinct text among `column`'s non-NULL values, as unescaped
    /// field text — complete, or `None`.
    fn dictionary(&self, column: usize) -> Option<impl Iterator<Item = &str>>;
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
        let rows = group.rows();
        let (nulls, values) = match group.null_count(self.index) {
            Some(nulls) => (nulls > 0, nulls < rows),
            None => (rows > 0, rows > 0),
        };
        let mut set = TruthSet::EMPTY;
        if nulls {
            set = set.union(TruthSet::of(self.eval_value(None).expect("a NULL always answers")));
        }
        if values {
            set = set.union(self.value_truths(group));
        }
        set
    }

    /// What this term could answer over a **non-NULL** value of `group`.
    ///
    /// The dictionary and the bounds each give a superset of the answer, so
    /// where the term reads both it takes what they agree on.
    fn value_truths(&self, group: &impl GroupStatistics) -> TruthSet {
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
            && let Some((min, max)) = group.bounds(self.index)
        {
            set = set.intersection(self.bounded(kind, literal, min, max));
        }
        set
    }

    /// What `min` and `max` allow this term to answer over a value between
    /// them. A bound that does not key allows anything.
    ///
    /// The four ordering operators are monotone in the value, so each end
    /// settles one truth. **The equality operators only ever rule out
    /// "equal"**, when the literal lies outside the bounds: that no value is
    /// *unequal* would need every value to be the literal, and bounds equal
    /// to it say so of keys, where a canonicalized comparison reads spellings
    /// — one per key only in text `*_out` wrote
    /// (`docs/design/decisions.md`, "D57").
    fn bounded(&self, kind: &CompareKind, literal: &OrderKey, min: &str, max: &str) -> TruthSet {
        let (Some(low), Some(high)) = (order_key(kind, min), order_key(kind, max)) else {
            return TruthSet::TWO_VALUED;
        };
        let low = compare_keys(&low, literal);
        let high = compare_keys(&high, literal);
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
        match self {
            Self::Term(term) => term.truths(group),
            Self::And(children) => children
                .iter()
                .fold(TruthSet::of(Truth::True), |set, child| set.and(child.truths(group))),
            Self::Or(children) => children
                .iter()
                .fold(TruthSet::of(Truth::False), |set, child| set.or(child.truths(group))),
            Self::Not(inner) => inner.truths(group).not(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::datatypes::{DataType, Field, IntervalUnit, Schema, TimeUnit};

    use super::*;
    use crate::pgtype::comparison_for;
    use crate::preamble::{CollationDef, ColumnDef, TypeDef, TypeKind};
    use crate::resolve::ColumnNote;

    /// [`super::resolve_term`] in PostgreSQL's semantics, which every test
    /// but the Arrow-semantics ones asks for.
    fn resolve_term(
        predicate: &Predicate,
        index: usize,
        resolved: &ResolvedSchema,
        header_offset: u64,
    ) -> Result<ResolvedTerm> {
        super::resolve_term(
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
    fn text_term(p: &Predicate, index: usize) -> ResolvedTerm {
        let compared = p.value.as_ref().map(|value| ComparedTerm {
            column: p.column.clone(),
            comparison: Comparison::Canonical(value.clone()),
            declared_type: String::new(),
            divergences: Vec::new(),
            statistics: BelievedStatistics::NONE,
        });
        ResolvedTerm { op: p.op, index, compared: if p.op.is_ordering() { None } else { compared } }
    }

    /// The one note a term announces, or `None` — a scalar column has at most
    /// one diverging position.
    #[track_caller]
    fn only_note(term: &ResolvedTerm) -> Option<ComparisonNote> {
        let notes = term.comparison_notes();
        assert!(notes.len() <= 1, "a scalar column announces at most one note: {notes:?}");
        notes.into_iter().next()
    }

    /// One text term's own truth value over `raw_row`.
    fn truth(p: &Predicate, raw_row: &[u8], index: usize) -> Truth {
        text_term(p, index)
            .eval(RawRow::unchecked(raw_row), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
    }

    /// Whether a row survives a conjunction of text terms.
    fn matches_all(filters: &[Predicate], indices: &[usize], raw_row: &[u8]) -> bool {
        let expr = ResolvedExpr::And(
            filters
                .iter()
                .zip(indices)
                .map(|(p, &i)| ResolvedExpr::Term(text_term(p, i)))
                .collect(),
        );
        expr.matches(RawRow::unchecked(raw_row), &mut RowSplit::default(), "public.t", 0).unwrap()
    }

    /// The type list every one-column schema below resolves against: one
    /// enum, so a column declared `public.mood` reaches the register's enum
    /// arm rather than its "no such type" one.
    fn test_types() -> Vec<TypeDef> {
        vec![
            TypeDef {
                name: "public.mood".into(),
                kind: TypeKind::Enum { labels: vec!["sad".into(), "ok".into()] },
            },
            // A range whose DDL stated no `subtype` — the one shape the
            // resolver calls nested and the register refuses outright, there
            // being no bound type to name the refusal after.
            TypeDef {
                name: "public.opaquerange".into(),
                kind: TypeKind::Range {
                    subtype: None,
                    multirange_type_name: None,
                    canonical: None,
                },
            },
        ]
    }

    /// A one-column `ResolvedSchema` for `declared`/`data_type`, mapped and
    /// scalar — the shape an ordering term is allowed on.
    ///
    /// The comparison plan comes from the register itself rather than being
    /// stated here, so these tests exercise the same `declared -> plan` walk
    /// `resolve_columns` makes.
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
            comparisons: vec![comparison_for(declared, None, &test_types(), &[])],
        }
    }

    /// A one-column schema for a **nested** declared type, resolved against
    /// `types` so the Arrow type, the nested plan and the comparison plan
    /// agree the way they do in a real query.
    fn nested_column(declared: &str, types: &[TypeDef]) -> ResolvedSchema {
        let crate::pgtype::TypeOutcome::Mapped(data_type, plan) =
            crate::pgtype::resolve_declared_type(declared, types)
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
        }
    }

    /// One nested term's verdict over a single-field row holding `field`.
    #[track_caller]
    fn nested_verdict(
        declared: &str,
        types: &[TypeDef],
        op: PredicateOp,
        field: &str,
        literal: &str,
    ) -> Result<Truth> {
        let p = Predicate { column: "v".into(), op, value: Some(literal.into()) };
        resolve_term(&p, 0, &nested_column(declared, types), 0)?.eval(
            RawRow::unchecked(field.as_bytes()),
            &mut RowSplit::default(),
            "public.t",
            0,
        )
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
        Ok(term
            .eval(RawRow::unchecked(field.as_bytes()), &mut RowSplit::default(), "public.t", 0)?
            .is_true())
    }

    #[test]
    fn eq_matches_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a\tb".into()) };
        assert_eq!(truth(&p, b"other\ta\\tb", 1), Truth::True);
        assert_eq!(truth(&p, b"other\tc", 1), Truth::False);
    }

    #[test]
    fn ne_matches_everything_but_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert_eq!(truth(&p, b"other\tb", 1), Truth::True);
        assert_eq!(truth(&p, b"other\ta", 1), Truth::False);
    }

    /// A NULL field is *unknown* under `=` and `!=`, not false, and a row is
    /// kept only where the root is true.
    #[test]
    fn null_is_unknown_under_eq_and_ne() {
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
        let ne = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert_eq!(truth(&eq, b"other\t\\N", 1), Truth::Unknown);
        assert_eq!(truth(&ne, b"other\t\\N", 1), Truth::Unknown);
        assert!(!matches_all(&[eq, ne], &[1, 1], b"other\t\\N"));
    }

    /// `IS DISTINCT FROM` is `!=` with NULL counted as a value, which is the
    /// one thing `NOT` cannot express: `NOT (x = a)` drops a NULL row.
    #[test]
    fn is_distinct_from_counts_null_as_a_value() {
        let idf = Predicate {
            column: "x".into(),
            op: PredicateOp::IsDistinctFrom,
            value: Some("a".into()),
        };
        let indf = Predicate {
            column: "x".into(),
            op: PredicateOp::IsNotDistinctFrom,
            value: Some("a".into()),
        };
        assert_eq!(truth(&idf, b"other\t\\N", 1), Truth::True);
        assert_eq!(truth(&idf, b"other\tb", 1), Truth::True);
        assert_eq!(truth(&idf, b"other\ta", 1), Truth::False);
        assert_eq!(truth(&indf, b"other\t\\N", 1), Truth::False);
        assert_eq!(truth(&indf, b"other\tb", 1), Truth::False);
        assert_eq!(truth(&indf, b"other\ta", 1), Truth::True);

        // The half `NOT` gets wrong, stated as the pair it is.
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
        let negated = ResolvedExpr::Not(Box::new(ResolvedExpr::Term(text_term(&eq, 1))));
        assert!(
            !negated
                .matches(RawRow::unchecked(b"other\t\\N"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        assert!(
            ResolvedExpr::Term(text_term(&idf, 1))
                .matches(RawRow::unchecked(b"other\t\\N"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
    }

    #[test]
    fn is_null_and_is_not_null() {
        let is_null = Predicate { column: "x".into(), op: PredicateOp::IsNull, value: None };
        let is_not_null = Predicate { column: "x".into(), op: PredicateOp::IsNotNull, value: None };
        assert_eq!(truth(&is_null, b"other\t\\N", 1), Truth::True);
        assert_eq!(truth(&is_null, b"other\ta", 1), Truth::False);
        assert_eq!(truth(&is_not_null, b"other\t\\N", 1), Truth::False);
        assert_eq!(truth(&is_not_null, b"other\ta", 1), Truth::True);
    }

    #[test]
    fn an_empty_conjunction_matches_every_row() {
        assert!(matches_all(&[], &[], b"a\tb"));
        // And the default filter *is* that conjunction.
        assert!(matches!(Expr::default(), Expr::And(ref children) if children.is_empty()));
    }

    #[test]
    fn every_term_must_match() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Eq, value: Some("1".into()) },
            Predicate { column: "b".into(), op: PredicateOp::Eq, value: Some("2".into()) },
        ];
        assert!(matches_all(&filters, &[0, 1], b"1\t2"));
        assert!(!matches_all(&filters, &[0, 1], b"1\t3"));
        assert!(!matches_all(&filters, &[0, 1], b"9\t2"));
    }

    /// Two terms on one column are an ordinary conjunction, and a
    /// contradictory pair matches nothing — no term is special-cased.
    #[test]
    fn two_terms_may_name_the_same_column() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("1".into()) },
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("2".into()) },
        ];
        assert!(matches_all(&filters, &[0, 0], b"3"));
        assert!(!matches_all(&filters, &[0, 0], b"2"));
    }

    /// The row every tree test below is evaluated over: field 0 holds `1` and
    /// field 1 is NULL, so a leaf of each truth value is built out of real
    /// terms rather than out of a constant.
    const THREE_VALUED_ROW: &[u8] = b"1\t\\N";

    /// A leaf that evaluates to `want` over [`THREE_VALUED_ROW`]: `f0 = 1`,
    /// `f0 = 2`, and `f1 = 1` against the NULL field.
    fn leaf(want: Truth) -> ResolvedExpr {
        let (index, value) = match want {
            Truth::True => (0, "1"),
            Truth::False => (0, "2"),
            Truth::Unknown => (1, "1"),
        };
        let p = Predicate { column: "v".into(), op: PredicateOp::Eq, value: Some(value.into()) };
        ResolvedExpr::Term(text_term(&p, index))
    }

    fn exact(expr: &ResolvedExpr) -> Truth {
        expr.eval(
            true,
            RawRow::unchecked(THREE_VALUED_ROW),
            &mut RowSplit::default(),
            "public.t",
            0,
        )
        .unwrap()
    }

    const VALUES: [Truth; 3] = [Truth::True, Truth::False, Truth::Unknown];

    /// Kleene's tables, written out rather than derived, over trees whose
    /// leaves are real terms.
    #[test]
    fn and_or_and_not_are_three_valued() {
        use Truth::{False, True, Unknown};

        assert_eq!(exact(&ResolvedExpr::And(vec![])), True, "an empty conjunction is true");
        assert_eq!(exact(&ResolvedExpr::Or(vec![])), False, "an empty disjunction is false");

        for a in VALUES {
            assert_eq!(exact(&ResolvedExpr::Not(Box::new(leaf(a)))), a.not());
            for b in VALUES {
                let pair = || vec![leaf(a), leaf(b)];
                let and = match (a, b) {
                    (False, _) | (_, False) => False,
                    (Unknown, _) | (_, Unknown) => Unknown,
                    _ => True,
                };
                let or = match (a, b) {
                    (True, _) | (_, True) => True,
                    (Unknown, _) | (_, Unknown) => Unknown,
                    _ => False,
                };
                assert_eq!(exact(&ResolvedExpr::And(pair())), and, "{a:?} AND {b:?}");
                assert_eq!(exact(&ResolvedExpr::Or(pair())), or, "{a:?} OR {b:?}");
            }
        }
    }

    /// Every tree of height at most three over the three truth values, with
    /// `And`/`Or` arity two.
    fn trees(height: usize) -> Vec<ResolvedExpr> {
        let mut out: Vec<ResolvedExpr> = VALUES.iter().copied().map(leaf).collect();
        if height == 0 {
            return out;
        }
        let sub = trees(height - 1);
        for a in &sub {
            out.push(ResolvedExpr::Not(Box::new(a.clone())));
            for b in &sub {
                out.push(ResolvedExpr::And(vec![a.clone(), b.clone()]));
                out.push(ResolvedExpr::Or(vec![a.clone(), b.clone()]));
            }
        }
        out
    }

    /// The short-circuit is verdict-preserving: `matches` evaluates the root
    /// inexactly — an `And` may report `False` for an `Unknown` and stop —
    /// and nothing above the root tells the two apart. Asserted over every
    /// tree of height three.
    #[test]
    fn the_root_verdict_survives_the_short_circuit() {
        let all = trees(2);
        assert!(all.len() > 1000, "only {} trees", all.len());
        for expr in &all {
            assert_eq!(
                expr.matches(
                    RawRow::unchecked(THREE_VALUED_ROW),
                    &mut RowSplit::default(),
                    "public.t",
                    0
                )
                .unwrap(),
                exact(expr).is_true(),
                "{expr:?}"
            );
        }
    }

    /// Every [`TruthSet`], the empty one included.
    fn all_sets() -> impl Iterator<Item = TruthSet> {
        (0..8).map(TruthSet)
    }

    /// **The set operations are Kleene's tables lifted**, asserted against
    /// the row evaluator itself rather than against a second copy of the
    /// tables: over every pair of sets, `A AND B` is exactly what `a AND b`
    /// evaluates to over real terms for some `a` in `A` and `b` in `B`, and so
    /// for `OR` and `NOT`.
    #[test]
    fn truth_sets_combine_as_their_members_do() {
        for a in all_sets() {
            let negated = a.members().map(|x| exact(&ResolvedExpr::Not(Box::new(leaf(x)))));
            assert_eq!(a.not(), negated.fold(TruthSet::EMPTY, |s, t| s.union(TruthSet::of(t))));
            for b in all_sets() {
                let (mut and, mut or) = (TruthSet::EMPTY, TruthSet::EMPTY);
                for x in a.members() {
                    for y in b.members() {
                        let pair = || vec![leaf(x), leaf(y)];
                        and = and.union(TruthSet::of(exact(&ResolvedExpr::And(pair()))));
                        or = or.union(TruthSet::of(exact(&ResolvedExpr::Or(pair()))));
                    }
                }
                assert_eq!(a.and(b), and, "{a:?} AND {b:?}");
                assert_eq!(a.or(b), or, "{a:?} OR {b:?}");
            }
        }
    }

    /// One column's statistics for one row group, stated by hand — the
    /// shape [`GroupStatistics`] reads, with every column index answered
    /// alike.
    #[derive(Debug, Clone, Default)]
    struct Group {
        rows: u64,
        nulls: Option<u64>,
        bounds: Option<(String, String)>,
        dictionary: Option<Vec<String>>,
    }

    impl GroupStatistics for Group {
        fn rows(&self) -> u64 {
            self.rows
        }
        fn null_count(&self, _: usize) -> Option<u64> {
            self.nulls
        }
        fn bounds(&self, _: usize) -> Option<(&str, &str)> {
            self.bounds.as_ref().map(|(min, max)| (min.as_str(), max.as_str()))
        }
        fn dictionary(&self, _: usize) -> Option<impl Iterator<Item = &str>> {
            self.dictionary.as_ref().map(|entries| entries.iter().map(String::as_str))
        }
    }

    fn sets(truths: &[Truth]) -> TruthSet {
        truths.iter().fold(TruthSet::EMPTY, |set, t| set.union(TruthSet::of(*t)))
    }

    /// What a group's statistics let each kind of term say, one case per
    /// rule, by hand: a column the statistics bound, a group with NULLs, a
    /// `NOT` over both, the equality operators' one-sided reading of bounds,
    /// a dictionary a decoded kind reads by key, and statistics a term's
    /// comparison does not share.
    #[test]
    fn a_group_is_answered_from_its_statistics() {
        use Truth::{False, True, Unknown};
        let term = |declared: &str, op, literal: Option<&str>| {
            let p = Predicate { column: "v".into(), op, value: literal.map(str::to_string) };
            ResolvedExpr::Term(
                resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap(),
            )
        };
        let bounded = |min: &str, max: &str, nulls: u64| Group {
            rows: 4,
            nulls: Some(nulls),
            bounds: Some((min.into(), max.into())),
            dictionary: None,
        };
        let listed = |entries: &[&str]| Group {
            rows: 4,
            nulls: Some(0),
            bounds: None,
            dictionary: Some(entries.iter().map(|e| e.to_string()).collect()),
        };
        let lt5 = term("integer", PredicateOp::Lt, Some("5"));
        let not_lt5 = ResolvedExpr::Not(Box::new(lt5.clone()));
        let cases: Vec<(&str, &ResolvedExpr, Group, TruthSet)> = vec![
            ("above the literal", &lt5, bounded("6", "9", 0), sets(&[False])),
            ("straddling it", &lt5, bounded("4", "9", 0), sets(&[True, False])),
            ("with NULLs", &lt5, bounded("6", "9", 1), sets(&[False, Unknown])),
            // A "may match" flag would call this group empty.
            ("negated", &not_lt5, bounded("6", "9", 1), sets(&[True, Unknown])),
            ("no rows", &lt5, Group { rows: 0, nulls: Some(0), ..Group::default() }, sets(&[])),
            (
                "no statistics",
                &lt5,
                Group { rows: 4, ..Group::default() },
                sets(&[True, False, Unknown]),
            ),
            (
                "only NULLs",
                &lt5,
                Group { rows: 4, nulls: Some(4), ..Group::default() },
                sets(&[Unknown]),
            ),
        ];
        let eq5 = term("integer", PredicateOp::Eq, Some("5"));
        let ne5 = term("integer", PredicateOp::Ne, Some("5"));
        let distinct = term("integer", PredicateOp::IsDistinctFrom, Some("5"));
        let is_null = term("integer", PredicateOp::IsNull, None);
        let decimal = term("numeric", PredicateOp::Eq, Some("1.5"));
        let unknown_collation_lt = term("text", PredicateOp::Lt, Some("b"));
        let unknown_collation_eq = term("text", PredicateOp::Eq, Some("b"));
        let more: Vec<(&str, &ResolvedExpr, Group, TruthSet)> = vec![
            ("= outside the bounds", &eq5, bounded("6", "9", 0), sets(&[False])),
            // Bounds equal to the literal say nothing about spellings.
            ("!= on bounds equal to it", &ne5, bounded("5", "5", 0), sets(&[True, False])),
            ("!= on a dictionary of it", &ne5, listed(&["5"]), sets(&[False])),
            ("a dictionary without it", &eq5, listed(&["6", "7"]), sets(&[False])),
            (
                "both, agreeing",
                &eq5,
                Group { bounds: Some(("1".into(), "9".into())), ..listed(&["6"]) },
                sets(&[False]),
            ),
            (
                "DISTINCT over NULLs",
                &distinct,
                Group { rows: 2, nulls: Some(2), ..Group::default() },
                sets(&[True]),
            ),
            (
                "IS NULL with none",
                &is_null,
                Group { rows: 2, nulls: Some(0), ..Group::default() },
                sets(&[False]),
            ),
            ("a decoded kind's entry", &decimal, listed(&["1.50"]), sets(&[True])),
            (
                "bounds under an unknown collation",
                &unknown_collation_lt,
                bounded("c", "d", 0),
                sets(&[True, False]),
            ),
            ("its dictionary", &unknown_collation_eq, listed(&["a"]), sets(&[False])),
        ];
        for (case, expr, group, want) in cases.into_iter().chain(more) {
            assert_eq!(expr.truths(&group), want, "{case}");
        }
    }

    /// A two-column schema, both `integer`, for the tests about *where* a
    /// decode failure surfaces.
    fn two_integers() -> ResolvedSchema {
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
        }
    }

    /// A decode failure surfaces only where evaluation reaches it, so which
    /// rows error depends on where the term sits in the tree — and on whether
    /// a `Not` sits above it, a `Not` being the one node that has to tell
    /// `False` from `Unknown` and so cannot short-circuit an unknown away.
    #[test]
    fn a_decode_failure_surfaces_only_where_it_is_reached() {
        let schema = two_integers();
        let term = |column: &str, op, value: &str, index| {
            let p = Predicate { column: column.into(), op, value: Some(value.into()) };
            ResolvedExpr::Term(resolve_term(&p, index, &schema, 0).unwrap())
        };
        // `b` holds text no `integer` decoder will read, so any term over it
        // is `Error::FieldDecode` the moment it is evaluated.
        let row: &[u8] = b"1\tnope";
        let corrupt = || term("b", PredicateOp::Gt, "0", 1);
        let ok = |value| term("a", PredicateOp::Eq, value, 0);

        assert!(
            corrupt()
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .is_err()
        );
        // Settled by a field that did decode, in both directions.
        assert!(
            ResolvedExpr::Or(vec![ok("1"), corrupt()])
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        assert!(
            !ResolvedExpr::And(vec![ok("2"), corrupt()])
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        // Not settled: the walk reaches the corrupt field.
        assert!(
            ResolvedExpr::And(vec![ok("1"), corrupt()])
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .is_err()
        );

        // An unknown conjunct settles the *root*, so the walk stops there
        // until a `Not` above it makes `Unknown` and `False` different
        // answers. Field 2 does not exist in this row, so the leading term
        // reads NULL and is unknown.
        let unknown = || {
            let p = Predicate { column: "c".into(), op: PredicateOp::Eq, value: Some("x".into()) };
            ResolvedExpr::And(vec![ResolvedExpr::Term(text_term(&p, 2)), corrupt()])
        };
        assert!(
            !unknown()
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        assert!(
            ResolvedExpr::Not(Box::new(unknown()))
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .is_err()
        );
    }

    #[test]
    fn missing_column_index_is_treated_as_null() {
        // Can't happen once a caller resolves the index from the block's own
        // schema; the fallback is exercised anyway.
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert_eq!(truth(&p, b"onlyone", 5), Truth::Unknown);
    }

    /// The four operators over the boundary itself — the case a `<` / `<=`
    /// pair differs on.
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

    /// Numbers order as numbers: the text comparison `Eq` uses would put `9`
    /// after `10`.
    #[test]
    fn integers_order_numerically_not_lexicographically() {
        assert!(ordered("integer", DataType::Int32, PredicateOp::Gt, "9", "10").unwrap());
        assert!(!ordered("integer", DataType::Int32, PredicateOp::Lt, "9", "10").unwrap());
    }

    /// An `oid` is unsigned across its whole range: `4294967295` is above
    /// `2147483648`, which an `Int32` reading would make two negative
    /// numbers (I39).
    #[test]
    fn an_oid_orders_over_the_whole_unsigned_range() {
        let oid = |op, value, field| ordered("oid", DataType::UInt32, op, value, field).unwrap();
        assert!(oid(PredicateOp::Gt, "2147483648", "4294967295"));
        assert!(oid(PredicateOp::Lt, "2147483648", "0"));
        assert!(oid(PredicateOp::Ge, "4294967295", "4294967295"));
    }

    /// `oidin` reads a signed literal by wrapping it — `-1` is 4294967295 —
    /// and this build does not implement that, so the literal is refused
    /// rather than read as −1.
    #[test]
    fn a_signed_oid_literal_is_refused_rather_than_wrapped() {
        let p = order_predicate(PredicateOp::Lt, "-1");
        let err = resolve_term(&p, 0, &one_column("oid", DataType::UInt32), 0).unwrap_err();
        assert!(
            matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v"),
            "{err:?}"
        );
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
    /// rounded: the column's own decoder does not drop non-zero digits.
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

    /// A NULL field is excluded by every ordering operator, as under
    /// `Eq`/`Ne`.
    #[test]
    fn a_null_field_matches_no_ordering_operator() {
        for op in [PredicateOp::Lt, PredicateOp::Le, PredicateOp::Gt, PredicateOp::Ge] {
            assert!(!ordered("integer", DataType::Int32, op, "0", "\\N").unwrap());
        }
    }

    /// The three special values are ordered exactly, on whichever side they
    /// appear: `-infinity` below every finite value, `infinity` above it, and
    /// each equal to itself (I34).
    #[test]
    fn date_and_timestamp_infinities_are_ordered() {
        let date = |op, value, field| ordered("date", DataType::Date32, op, value, field).unwrap();
        assert!(date(PredicateOp::Gt, "2020-01-01", "infinity"));
        assert!(!date(PredicateOp::Lt, "2020-01-01", "infinity"));
        assert!(date(PredicateOp::Lt, "0044-01-01 BC", "-infinity"));
        assert!(date(PredicateOp::Lt, "infinity", "-infinity"));
        // Each special equals itself, so the boundary pair splits on it.
        assert!(date(PredicateOp::Ge, "infinity", "infinity"));
        assert!(!date(PredicateOp::Gt, "infinity", "infinity"));
        assert!(date(PredicateOp::Le, "-infinity", "-infinity"));

        let ts = |op, value, field| {
            ordered(
                "timestamp without time zone",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                op,
                value,
                field,
            )
            .unwrap()
        };
        assert!(ts(PredicateOp::Gt, "2020-01-01 00:00:00", "infinity"));
        assert!(ts(PredicateOp::Lt, "2020-01-01 00:00:00", "-infinity"));
        let tstz = ordered(
            "timestamp with time zone",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            PredicateOp::Gt,
            "2020-01-01 00:00:00+00",
            "infinity",
        );
        assert!(tstz.unwrap());
    }

    /// A `numeric(p,s)` column can hold `NaN` — the typmod does not apply to
    /// it — and PostgreSQL orders it above every other value, itself included
    /// (I34). An *infinity* cannot reach this kind at all: any typmod rejects
    /// one, and a `numeric` without a typmod is held as text.
    #[test]
    fn a_decimal_nan_is_the_largest_value_and_equals_itself() {
        let t = DataType::Decimal128(10, 2);
        let nan = |op, value, field| ordered("numeric(10,2)", t.clone(), op, value, field).unwrap();
        assert!(nan(PredicateOp::Gt, "999999.99", "NaN"));
        assert!(nan(PredicateOp::Gt, "-1.50", "NaN"));
        assert!(nan(PredicateOp::Ge, "NaN", "NaN"));
        assert!(!nan(PredicateOp::Gt, "NaN", "NaN"));
        assert!(nan(PredicateOp::Lt, "NaN", "0.00"));
    }

    /// The spelling is the one that type's own `*_out` writes and nothing
    /// else, so a `date` reading `Infinity` is undecodable on either side. A
    /// text column holding the word is unaffected.
    #[test]
    fn only_the_types_own_spelling_is_special() {
        let err = ordered("date", DataType::Date32, PredicateOp::Gt, "Infinity", "2020-01-01")
            .unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
        let err = ordered("date", DataType::Date32, PredicateOp::Gt, "2020-01-01", "Infinity")
            .unwrap_err();
        assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");

        // A `text` column holding `infinity` holds the *word*: it compares
        // bytewise, below `zzz`, where a special value would sort above it.
        let text = |op, field| ordered("text", DataType::Utf8View, op, "zzz", field).unwrap();
        assert!(text(PredicateOp::Lt, "infinity"));
        assert!(!text(PredicateOp::Gt, "infinity"));
    }

    /// A field that is not a value of its mapped type is the same fault the
    /// typed build path reports, with the same wording and the same escape —
    /// the population the special values are separated *from*.
    #[test]
    fn an_undecodable_field_is_a_field_decode_error() {
        let err = ordered("integer", DataType::Int32, PredicateOp::Gt, "0", "twelve").unwrap_err();
        assert!(
            matches!(err, Error::FieldDecode { ref column, ref value, .. }
                if column == "v" && value == "twelve"),
            "{err:?}"
        );
    }

    /// **A value is answered with no row around it, and the answer is the
    /// row's** — every [`Comparison`] arm and both NULL-counting families,
    /// over a NULL, values either side of the literal, and a value that is
    /// not of the column's type. The one thing a value cannot carry is where
    /// it came from, so the row path's `FieldDecode` is the value path's
    /// `None`, and nothing else is.
    #[test]
    fn a_value_outside_any_row_is_answered_as_the_row_holding_it_is() {
        use crate::copy::encode_field;
        let scalar = |declared: &str, op, literal: &str| {
            let schema = one_column(declared, DataType::Utf8View);
            resolve_term(&order_predicate(op, literal), 0, &schema, 0).unwrap()
        };
        let terms = [
            ("ordered", scalar("integer", PredicateOp::Le, "10")),
            ("canonical", scalar("integer", PredicateOp::Eq, "10")),
            ("trimmed", scalar("character(4)", PredicateOp::Ne, "10")),
            ("decoded", scalar("numeric", PredicateOp::IsNotDistinctFrom, "10")),
            ("distinct", scalar("integer", PredicateOp::IsDistinctFrom, "10")),
            (
                "null test",
                resolve_term(
                    &Predicate { column: "v".into(), op: PredicateOp::IsNull, value: None },
                    0,
                    &one_column("integer", DataType::Int32),
                    0,
                )
                .unwrap(),
            ),
            (
                "nested",
                resolve_term(
                    &order_predicate(PredicateOp::Lt, "{10}"),
                    0,
                    &nested_column("integer[]", &[]),
                    0,
                )
                .unwrap(),
            ),
        ];
        let values =
            [None, Some("9"), Some("10"), Some("10.0"), Some("10  "), Some("{9}"), Some("x")];
        let mut undecodable = 0;
        for (arm, term) in &terms {
            for value in values {
                let row = encode_field(value);
                let by_row =
                    term.eval(RawRow::unchecked(&row), &mut RowSplit::default(), "public.t", 0);
                match (term.eval_value(value), by_row) {
                    (Some(answer), Ok(by_row)) => assert_eq!(answer, by_row, "{arm} {value:?}"),
                    (None, Err(Error::FieldDecode { .. })) => undecodable += 1,
                    (answer, by_row) => panic!("{arm} {value:?}: {answer:?} against {by_row:?}"),
                }
            }
        }
        // The two decoding arms refuse `x` and the nested one every scalar
        // spelling, so the `None` half is reached rather than vacuous.
        assert!(undecodable >= 3, "only {undecodable} values refused");
    }

    /// A literal that is not a value of the column's type is refused when the
    /// block's schema resolves, once rather than per row.
    #[test]
    fn an_undecodable_literal_is_refused_at_resolution() {
        let p = order_predicate(PredicateOp::Gt, "twelve");
        let err = resolve_term(&p, 0, &one_column("integer", DataType::Int32), 0).unwrap_err();
        assert!(
            matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v"),
            "{err:?}"
        );
    }

    /// Refusal is by resolution and plan, and the four reasons are distinct
    /// facts about the column.
    #[test]
    fn an_ordering_operator_is_refused_off_a_mapped_scalar_column() {
        let p = order_predicate(PredicateOp::Gt, "1");

        let mut unmapped = one_column("mystery", DataType::Utf8View);
        unmapped.columns[0] = ColumnResolution::UnknownType;
        assert!(matches!(
            resolve_term(&p, 0, &unmapped, 7).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, header_offset: 7, .. } if reason == NOT_MAPPED
        ));

        // A nested column the register compares not at all — a range whose
        // DDL stated no subtype, so there is no bound type to compare by.
        let mut nested = one_column("public.opaquerange", DataType::Utf8View);
        nested.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
        assert!(matches!(
            resolve_term(&p, 0, &nested, 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NESTED
        ));

        // A nested column whose *shape* compares here and one of whose
        // positions does not: refused too, and the sentence names the
        // position and the type rather than the nesting.
        let mut inherited = one_column("json[]", DataType::Utf8View);
        inherited.plans[0] = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        let err = resolve_term(&p, 0, &inherited, 0).unwrap_err();
        let Error::UnorderedPredicateColumn { reason, .. } = &err else { panic!("{err:?}") };
        assert!(reason.contains("`[]` inside it is `json`"), "{reason}");

        // A `Mapped` scalar column whose *declared* type the register
        // refuses: unreachable from the mapping table, everything it maps to
        // a scalar having a comparison in the same arm, and refused anyway.
        assert!(matches!(
            resolve_term(&p, 0, &one_column("mystery", DataType::UInt8), 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NO_ORDER
        ));
    }

    /// Under Arrow's semantics every comparing operator on a nested column is
    /// refused — `=` too, which PostgreSQL's answers structurally or as text
    /// — and a column with no plan still answers `=` bytewise, which is
    /// Arrow's `=` over the `Utf8View` it emits, announcing nothing. Its
    /// ordering is `arrow_semantics_orders_a_column_with_no_plan_bytewise`'s.
    #[test]
    fn arrow_semantics_refuses_a_nested_column_under_every_comparing_operator() {
        let arrow = |p: &Predicate, resolved: &ResolvedSchema| {
            super::resolve_term(p, 0, resolved, 0, ComparisonSemantics::Arrow)
        };
        let mut opaque = one_column("public.opaquerange", DataType::Utf8View);
        opaque.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
        let array = nested_column("integer[]", &[]);
        let mut inherited = one_column("json[]", DataType::Utf8View);
        inherited.plans[0] = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        for resolved in [&opaque, &array, &inherited] {
            for op in [PredicateOp::Eq, PredicateOp::IsDistinctFrom, PredicateOp::Lt] {
                let err = arrow(&order_predicate(op, "{}"), resolved).unwrap_err();
                assert!(
                    matches!(&err, Error::UncomparablePredicateColumn { reason, .. }
                        if reason == NESTED_IN_ARROW),
                    "{err:?}"
                );
            }
            assert!(arrow(&order_predicate(PredicateOp::IsNull, "{}"), resolved).is_ok());
        }
        let mut unknown = one_column("box", DataType::Utf8View);
        unknown.columns[0] = ColumnResolution::UnknownType;
        let term = arrow(&order_predicate(PredicateOp::Eq, "(1,1),(0,0)"), &unknown).unwrap();
        assert!(term.comparison_notes().is_empty());
        assert_eq!(term.eval_value(Some("(1,1),(0,0)")), Some(Truth::True));
    }

    /// **A column's divergences depend on the semantics asked for**, and
    /// Arrow's are read off what the column emits rather than off which terms
    /// this build answers: a nested column refused every Arrow term still
    /// reports DataFusion's order of it, and each position inside it its own
    /// divergence under its path — `numeric[]` its element's text, `text[]`
    /// its element's collation, `real[]` its element's unnormalized zero; an
    /// enum reports its label text; a float column reports nothing in either.
    /// A column that fell back to text reports nothing in Arrow's semantics,
    /// its column note being the finding. One schema, so the order of the
    /// report is the columns'.
    #[test]
    fn a_column_s_divergences_follow_the_semantics_asked_for() {
        use ComparisonDivergence as D;
        let columns = [
            one_column("real", DataType::Float32),
            one_column("text", DataType::Utf8View),
            one_column("interval", DataType::Interval(IntervalUnit::MonthDayNano)),
            nested_column("integer[]", &[]),
            {
                let mut undeclared = one_column("text", DataType::Utf8View);
                undeclared.columns[0] = ColumnResolution::NotDeclared;
                undeclared.notes[0].declared = None;
                undeclared.comparisons[0] = ComparisonPlan::Refused;
                undeclared
            },
            {
                let mut unknown = one_column("box", DataType::Utf8View);
                unknown.columns[0] = ColumnResolution::UnknownType;
                unknown.comparisons[0] = ComparisonPlan::Refused;
                unknown
            },
            nested_column("numeric[]", &[]),
            nested_column("text[]", &[]),
            nested_column("real[]", &[]),
        ];
        let mut resolved = ResolvedSchema::default();
        for (i, c) in columns.into_iter().enumerate() {
            resolved.columns.extend(c.columns);
            resolved
                .notes
                .extend(c.notes.into_iter().map(|n| ColumnNote { column: format!("c{i}"), ..n }));
            resolved.plans.extend(c.plans);
            resolved.comparisons.extend(c.comparisons);
        }
        let report = |semantics| {
            column_divergences(&resolved, semantics)
                .into_iter()
                .map(|n| (n.column, n.path, n.divergence))
                .collect::<Vec<_>>()
        };
        let c = |i: usize| format!("c{i}");
        let element = || Some("[]".to_string());
        assert_eq!(
            report(ComparisonSemantics::Postgres),
            [
                (c(1), None, D::UnknownCollation),
                (c(5), None, D::UnmodelledType),
                (c(7), element(), D::UnknownCollation),
            ]
        );
        assert_eq!(
            report(ComparisonSemantics::Arrow),
            [
                (c(1), None, D::UnknownCollation),
                (c(2), None, D::IntervalFields),
                (c(3), None, D::NestedArrowOrder),
                (c(6), None, D::NestedArrowOrder),
                (c(6), element(), D::ValueAsText),
                (c(7), None, D::NestedArrowOrder),
                (c(7), element(), D::UnknownCollation),
                (c(8), None, D::NestedArrowOrder),
                (c(8), element(), D::UnnormalizedZero),
            ]
        );
        let notes = column_divergences(&resolved, ComparisonSemantics::Arrow);
        let order = notes.iter().find(|n| n.column == c(3)).unwrap();
        assert!(!order.divergence.affects_equality());
        assert!(
            order.message().starts_with("`c3` (integer[]) is ordered as DataFusion orders"),
            "{}",
            order.message()
        );
        let zero = notes.iter().find(|n| n.divergence == D::UnnormalizedZero).unwrap();
        assert!(zero.divergence.affects_equality());
        assert!(zero.message().starts_with("`c8[]` (real) is compared"), "{}", zero.message());
    }

    /// **A text-emitted kind's equality warning is about a literal's
    /// spelling**: DataFusion compares a literal bytewise with the emitted
    /// text, so a `timetz` literal the server reads as the value it wrote
    /// matches nothing in Arrow's semantics where PostgreSQL's matches it.
    /// The column says so, and names such a spelling.
    #[test]
    fn a_literal_not_spelled_as_the_server_writes_misses_in_arrow_semantics() {
        let resolved = one_column("time with time zone", DataType::Utf8View);
        let arrow = |literal| {
            let p = order_predicate(PredicateOp::Eq, literal);
            super::resolve_term(&p, 0, &resolved, 0, ComparisonSemantics::Arrow).unwrap()
        };
        assert_eq!(arrow("12:00+00").eval_value(Some("12:00:00+00")), Some(Truth::False));
        // One this build's PostgreSQL semantics also reads, answering as the
        // server does.
        let p = order_predicate(PredicateOp::Eq, "12:00:00.0+00");
        let postgres = resolve_term(&p, 0, &resolved, 0).unwrap();
        assert_eq!(postgres.eval_value(Some("12:00:00+00")), Some(Truth::True));
        assert_eq!(arrow("12:00:00.0+00").eval_value(Some("12:00:00+00")), Some(Truth::False));
        let [note] = &column_divergences(&resolved, ComparisonSemantics::Arrow)[..] else {
            panic!("one note")
        };
        assert_eq!(note.divergence, ComparisonDivergence::ValueAsText);
        assert!(note.divergence.affects_equality());
        assert!(note.message().contains("`'12:00+00'` misses `12:00:00+00`"), "{}", note.message());
    }

    /// **A float nested in a list is compared with no `-0` made `0`**, as
    /// `make_comparator` compares it and as [`ComparisonDivergence::UnnormalizedZero`]
    /// says — where a float column's `-0` equals `0` in Arrow's semantics
    /// (DataFusion's `apply_cmp`). Read off the arrays a batch builds.
    #[test]
    fn a_nested_float_s_negative_zero_is_not_zero_to_datafusion() {
        let resolved = nested_column("real[]", &[]);
        let array = crate::batch::column_of(
            resolved.schema.field(0).data_type(),
            &resolved.plans[0],
            &["{-0}", "{0}"],
        )
        .unwrap();
        let cmp = arrow::array::make_comparator(&array, &array, Default::default()).unwrap();
        assert_eq!(cmp(0, 1), Ordering::Less);
    }

    /// **A column with no plan is ordered bytewise in Arrow's semantics**,
    /// under every ordering operator, where PostgreSQL's refuses them: the
    /// column emits the file's text as `Utf8View`, which DataFusion orders by
    /// its bytes. Both roads there — a type this build does not map, and a
    /// column no DDL declared, which is every column under
    /// `--schema-mode strings` — answer alike, announce nothing and read no
    /// statistics.
    #[test]
    fn arrow_semantics_orders_a_column_with_no_plan_bytewise() {
        let mut unknown = one_column("box", DataType::Utf8View);
        unknown.columns[0] = ColumnResolution::UnknownType;
        let mut undeclared = one_column("text", DataType::Utf8View);
        undeclared.columns[0] = ColumnResolution::NotDeclared;
        undeclared.comparisons[0] = ComparisonPlan::Refused;
        // Bytewise, so an upper-case letter is below every lower-case one and
        // a multi-byte character above both.
        let cases = [
            (PredicateOp::Lt, "a", [true, false, false, false]),
            (PredicateOp::Le, "a", [true, true, false, false]),
            (PredicateOp::Gt, "a", [false, false, true, true]),
            (PredicateOp::Ge, "b", [false, false, true, true]),
        ];
        let values = ["Z", "a", "b", "é"];
        for resolved in [&unknown, &undeclared] {
            for (op, literal, want) in cases {
                let p = order_predicate(op, literal);
                assert!(matches!(
                    resolve_term(&p, 0, resolved, 0),
                    Err(Error::UnorderedPredicateColumn { .. })
                ));
                let term =
                    super::resolve_term(&p, 0, resolved, 0, ComparisonSemantics::Arrow).unwrap();
                assert!(term.comparison_notes().is_empty());
                let statistics = &term.compared.as_ref().unwrap().statistics;
                assert!(statistics.bounds.is_none() && !statistics.dictionary);
                for (value, want) in values.iter().zip(want) {
                    assert_eq!(
                        term.eval_value(Some(value)),
                        Some(Truth::of(want)),
                        "{value:?} {} {literal:?}",
                        op.symbol()
                    );
                }
            }
        }
    }

    /// Which statistics a term reads, per semantics: `(bounds under <,
    /// dictionary under =)`. **Arrow's semantics reads bounds only where a
    /// kind keeps its order there** — they were gathered in PostgreSQL's —
    /// and a dictionary wherever its entries are the field's own text, which
    /// `character(n)`'s, stored unpadded, are not.
    #[test]
    fn arrow_semantics_reads_only_the_statistics_that_hold_there() {
        let read = |declared: &str, literal: &str, semantics| {
            let believed = |op| {
                let p = order_predicate(op, literal);
                let resolved = one_column(declared, DataType::Utf8View);
                let term = super::resolve_term(&p, 0, &resolved, 0, semantics).unwrap();
                term.compared.unwrap().statistics
            };
            (believed(PredicateOp::Lt).bounds.is_some(), believed(PredicateOp::Eq).dictionary)
        };
        for (declared, literal, postgres, arrow) in [
            ("integer", "1", (true, true), (true, true)),
            ("uuid", "00000000-0000-0000-0000-000000000000", (true, true), (true, true)),
            ("real", "1", (true, true), (true, true)),
            ("numeric", "1", (true, true), (false, true)),
            ("interval", "1 day", (true, true), (false, true)),
            ("public.mood", "ok", (true, true), (false, true)),
            // On the database's collation, so gathered with no bounds.
            ("text", "a", (false, true), (false, true)),
            ("character(3)", "a", (false, true), (false, false)),
        ] {
            assert_eq!(
                read(declared, literal, ComparisonSemantics::Postgres),
                postgres,
                "{declared}"
            );
            assert_eq!(read(declared, literal, ComparisonSemantics::Arrow), arrow, "{declared}");
        }
    }

    /// A range type declaring a `canonical` function refuses **every**
    /// comparing operator, `=` and `!=` included, through an error of its own
    /// — the one refusal that cannot end by offering the text comparison,
    /// that comparison being what the file says is not the server's.
    ///
    /// The two NULL tests still answer: they read no value and consult no
    /// plan.
    #[test]
    fn a_range_declaring_a_canonical_function_refuses_every_operator() {
        let types = vec![TypeDef {
            name: "public.canonrange".into(),
            kind: TypeKind::Range {
                subtype: Some("integer".into()),
                multirange_type_name: None,
                canonical: Some("public.canonrange_canonical".into()),
            },
        }];
        let schema = nested_column("public.canonrange", &types);
        for op in [
            PredicateOp::Lt,
            PredicateOp::Le,
            PredicateOp::Gt,
            PredicateOp::Ge,
            PredicateOp::Eq,
            PredicateOp::Ne,
            PredicateOp::IsDistinctFrom,
            PredicateOp::IsNotDistinctFrom,
        ] {
            let p = Predicate { column: "v".into(), op, value: Some("[1,10]".into()) };
            let err = resolve_term(&p, 0, &schema, 7).unwrap_err();
            let Error::UncomparablePredicateColumn { reason, header_offset: 7, .. } = &err else {
                panic!("{op:?}: {err:?}")
            };
            // The sentence names the range type and its function, which is
            // what a user needs to go and look at the DDL.
            assert!(reason.contains("`public.canonrange`"), "{op:?}: {reason}");
            assert!(reason.contains("`public.canonrange_canonical`"), "{op:?}: {reason}");
        }
        for op in [PredicateOp::IsNull, PredicateOp::IsNotNull] {
            let p = Predicate { column: "v".into(), op, value: None };
            let term = resolve_term(&p, 0, &schema, 0).expect("a NULL test consults no plan");
            assert!(term.compared.is_none(), "{op:?}");
        }
    }

    /// Only a divergent classification produces a note, and the sentence is
    /// chosen from the declared type rather than the Arrow one.
    #[test]
    fn a_divergent_comparison_produces_a_note_naming_why() {
        let note = |declared, data_type, literal| {
            let p = order_predicate(PredicateOp::Gt, literal);
            only_note(&resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap())
        };

        assert_eq!(note("integer", DataType::Int32, "1"), None, "an agreeing type says nothing");

        let text = note("character varying(10)", DataType::Utf8View, "a").unwrap();
        assert_eq!(text.divergence, ComparisonDivergence::UnknownCollation);
        assert!(text.message().contains("collation"), "{}", text.message());

        // `character(n)` asks the same collation question over its own
        // trimming comparison, so a bare column gets the collation sentence
        // and one declaring `COLLATE "C"` gets no note at all.
        let padded = note("character(10)", DataType::Utf8View, "a").unwrap();
        assert_eq!(padded.divergence, ComparisonDivergence::UnknownCollation);
        assert!(padded.message().contains("collation"), "{}", padded.message());

        // `json` is what `AsText` has left: the server defines no comparison
        // for it at all.
        let other = note("json", DataType::Utf8View, "1").unwrap();
        assert_eq!(other.divergence, ComparisonDivergence::AsText);
        assert!(other.message().contains("no comparison"), "{}", other.message());

        // Types that share `Utf8View` with the rows above and say nothing:
        // a bare `numeric`, an enum, and the three the text-held row lost.
        assert_eq!(note("numeric", DataType::Utf8View, "10"), None);
        for (declared, literal) in [
            ("time with time zone", "00:00:00+00"),
            ("inet", "10.0.0.1"),
            ("macaddr", "08:00:2b:01:02:03"),
        ] {
            assert_eq!(note(declared, DataType::Utf8View, literal), None, "{declared}");
        }
        assert_eq!(
            note(
                "public.mood",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                "sad",
            ),
            None
        );
    }

    /// `jsonb` orders by kind before value, and the kind order is
    /// `JsonbValue`'s own type codes: an object above an array, an array above
    /// every scalar, and a boolean above a number above a string above JSON
    /// `null` (I41). The first three pairs are cells of
    /// `fixtures/16/oracle/comparisons.tsv`; the rest are the probe I41
    /// records, no oracle case carrying a boolean or a string leaf.
    ///
    /// `holds` reads `field <op> literal`, the direction a filter asks in.
    #[test]
    fn jsonb_orders_by_kind_before_value() {
        let holds =
            |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("1", PredicateOp::Gt, "null"));
        assert!(holds("[1, 2]", PredicateOp::Gt, "1"));
        assert!(holds(r#"{"a": 1}"#, PredicateOp::Gt, "[1, 2]"));
        assert!(holds("true", PredicateOp::Gt, "1"));
        assert!(holds("1", PredicateOp::Gt, r#""a""#));
        assert!(holds("false", PredicateOp::Lt, "true"));
        // A bytewise comparison of the same text gets that third one
        // backwards: `{` is below `n`.
        assert!(holds(r#"{"a": 1}"#, PredicateOp::Gt, "null"));
    }

    /// A container is ordered by its **size** before any member of it and
    /// only then member-wise, so a one-pair object sorts below a two-pair one
    /// whatever the keys say.
    #[test]
    fn a_jsonb_container_is_ordered_by_size_first() {
        let holds =
            |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("[1, 2]", PredicateOp::Gt, "[3]"));
        assert!(holds(r#"{"z": 1}"#, PredicateOp::Lt, r#"{"a": 1, "b": 2}"#));
        assert!(holds("[]", PredicateOp::Lt, "[[]]"));
        // Same size: the members decide, keys before values.
        assert!(holds(r#"{"z": 1}"#, PredicateOp::Gt, r#"{"a": 2}"#));
        assert!(holds(r#"{"a": 2}"#, PredicateOp::Gt, r#"{"a": 1}"#));
        // A container element outranks a scalar one at the same position.
        assert!(holds("[[1], 2]", PredicateOp::Gt, "[3, [4]]"));
    }

    /// The pairs of an object are walked in **storage** order — key length
    /// first, then bytes — while the keys themselves are compared as strings.
    /// The two orders disagree here: stored, this is `z` against `y`, and
    /// sorted alphabetically it would be `aa` against `y`, which answers the
    /// other way (I41).
    #[test]
    fn jsonb_object_pairs_are_walked_in_storage_order() {
        assert!(
            ordered(
                "jsonb",
                DataType::Utf8View,
                PredicateOp::Gt,
                r#"{"y": 3, "zz": 4}"#,
                r#"{"z": 1, "aa": 2}"#,
            )
            .unwrap()
        );
    }

    /// A top-level scalar is stored in a one-element pseudo-array, and
    /// `compareJsonbContainers` lets the element count *overwrite* the
    /// `rawScalar` answer — so a scalar sorts below a one- or many-element
    /// array and **above an empty one** (I41).
    #[test]
    fn a_top_level_scalar_outranks_an_empty_array() {
        let holds =
            |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("1", PredicateOp::Gt, "[]"));
        assert!(holds("null", PredicateOp::Gt, "[]"));
        assert!(holds("1", PredicateOp::Lt, "[1]"));
        assert!(holds("1", PredicateOp::Lt, "[1, 2]"));
        // An object is above a scalar whatever its size: the type order
        // decides before any count.
        assert!(holds("1", PredicateOp::Lt, "{}"));
        assert!(holds("[]", PredicateOp::Lt, "{}"));
    }

    /// A `jsonb` number is a `numeric`, so `1`, `1.0` and `1e0` are one value
    /// and `9` is below `10`. Whitespace, key order and a duplicate key are
    /// normalized on the way in, the last of them to the **last** value
    /// written, which is what the server stores.
    #[test]
    fn a_jsonb_literal_is_canonicalized_the_way_the_server_stores_it() {
        fn holds(field: &str, op: PredicateOp, literal: &str) -> bool {
            ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap()
        }
        fn equal(field: &str, literal: &str) {
            assert!(holds(field, PredicateOp::Ge, literal), "{field} >= {literal}");
            assert!(holds(field, PredicateOp::Le, literal), "{field} <= {literal}");
        }
        for spelling in ["1", "1.0", "1e0", "1.00", "0.1e1", "100e-2"] {
            equal("1", spelling);
        }
        assert!(holds("10", PredicateOp::Gt, "9"));
        equal("-0", "0");
        equal(r#"{"a": 1}"#, r#"{  "a" : 1  }"#);
        equal(r#"{"a": 1}"#, r#"{"a":1}"#);
        equal(r#"{"a": 2}"#, r#"{"a":1,"a":2}"#);
        equal(r#"{"a": 1, "b": 2}"#, r#"{"b":2,"a":1}"#);
        // `escape_json` writes a tab as `\t`, and the `COPY` row then doubles
        // that backslash — so the field below is four characters of escaping
        // deep and the literal, which is COPY-decoded already, is two. Both
        // spellings a JSON escape has for the character are one value;
        // *unescaped* it is not a JSON string at all.
        equal("\"a\\\\tb\"", "\"a\\u0009b\"");
        // A character above the BMP, which `escape_json` writes as itself and
        // a literal may write as a surrogate pair.
        equal("\"\u{1f600}\"", "\"\\ud83d\\ude00\"");
    }

    /// The literal grammar is `jsonb_in`'s and nothing wider: every one of
    /// these is text the server itself refuses (I41), so it is
    /// `Error::PredicateValueDecode` naming the value.
    #[test]
    fn a_jsonb_literal_outside_the_input_grammar_is_refused() {
        for literal in [
            "01",
            "+1",
            ".5",
            "1.",
            "NaN",
            "1 2",
            "[1,]",
            "{a:1}",
            "{",
            "{\"a\":1",
            "'a'",
            "",
            "\"\\x41\"",
            "\"\\ud83d\"",
            "\"\\u0000\"",
            "\"a\nb\"",
            "1e999999",
        ] {
            let err =
                ordered("jsonb", DataType::Utf8View, PredicateOp::Gt, literal, "1").unwrap_err();
            assert!(matches!(err, Error::PredicateValueDecode { .. }), "{literal:?}: {err:?}");
        }
        // A field the parser refuses is the other fault.
        let err = ordered("jsonb", DataType::Utf8View, PredicateOp::Gt, "1", "{oops}").unwrap_err();
        assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");
    }

    /// A `jsonb` column announces that its structure is compared exactly and
    /// only a string leaf is left, on the database's collation (I32, I41).
    /// `json` keeps the text-held sentence, the server defining no order for
    /// it at all.
    #[test]
    fn jsonb_announces_its_string_leaves_and_json_stays_text_held() {
        let note = |declared, literal| {
            let p = order_predicate(PredicateOp::Gt, literal);
            only_note(&resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap())
        };
        let jsonb = note("jsonb", "1").unwrap();
        assert_eq!(jsonb.divergence, ComparisonDivergence::JsonbStringCollation);
        assert!(jsonb.message().contains("structurally"), "{}", jsonb.message());
        assert!(jsonb.message().contains("object key"), "{}", jsonb.message());

        let json = note("json", "anything").unwrap();
        assert_eq!(json.divergence, ComparisonDivergence::AsText);
        assert!(json.message().contains("no comparison"), "{}", json.message());
    }

    /// A bare `numeric` orders as a decimal: `9` is below `10` where the text
    /// it is held as puts it above, and trailing zeros are not part of the
    /// value (I33).
    #[test]
    fn a_bare_numeric_orders_by_value_not_by_its_text() {
        let n =
            |op, value, field| ordered("numeric", DataType::Utf8View, op, value, field).unwrap();
        assert!(n(PredicateOp::Gt, "9", "10"));
        assert!(!n(PredicateOp::Lt, "9", "10"));
        // Equal by value, written two ways — a bare `numeric` preserves the
        // scale it was written with.
        assert!(n(PredicateOp::Ge, "1.5", "1.50"));
        assert!(n(PredicateOp::Le, "1.5", "1.50"));
        assert!(!n(PredicateOp::Gt, "1.5", "1.50"));
        // Sign, then magnitude, then the fraction.
        assert!(n(PredicateOp::Lt, "0", "-0.001"));
        assert!(n(PredicateOp::Gt, "-2", "-1.9"));
        assert!(n(PredicateOp::Gt, "0.45", "0.5"));
        assert!(n(PredicateOp::Lt, "0.55", "0.5"));
        // One zero, however either side spells it.
        assert!(n(PredicateOp::Ge, "-0", "0.000"));
        assert!(n(PredicateOp::Le, "-0", "0.000"));
        // Past every fixed-width integer, which is why the key is digits.
        let big = "1".repeat(200);
        assert!(n(PredicateOp::Gt, &big, &format!("{big}0")));
    }

    /// All three of `numeric`'s specials are ordered, and only a *bare*
    /// column admits the two infinities: any typmod rejects them (I34), so on
    /// a `numeric(77,0)` the literal is refused.
    #[test]
    fn a_bare_numeric_carries_all_three_specials() {
        let n =
            |op, value, field| ordered("numeric", DataType::Utf8View, op, value, field).unwrap();
        assert!(n(PredicateOp::Gt, "1.5", "Infinity"));
        assert!(n(PredicateOp::Lt, "1.5", "-Infinity"));
        assert!(n(PredicateOp::Gt, "Infinity", "NaN"));
        assert!(n(PredicateOp::Ge, "NaN", "NaN"));
        assert!(!n(PredicateOp::Gt, "NaN", "NaN"));
        assert!(n(PredicateOp::Gt, "-Infinity", "Infinity"));
        // `numeric_out`'s spelling and nothing else.
        let err = ordered("numeric", DataType::Utf8View, PredicateOp::Gt, "inf", "0").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");

        // Past `Decimal256`: a `NaN` is reachable, an infinity is not.
        let wide =
            |op, value, field| ordered("numeric(77,0)", DataType::Utf8View, op, value, field);
        assert!(wide(PredicateOp::Gt, "0", "NaN").unwrap());
        let err = wide(PredicateOp::Gt, "Infinity", "0").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
    }

    /// An enum orders by declaration order (I33), which is the reverse of the
    /// label text here: `sad` is declared first and sorts last alphabetically.
    #[test]
    fn an_enum_orders_by_declaration_order() {
        let dict = || DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let e = |op, value, field| ordered("public.mood", dict(), op, value, field).unwrap();
        assert!(e(PredicateOp::Gt, "sad", "ok"));
        assert!(!e(PredicateOp::Lt, "sad", "ok"));
        assert!(e(PredicateOp::Ge, "sad", "sad"));
        assert!(!e(PredicateOp::Gt, "sad", "sad"));

        // A label the type does not declare is not a value of the column, on
        // either side.
        let err = ordered("public.mood", dict(), PredicateOp::Gt, "nope", "sad").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
        let err = ordered("public.mood", dict(), PredicateOp::Gt, "sad", "SAD").unwrap_err();
        assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");
    }

    /// An `interval`'s months collapse to 30 days and its days to 86400
    /// seconds, so `1 mon`, `30 days` and `720:00:00` are one value written
    /// three ways — every answer here read out of
    /// `fixtures/17/oracle/comparisons.tsv`.
    ///
    /// `holds` reads `field <op> literal`, the direction a filter asks in:
    /// the row's own value on the left.
    #[test]
    fn an_interval_orders_by_its_collapsed_span() {
        let holds = |field, op, literal| {
            ordered("interval", DataType::Interval(IntervalUnit::MonthDayNano), op, literal, field)
                .unwrap()
        };
        for spelling in ["30 days", "720:00:00"] {
            assert!(holds(spelling, PredicateOp::Ge, "1 mon"), "{spelling}");
            assert!(holds(spelling, PredicateOp::Le, "1 mon"), "{spelling}");
        }
        // `-1 days` is below `00:00:00`, which bytewise it is not.
        assert!(holds("-1 days", PredicateOp::Lt, "00:00:00"));
        // The full `interval_out` form, with a year part and a time tail.
        let full = "1 year 2 mons 3 days 04:05:06";
        assert!(holds(full, PredicateOp::Lt, "2 years"));
        assert!(holds(full, PredicateOp::Gt, "1 mon"));
        // The sign on a time tail belongs to the whole tail, and a `+` on a
        // part that follows a negative one is `AddPostgresIntPart`'s.
        assert!(holds("-1 days -04:00:00", PredicateOp::Lt, "-1 days"));
        assert!(holds("-1 days +04:00:00", PredicateOp::Gt, "-1 days"));
        // The two infinities are read on every file, in `date_out`'s
        // spellings rather than `numeric_out`'s.
        assert!(holds("infinity", PredicateOp::Gt, "1 mon"));
        assert!(holds("-infinity", PredicateOp::Lt, "1 mon"));
        assert!(holds("infinity", PredicateOp::Ge, "infinity"));
        assert!(matches!(
            ordered(
                "interval",
                DataType::Interval(IntervalUnit::MonthDayNano),
                PredicateOp::Gt,
                "Infinity",
                "1 mon"
            )
            .unwrap_err(),
            Error::PredicateValueDecode { .. }
        ));
    }

    /// The `interval` grammar is `interval_out`'s under `IntervalStyle =
    /// postgres` (I4) and nothing wider, so a spelling the server's *input*
    /// function takes is refused. The first three are literals
    /// `fixtures/17/oracle/literals.tsv` records the server accepting.
    #[test]
    fn an_interval_literal_outside_the_output_grammar_is_refused() {
        for literal in ["1.5 hours", "P1Y2M", "1 century", "1 month", "@ 1 day", "", "1 day "] {
            let err = ordered(
                "interval",
                DataType::Interval(IntervalUnit::MonthDayNano),
                PredicateOp::Gt,
                literal,
                "1 mon",
            )
            .unwrap_err();
            assert!(matches!(err, Error::PredicateValueDecode { .. }), "{literal:?}: {err:?}");
        }
    }

    /// A `time with time zone` sorts by the UTC instant and breaks a tie on
    /// the zone, so `00:00:00-05` is above `00:00:00+00` — five hours later,
    /// though it reads earlier — and two spellings of one instant are still
    /// unequal (`fixtures/16/oracle/comparisons.tsv`).
    #[test]
    fn a_timetz_orders_by_the_utc_instant_then_the_zone() {
        let holds = |field, op, literal| {
            ordered("time with time zone", DataType::Utf8View, op, literal, field).unwrap()
        };
        assert!(holds("00:00:00-05", PredicateOp::Gt, "00:00:00+00"));
        // Same instant, different zone: ordered, and not equal. The stored
        // zone is seconds *west* of GMT, so the value displaying `-05` ranks
        // above the one displaying `+00`.
        assert!(holds("00:00:00-05", PredicateOp::Gt, "05:00:00+00"));
        assert!(!holds("00:00:00-05", PredicateOp::Le, "05:00:00+00"));
        // `24:00:00` is a real boundary value, above everything finite here.
        assert!(holds("24:00:00+00", PredicateOp::Gt, "05:00:00+00"));
        // A `timetz` always carries an offset; one without is not a value.
        assert!(matches!(
            ordered("time with time zone", DataType::Utf8View, PredicateOp::Gt, "00:00:00", "\\N")
                .unwrap_err(),
            Error::PredicateValueDecode { .. }
        ));
    }

    /// `network_cmp_internal`: family first, then the shorter netmask's worth
    /// of address bits, then the netmask, then the whole address (I40). The
    /// last pair is the one no address-then-netmask key can get right.
    #[test]
    fn inet_orders_by_family_then_prefix_then_netmask() {
        let holds =
            |field, op, literal| ordered("inet", DataType::Utf8View, op, literal, field).unwrap();
        // IPv4 below every IPv6 address, whatever the bytes say.
        assert!(holds("192.168.1.1", PredicateOp::Lt, "::1"));
        assert!(holds("192.168.1.1", PredicateOp::Gt, "10.0.0.1"));
        // Same first eight bits, different netmask: the netmask decides,
        // *before* the host bits below it are looked at.
        assert!(holds("10.1.0.0/8", PredicateOp::Lt, "10.0.0.0/16"));
        assert!(holds("10.0.0.0/16", PredicateOp::Gt, "10.1.0.0/8"));
    }

    /// `cidr` compares exactly as `inet` does; the only difference is that
    /// `cidr_in` refuses a value with a bit set below its netmask, and so
    /// does this (`fixtures/16/oracle/literals.tsv`).
    #[test]
    fn a_cidr_refuses_a_literal_with_host_bits_set() {
        let holds =
            |field, op, literal| ordered("cidr", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("10.0.0.0/8", PredicateOp::Lt, "192.168.1.0/24"));
        assert!(holds("192.168.1.0/24", PredicateOp::Lt, "::/0"));
        assert!(matches!(
            ordered("cidr", DataType::Utf8View, PredicateOp::Gt, "192.168.1.1/24", "10.0.0.0/8")
                .unwrap_err(),
            Error::PredicateValueDecode { .. }
        ));
        // The same value is an ordinary `inet`.
        assert!(
            ordered("inet", DataType::Utf8View, PredicateOp::Lt, "192.168.1.1/24", "10.0.0.0/8")
                .unwrap()
        );
    }

    /// The two MAC types compare as their bytes and differ only in width, so
    /// a six-octet literal is not a `macaddr8` value and the reverse holds
    /// too. The colon form is the only one read, `macaddr_out` writing it.
    #[test]
    fn a_macaddr_compares_as_its_octets_at_its_own_width() {
        let holds = |declared, field, op, literal| {
            ordered(declared, DataType::Utf8View, op, literal, field).unwrap()
        };
        assert!(holds("macaddr", "08:00:2b:01:02:03", PredicateOp::Lt, "08:00:2b:01:02:04"));
        assert!(holds(
            "macaddr8",
            "08:00:2b:01:02:03:04:05",
            PredicateOp::Lt,
            "08:00:2b:01:02:03:04:06"
        ));
        for (declared, literal) in [
            ("macaddr", "08:00:2b:01:02:03:04:05"),
            ("macaddr8", "08:00:2b:01:02:03"),
            // A separator the server takes and `macaddr_out` never writes.
            ("macaddr", "08-00-2b-01-02-03"),
            // `from_str_radix` would take the sign; the digit check does not.
            ("macaddr", "+8:00:2b:01:02:03"),
        ] {
            let err =
                ordered(declared, DataType::Utf8View, PredicateOp::Gt, literal, "\\N").unwrap_err();
            assert!(
                matches!(err, Error::PredicateValueDecode { .. }),
                "{declared} {literal}: {err:?}"
            );
        }
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
        // `\x00ff` is COPY-escaped in the row and sorts after `\x00`.
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

    /// `=` renders the filter's literal into the spelling the file holds and
    /// compares bytes, so a literal that is the same *value* written
    /// differently matches.
    #[test]
    fn equality_canonicalizes_the_literal_once() {
        let eq = |declared, data_type, literal, field| {
            ordered(declared, data_type, PredicateOp::Eq, literal, field).unwrap()
        };
        // The scale the column declares, which the file writes out in full.
        assert!(eq("numeric(10,2)", DataType::Decimal128(10, 2), "1.5", "1.50"));
        assert!(!eq("numeric(10,2)", DataType::Decimal128(10, 2), "1.5", "1.51"));
        // A leading zero, a leading plus, a case difference, an input-only
        // separator: each is the same value the file spells one way.
        assert!(eq("integer", DataType::Int32, "+007", "7"));
        assert!(eq("oid", DataType::UInt32, "0042", "42"));
        assert!(eq(
            "uuid",
            DataType::FixedSizeBinary(16),
            "0AF1C2D3-0000-0000-0000-000000000000",
            "0af1c2d3-0000-0000-0000-000000000000"
        ));
        assert!(eq("macaddr", DataType::Utf8View, "08:00:2B:01:02:03", "08:00:2b:01:02:03"));
        // Both `numeric` kinds spell `NaN` the way their own `*_out` does,
        // so it renders to itself.
        assert!(eq("numeric(10,2)", DataType::Decimal128(10, 2), "NaN", "NaN"));
        assert!(eq("date", DataType::Date32, "infinity", "infinity"));
        assert!(!eq("date", DataType::Date32, "infinity", "-infinity"));
    }

    /// `=` on a `text` or `varchar` column is a byte comparison:
    /// `CompareKind::Text` renders the literal to itself, so the commonest
    /// column there is carries no per-row work.
    #[test]
    fn equality_on_a_text_column_is_unchanged() {
        for declared in ["text", "character varying(10)", "name", "json"] {
            assert!(ordered(declared, DataType::Utf8View, PredicateOp::Eq, "a b", "a b").unwrap());
            assert!(
                !ordered(declared, DataType::Utf8View, PredicateOp::Eq, "a b", "a  b").unwrap()
            );
        }
    }

    /// `character(n)` is the third category — the *field* narrowed per row.
    /// The dump writes every value padded to `n` and `bpchareq` strips the
    /// padding from both sides (I38), so an unpadded literal matches.
    #[test]
    fn equality_on_a_char_column_trims_both_sides() {
        let eq = |literal, field| {
            ordered("character(10)", DataType::Utf8View, PredicateOp::Eq, literal, field).unwrap()
        };
        assert!(eq("a", "a         "));
        assert!(eq("a         ", "a"));
        // A tab is a value byte, not padding. The *field* side is COPY TEXT,
        // so the tab is written escaped; the literal side is a `Predicate`'s
        // own value and is not.
        assert!(!eq("a", "a\\t        "));
        assert!(eq("a\t", "a\\t        "));
    }

    /// The five kinds whose `*_out` is not injective over the values one file
    /// can hold: no rendering of the literal makes them bytewise, so both
    /// sides are decoded per row.
    #[test]
    fn the_decoded_kinds_call_two_spellings_of_one_value_equal() {
        let eq = |declared, literal, field| {
            ordered(declared, DataType::Utf8View, PredicateOp::Eq, literal, field).unwrap()
        };
        // A bare `numeric` keeps its display scale (I33).
        assert!(eq("numeric", "1.5", "1.50"));
        assert!(eq("numeric", "1.50", "1.5"));
        assert!(!eq("numeric", "1.5", "1.51"));
        // An `interval` collapses months to 30 days and days to 86400 s (I40).
        assert!(eq("interval", "1 mon", "30 days"));
        assert!(eq("interval", "30 days", "720:00:00"));
        // `float8eq` has one zero, and a dump can write `-0`.
        assert!(
            ordered("double precision", DataType::Float64, PredicateOp::Eq, "0", "-0").unwrap()
        );
        // ... and `NaN = NaN` is true on the server, which is neither IEEE's
        // answer nor Rust's (I33).
        assert!(
            ordered("double precision", DataType::Float64, PredicateOp::Eq, "NaN", "NaN").unwrap()
        );
        // `jsonb` prints its numbers through `numeric_out` and compares them
        // by value (I41), so one document is written two ways.
        assert!(eq("jsonb", "{\"a\": 1.50}", "{\"a\": 1.5}"));
        assert!(!eq("jsonb", "{\"a\": 1.5}", "{\"a\": 2}"));
        // The two that decode for a reason about this build rather than
        // about PostgreSQL.
        assert!(eq("inet", "10.0.0.1/32", "10.0.0.1"));
        assert!(eq("time with time zone", "00:00:00+00", "00:00:00+00"));
        assert!(!eq("time with time zone", "00:00:00+00", "01:00:00+01"));
    }

    /// `!=` is `=` inverted, and a NULL field matches neither.
    #[test]
    fn typed_inequality_inverts_and_a_null_matches_neither() {
        let ne = |literal, field| {
            ordered("numeric(10,2)", DataType::Decimal128(10, 2), PredicateOp::Ne, literal, field)
                .unwrap()
        };
        assert!(!ne("1.5", "1.50"));
        assert!(ne("1.5", "1.51"));
        for op in [PredicateOp::Eq, PredicateOp::Ne] {
            assert!(
                !ordered("integer", DataType::Int32, op, "1", "\\N").unwrap(),
                "a NULL field matches no comparison"
            );
        }
    }

    /// A literal that is not a value of the column's type is
    /// `Error::PredicateValueDecode` before a row is read — the same refusal
    /// an ordering operator makes, on the same output-form-only grammar.
    #[test]
    fn an_undecodable_equality_literal_is_refused_at_resolution() {
        for (declared, data_type, literal) in [
            ("integer", DataType::Int32, "abc"),
            ("boolean", DataType::Boolean, "true"),
            ("oid", DataType::UInt32, "-1"),
            ("bytea", DataType::Binary, "abc"),
            ("public.mood", DataType::Utf8View, "furious"),
            ("numeric(10,2)", DataType::Decimal128(10, 2), "1.005"),
            ("interval", DataType::Interval(IntervalUnit::MonthDayNano), "1 month"),
        ] {
            let p = order_predicate(PredicateOp::Eq, literal);
            assert!(
                matches!(
                    resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap_err(),
                    Error::PredicateValueDecode { value, .. } if value == literal
                ),
                "{declared} = {literal}"
            );
        }
    }

    /// The refusal names the form the column's `CompareKind` reads, not only
    /// the value it turned down — so a `boolean` is told how a `boolean` is
    /// written, and `interval`, `inet` and `macaddr` are answered by the same
    /// sentence.
    #[test]
    fn a_refused_literal_names_the_form_the_column_accepts() {
        for (declared, data_type, literal, accepted) in [
            ("boolean", DataType::Boolean, "true", "`t` or `f`"),
            (
                "interval",
                DataType::Interval(IntervalUnit::MonthDayNano),
                "1 month",
                "the way `interval` prints it",
            ),
            ("inet", DataType::Utf8View, "10", "a full IPv4 or IPv6 address"),
            ("macaddr", DataType::Utf8View, "08-00-2b-01-02-03", "six colon-separated hex pairs"),
            // The two arms where the kind's own payload is the answer: the
            // enum's declared labels, and the `numeric` scale.
            ("public.mood", DataType::Utf8View, "furious", "declared labels: 'sad', 'ok'"),
            (
                "numeric(10,2)",
                DataType::Decimal128(10, 2),
                "1.005",
                "as a number with at most 2 decimal places, or `NaN`",
            ),
        ] {
            let p = order_predicate(PredicateOp::Eq, literal);
            let message =
                resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap_err().to_string();
            assert!(message.contains(literal), "{declared} = {literal}: {message}");
            assert!(message.contains(accepted), "{declared} = {literal}: {message}");
        }
    }

    /// The enum clause names the labels themselves, quoted the way the dump
    /// writes them and a `--filter` value reads them back, and stops at
    /// [`ENUM_LABELS_SHOWN`] with a pointer at the output that carries the
    /// rest.
    #[test]
    fn the_enum_clause_quotes_its_labels_and_caps_the_list() {
        let kind = |labels: &[&str]| {
            CompareKind::Enum(labels.iter().map(|l| (*l).to_string()).collect::<Arc<[String]>>())
        };
        assert_eq!(
            accepted_form(&kind(&["sad", "has space", "has'quote"])),
            "as one of the type's declared labels: 'sad', 'has space', 'has''quote'"
        );

        let many: Vec<String> = (0..ENUM_LABELS_SHOWN + 3).map(|i| format!("l{i}")).collect();
        let clause = accepted_form(&CompareKind::Enum(many.iter().cloned().collect()));
        assert!(clause.contains("'l0'"), "{clause}");
        assert!(clause.contains(&format!("'l{}'", ENUM_LABELS_SHOWN - 1)), "{clause}");
        assert!(!clause.contains(&format!("'l{ENUM_LABELS_SHOWN}'")), "{clause}");
        assert!(clause.ends_with("and 3 more; see `info --detail`"), "{clause}");
    }

    /// The `numeric(p,s)` clause branches on the sign of the scale, as what
    /// the column refuses does: a positive scale bounds the fraction, a zero
    /// scale admits no fraction, and a negative one admits only multiples of
    /// a power of ten.
    #[test]
    fn the_numeric_clause_branches_on_the_sign_of_the_scale() {
        for (scale, expected) in [
            (2i8, "as a number with at most 2 decimal places, or `NaN`"),
            (1, "as a number with at most 1 decimal place, or `NaN`"),
            (0, "as a whole number, or `NaN`"),
            (-2, "as a whole number that is a multiple of 100, or `NaN`"),
        ] {
            assert_eq!(accepted_form(&CompareKind::Decimal(scale)), expected, "scale {scale}");
        }
        // The scale-free arm keeps the scale-free clause: a `p > 76` column
        // compares as text and refuses no literal for its shape.
        assert_eq!(
            accepted_form(&CompareKind::Numeric { infinities: false }),
            "as a number, or `NaN`"
        );
    }

    /// A divergence is operator-conditional, and three of the six reach
    /// ordering alone: a libc collation is deterministic, so `texteq` is a
    /// byte comparison whatever the collation is.
    #[test]
    fn a_collation_divergence_does_not_reach_equality() {
        let note = |declared, op, literal| {
            let p = order_predicate(op, literal);
            only_note(&resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap())
        };
        for declared in ["text", "character varying(10)", "character(10)"] {
            assert_eq!(
                note(declared, PredicateOp::Gt, "a").map(|n| n.divergence),
                Some(ComparisonDivergence::UnknownCollation),
                "{declared} under an ordering operator"
            );
            assert_eq!(note(declared, PredicateOp::Eq, "a"), None, "{declared} under `=`");
        }
        assert_eq!(
            note("jsonb", PredicateOp::Gt, "1").map(|n| n.divergence),
            Some(ComparisonDivergence::JsonbStringCollation)
        );
        assert_eq!(note("jsonb", PredicateOp::Eq, "1"), None);
        // `json` reaches both, the server defining neither operator for it.
        for op in [PredicateOp::Gt, PredicateOp::Eq] {
            assert_eq!(
                note("json", op, "1").map(|n| n.divergence),
                Some(ComparisonDivergence::AsText)
            );
        }
    }

    /// A column the register has no comparison for still answers `=`, as
    /// text, and says so where the column is a resolved scalar: `box_eq`
    /// compares areas and a byte comparison does not.
    #[test]
    fn a_column_with_no_comparison_answers_equality_and_says_when_that_is_a_guess() {
        let p = order_predicate(PredicateOp::Eq, "(1,1),(0,0)");

        // `box` is `ColumnResolution::UnknownType` in a real query, this
        // build mapping no Arrow type for it, and that is the outcome the
        // announcement is keyed on.
        let mut scalar = one_column("box", DataType::Utf8View);
        scalar.columns[0] = ColumnResolution::UnknownType;
        let term = resolve_term(&p, 0, &scalar, 0).unwrap();
        let note = only_note(&term).expect("an unmodelled scalar announces");
        assert_eq!(note.divergence, ComparisonDivergence::UnmodelledType);
        assert!(note.message().contains("`box` compares areas"), "{}", note.message());
        assert!(
            term.eval(RawRow::unchecked(b"(1,1),(0,0)"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );
        assert!(
            !term
                .eval(RawRow::unchecked(b"(3,3),(2,2)"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );

        // A nested column the register does not compare is the other
        // plan-less population and is silent: its `range_out` text renders
        // the value, so a byte comparison of two canonical spellings is the
        // server's answer.
        let mut nested = one_column("public.opaquerange", DataType::Utf8View);
        nested.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
        assert_eq!(only_note(&resolve_term(&p, 0, &nested, 0).unwrap()), None);

        // So is a column no DDL explained — `--data-only`, or
        // `--schema-mode strings`. There is no declared type to qualify, and
        // `ResolvedSchema::notes` has already said so.
        let mut undeclared = one_column("mystery", DataType::Utf8View);
        undeclared.columns[0] = ColumnResolution::NotDeclared;
        assert_eq!(only_note(&resolve_term(&p, 0, &undeclared, 0).unwrap()), None);
    }

    /// A nested column one of whose positions has no order here still
    /// answers `=`, over the container's whole text, and **says what that
    /// costs**: `array_cmp` and `record_cmp` look up the position type's
    /// comparison proc and raise when there is none, so a `json` element
    /// takes the server's `=` away as surely as it takes its order. That is
    /// [`ComparisonDivergence::AsText`]'s own sentence one level down, and
    /// the note names the position exactly as the ordering refusal does.
    #[test]
    fn a_nested_uncomparable_position_announces_under_equality() {
        let types = vec![TypeDef {
            name: "public.jsonpair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("ok", "integer"), ColumnDef::new("doc", "json")]),
            },
        }];
        for (declared, path) in
            [("json[]", "[]"), ("public.jsonpair", ".doc"), ("public.jsonpair[]", "[].doc")]
        {
            let resolved = nested_column(declared, &types);
            for op in [PredicateOp::Eq, PredicateOp::Ne] {
                let term = resolve_term(&order_predicate(op, "{}"), 0, &resolved, 0).unwrap();
                let note =
                    only_note(&term).unwrap_or_else(|| panic!("{declared} under {}", op.symbol()));
                assert_eq!(note.column, "v", "{declared}");
                assert_eq!(note.path.as_deref(), Some(path), "{declared}");
                assert_eq!(note.declared_type, "json", "{declared}");
                assert_eq!(note.divergence, ComparisonDivergence::AsText, "{declared}");
                assert!(note.message().contains("no equality"), "{}", note.message());
            }
            // `=` is the byte comparison of the container's own canonical
            // text.
            let term =
                resolve_term(&order_predicate(PredicateOp::Eq, "{1,2}"), 0, &resolved, 0).unwrap();
            assert!(
                term.eval(RawRow::unchecked(b"{1,2}"), &mut RowSplit::default(), "public.t", 0)
                    .unwrap()
                    .is_true(),
                "{declared}"
            );
            assert!(
                !term
                    .eval(RawRow::unchecked(b"{1,3}"), &mut RowSplit::default(), "public.t", 0)
                    .unwrap()
                    .is_true(),
                "{declared}"
            );
            // The announcement is under `=`; `<` still names the same
            // position in a refusal.
            let Err(err) = resolve_term(&order_predicate(PredicateOp::Lt, "{}"), 0, &resolved, 0)
            else {
                panic!("{declared} is ordered");
            };
            assert!(err.to_string().contains(&format!("`{path}`")), "{err}");
        }
    }

    /// A column the **resolver** declined announces off its resolution and
    /// never off its tree — the boundary worth pinning, since the tree such a
    /// column carries does hold an `Uncomparable` position and the
    /// announcement above must not reach it. Both shapes resolve the column
    /// itself to text (I22, I26), so `resolve_term` drops the plan one branch
    /// earlier than the nested one.
    ///
    /// Silence is the right answer for `public.intarr[]`: PostgreSQL orders it
    /// through `array_ops` and its `array_out` text renders the value. For
    /// `box[]` it is `KD10` one level down, `box_eq` comparing areas; closing
    /// that is a question about which *resolutions* announce, not about this
    /// tree.
    #[test]
    fn a_position_the_resolver_declined_announces_nothing() {
        let types =
            vec![TypeDef { name: "public.intarr".into(), kind: TypeKind::domain("integer[]") }];
        for (declared, resolution) in [
            ("box[]", ColumnResolution::OpaqueElementType),
            ("public.intarr[]", ColumnResolution::NestedArrayElement),
        ] {
            let mut resolved = one_column(declared, DataType::Utf8View);
            resolved.comparisons = vec![comparison_for(declared, None, &types, &[])];
            assert!(
                matches!(resolved.comparisons[0], ComparisonPlan::Nested(NestedCompare::Array(_))),
                "{declared} carries a tree with an uncomparable position"
            );
            resolved.columns[0] = resolution;
            let term =
                resolve_term(&order_predicate(PredicateOp::Eq, "{}"), 0, &resolved, 0).unwrap();
            assert_eq!(only_note(&term), None, "{declared}");
        }
    }

    /// A column stating a collation the dump declares `deterministic = false`
    /// announces under **both** operator families (I42) — the only collation
    /// divergence that reaches `=`.
    ///
    /// The rows still come back bytewise, which is the defect the note names:
    /// under a non-deterministic collation two values spelled differently can
    /// be equal to the server, so `=` is a weaker filter than the server's and
    /// `<` is a different order.
    #[test]
    fn a_non_deterministic_collation_announces_under_equality_and_ordering() {
        let collations = [CollationDef { name: "public.icu_ci".to_string(), deterministic: false }];
        let schema = |collation: Option<&str>, declared: &[CollationDef]| {
            let mut resolved = one_column("text", DataType::Utf8View);
            resolved.comparisons = vec![comparison_for("text", collation, &[], declared)];
            resolved
        };

        for op in [PredicateOp::Gt, PredicateOp::Eq, PredicateOp::Ne] {
            let p = order_predicate(op, "a");
            let resolved = schema(Some("public.icu_ci"), &collations);
            let note = only_note(&resolve_term(&p, 0, &resolved, 0).unwrap())
                .unwrap_or_else(|| panic!("{op:?} announces"));
            assert_eq!(note.divergence, ComparisonDivergence::NonDeterministicCollation);
            assert!(note.message().contains("non-deterministic"), "{}", note.message());
        }

        // The same clause with nothing declared about it is the ordinary
        // named-collation divergence, which `=` does not see.
        let eq = order_predicate(PredicateOp::Eq, "a");
        let plain = schema(Some("public.icu_ci"), &[]);
        assert_eq!(only_note(&resolve_term(&eq, 0, &plain, 0).unwrap()), None);

        // The comparison itself is `Text`, so the term evaluates bytewise.
        let term = resolve_term(&eq, 0, &schema(Some("public.icu_ci"), &collations), 0).unwrap();
        assert!(only_note(&term).is_some());
        assert!(
            term.eval(RawRow::unchecked(b"a"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );
        assert!(
            !term
                .eval(RawRow::unchecked(b"A"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );
    }

    /// The three tie-breaks `array_cmp` reaches only when the elements agree,
    /// in the order it reaches them: element count, then dimension count,
    /// then the dimensions themselves, then the lower bounds (I45).
    ///
    /// The dimension pair is the one no oracle case carries: `{{1,2,3,4}}` and
    /// `{{1,2},{3,4}}` hold the same four elements in the same order, are both
    /// two-dimensional, and differ only in `dims`.
    #[test]
    fn an_array_falls_back_to_its_shape_only_when_the_elements_agree() {
        let types = test_types();
        let lt = |field: &str, literal: &str| {
            nested_verdict("integer[]", &types, PredicateOp::Lt, field, literal).unwrap()
        };
        // Elements first: element 0 settles `{1,9}` against `{2,0}` before
        // any count is looked at.
        assert_eq!(lt("{1,9}", "{2,0}"), Truth::True);
        // Element count, once the shorter array's elements agree.
        assert_eq!(lt("{1,2}", "{1,2,3}"), Truth::True);
        // Dimension count, at equal element counts.
        assert_eq!(lt("{{1},{2}}", "{1,2}"), Truth::False);
        // The dimensions, at equal element and dimension counts.
        assert_eq!(lt("{{1,2,3,4}}", "{{1,2},{3,4}}"), Truth::True);
        // The lower bounds, last of all.
        assert_eq!(lt("[0:1]={1,2}", "{1,2}"), Truth::True);
        // Two NULLs are equal and a NULL is above every value, at every
        // level and under every operator.
        assert_eq!(
            nested_verdict("integer[]", &types, PredicateOp::Eq, "{NULL}", "{NULL}").unwrap(),
            Truth::True
        );
        assert_eq!(lt("{NULL}", "{2147483647}"), Truth::False);
    }

    /// A zero-field composite is written `()`, and so is a one-field
    /// composite holding NULL — the literal cannot tell them apart and the
    /// declared field list decides (I23). The strict field decoder cannot ask
    /// that question for itself, so the comparison asks it.
    #[test]
    fn a_zero_field_composite_compares_equal_to_itself() {
        let types = [
            TypeDef {
                name: "public.empty_comp".into(),
                kind: TypeKind::Composite { fields: Some(Vec::new()) },
            },
            TypeDef {
                name: "public.one".into(),
                kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
            },
        ];
        assert_eq!(
            nested_verdict("public.empty_comp", &types, PredicateOp::Eq, "()", "()").unwrap(),
            Truth::True
        );
        assert_eq!(
            nested_verdict("public.one", &types, PredicateOp::Eq, "()", "()").unwrap(),
            Truth::True,
            "one field, holding NULL, on both sides"
        );
        // A field count the composite does not declare is not a value of the
        // type: `Error::FieldDecode` for a field, and the literal is refused
        // at resolution.
        assert!(matches!(
            nested_verdict("public.one", &types, PredicateOp::Eq, "(1,2)", "(1)"),
            Err(Error::FieldDecode { .. })
        ));
        assert!(matches!(
            nested_verdict("public.one", &types, PredicateOp::Eq, "(1)", "(1,2)"),
            Err(Error::PredicateValueDecode { .. })
        ));
    }

    /// `int2vector` names no operator of its own, so the server compares it
    /// through `anyarray` polymorphism — element-wise, not over the text
    /// (I47). `'2' < '10'` is where the two answers part company, and is what
    /// the committed oracle case is built around.
    #[test]
    fn an_int2vector_compares_element_wise_and_not_as_text() {
        let lt = |field: &str, literal: &str| {
            nested_verdict("int2vector", &[], PredicateOp::Lt, field, literal).unwrap()
        };
        assert_eq!(lt("2", "10"), Truth::True, "element-wise; bytewise would say false");
        assert_eq!(lt("10", "2"), Truth::False);
        // `array_cmp`: the elements first, then the shorter one.
        assert_eq!(lt("1 2", "1 2 3"), Truth::True);
        assert_eq!(lt("", "0"), Truth::True);
        assert_eq!(lt("-32768 32767", "0"), Truth::True);
        assert_eq!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2 3", "1 2 3").unwrap(),
            Truth::True
        );
        // The literal takes `int2vectorin`'s superset, the type having no
        // element input function for the leaf rule to apply to.
        assert_eq!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2", "  +1   02  ").unwrap(),
            Truth::True
        );
        // And the array spelling is not this type's, on either side.
        assert!(matches!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2", "{1,2}"),
            Err(Error::PredicateValueDecode { .. })
        ));
        assert!(matches!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "{1,2}", "1 2"),
            Err(Error::FieldDecode { .. })
        ));
    }

    /// The literal side reads the container's `*_in` superset and each leaf
    /// its own `*_out` form — one rule at every depth. So `{ 1 , 2 }` is
    /// `{1,2}` (`array_in` drops whitespace around an element) while
    /// `( 1 ,a)` is refused (`record_in` keeps it, and `1 ` is not what
    /// `int4out` writes).
    #[test]
    fn a_nested_literal_is_lenient_about_the_container_and_strict_about_the_leaf() {
        let types = [TypeDef {
            name: "public.point2d".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("x", "integer"), ColumnDef::new("y", "text")]),
            },
        }];
        let ints = test_types();
        for literal in ["{ 1 , 2 }", "{1,2}", "[1:2]={1,2}", "{1,\"2\"}"] {
            assert_eq!(
                nested_verdict("integer[]", &ints, PredicateOp::Eq, "{1,2}", literal).unwrap(),
                Truth::True,
                "{literal}"
            );
        }
        assert_eq!(
            nested_verdict("public.point2d", &types, PredicateOp::Eq, "(1,a)", "(1,a)").unwrap(),
            Truth::True
        );
        let err = nested_verdict("public.point2d", &types, PredicateOp::Eq, "(1,a)", "( 1 ,a)")
            .unwrap_err();
        let Error::PredicateValueDecode { accepted, .. } = &err else { panic!("{err:?}") };
        assert!(accepted.contains("spelled as the dump spells it"), "{accepted}");
    }

    /// A range's bounds settle infinity, then the held value, then
    /// inclusivity — and an exclusive bound's answer depends on *which end*
    /// it is (I46).
    ///
    /// `empty` below everything is the fourth rule, and it is not recoverable
    /// from the bounds: `empty` and `(,)` both have two absent bounds and sit
    /// at opposite ends of the order.
    #[test]
    fn a_range_bound_settles_infinity_then_value_then_inclusivity() {
        let types = test_types();
        let lt = |field: &str, literal: &str| {
            nested_verdict("numrange", &types, PredicateOp::Lt, field, literal).unwrap()
        };
        assert_eq!(lt("empty", "(,)"), Truth::True);
        assert_eq!(lt("(,)", "empty"), Truth::False);
        assert_eq!(
            nested_verdict("numrange", &types, PredicateOp::Eq, "empty", "empty").unwrap(),
            Truth::True
        );
        // An absent bound is the extreme of its own end.
        assert_eq!(lt("(,5)", "[1,10)"), Truth::True);
        assert_eq!(lt("[1,10)", "[1,)"), Truth::True);
        // Same value, different inclusivity, and the two ends disagree about
        // which way it goes.
        assert_eq!(lt("[1,10)", "(1,10)"), Truth::True);
        assert_eq!(lt("[1,10)", "[1,10]"), Truth::True);
    }

    /// A discrete range is rewritten into canonical form on the way in and a
    /// continuous one is not, so `[1,10]` and `[1,10)` are one value of
    /// `int4range` and two of `numrange` (I46).
    ///
    /// Two rewrites, and the second is not a successor: bounds that end up
    /// equal without both ends including the point collapse to `empty`, which
    /// is why `int4range '(1,2)'` holds nothing. It runs before the canonical
    /// function *and* after it, which is the only way `(1,2)` reaches it.
    #[test]
    fn a_discrete_range_is_canonicalized_and_a_continuous_one_is_not() {
        let types = test_types();
        let eq = |declared: &str, field: &str, literal: &str| {
            nested_verdict(declared, &types, PredicateOp::Eq, field, literal).unwrap()
        };
        for literal in ["[1,10)", "[1,9]", "(0,10)", "(0,9]"] {
            assert_eq!(eq("int4range", "[1,10)", literal), Truth::True, "{literal}");
        }
        assert_eq!(eq("int4range", "empty", "(1,2)"), Truth::True);
        assert_eq!(eq("int4range", "empty", "[1,1)"), Truth::True);
        assert_eq!(eq("numrange", "empty", "[1,1)"), Truth::True, "the collapse is not discrete");
        assert_eq!(eq("numrange", "[1,10)", "[1,10]"), Truth::False);
        // `daterange_canonical` skips a bound that is not a finite date, so
        // an infinity keeps the inclusivity it was written with (I34) — and
        // the *other* bound is still rewritten.
        assert_eq!(eq("daterange", "[2020-01-01,infinity]", "[2020-01-01,infinity]"), Truth::True);
        assert_eq!(
            eq("daterange", "[-infinity,2020-01-02)", "[-infinity,2020-01-01]"),
            Truth::True
        );
        // The successor can leave the subtype's range, which the server
        // raises on. `int8range` is where this build can see it: a leaf
        // literal is read as `i64` whatever the column's width (see
        // `order_key`), so `int4range` cannot.
        assert!(matches!(
            nested_verdict(
                "int8range",
                &types,
                PredicateOp::Eq,
                "[1,10)",
                "[1,9223372036854775807]"
            ),
            Err(Error::PredicateValueDecode { .. })
        ));
        // A lower bound above its upper is `22000` on the server — a fault
        // the container grammar cannot see, since the text is well formed.
        assert!(matches!(
            nested_verdict("int4range", &types, PredicateOp::Gt, "[1,10)", "[10,1)"),
            Err(Error::PredicateValueDecode { .. })
        ));
        assert!(matches!(
            nested_verdict("int4multirange", &types, PredicateOp::Gt, "{[1,10)}", "{[10,1)}"),
            Err(Error::PredicateValueDecode { .. })
        ));
    }

    /// A multirange's members are sorted, emptied out and merged before
    /// anything is compared, so several spellings are one value and the
    /// comparison itself is a plain sequence walk (I46).
    ///
    /// Whether two members merge is the range type's question, not the
    /// bounds': `{[1,5),[6,10)}` stays two members even in `int4range`, while
    /// `{[1,5],[6,10)}` becomes one, canonicalization having already made the
    /// first `[1,6)`. The same pair in `numrange` never merges.
    #[test]
    fn a_multirange_is_sorted_coalesced_and_emptied_before_it_is_compared() {
        let types = test_types();
        let eq = |declared: &str, field: &str, literal: &str| {
            nested_verdict(declared, &types, PredicateOp::Eq, field, literal).unwrap()
        };
        for literal in ["{[1,10)}", "{[1,5),[5,10)}", "{[5,10),[1,5)}", "{[1,1),[1,10)}"] {
            assert_eq!(eq("int4multirange", "{[1,10)}", literal), Truth::True, "{literal}");
        }
        assert_eq!(eq("int4multirange", "{[1,10)}", "{empty,[1,10)}"), Truth::True);
        assert_eq!(eq("int4multirange", "{[1,5)}", "{[1,3),[2,5)}"), Truth::True, "overlapping");
        assert_eq!(eq("int4multirange", "{[1,5),[6,10)}", "{[1,5),[6,10)}"), Truth::True);
        assert_eq!(eq("int4multirange", "{[1,10)}", "{[1,5],[6,10)}"), Truth::True, "adjacent");
        assert_eq!(eq("nummultirange", "{[1,5),[6,10)}", "{[1,5),[6,10)}"), Truth::True);
        assert_eq!(eq("nummultirange", "{[1,5),(5,10)}", "{[1,5),(5,10)}"), Truth::True);
        // Member-wise, then the shorter one first.
        let lt = |field: &str, literal: &str| {
            nested_verdict("int4multirange", &types, PredicateOp::Lt, field, literal).unwrap()
        };
        assert_eq!(lt("{}", "{[1,10)}"), Truth::True);
        assert_eq!(lt("{[1,5),[6,10)}", "{[1,10)}"), Truth::True);
    }

    /// The comparison register against the committed comparison oracle:
    /// every cell of `fixtures/<13-18>/oracle/comparisons.tsv`, answered by
    /// the same `resolve_term`/`matches` path a `--filter` takes, and compared
    /// with what the server itself said (`docs/design/decisions.md`, "D70").
    ///
    /// This is the check the oracle exists for: `oracle_register.py` and
    /// `oracle_differences.py` are about which rows exist, and nothing else
    /// compares an answer to an answer.
    mod oracle {
        use std::collections::{BTreeMap, BTreeSet};
        use std::path::{Path, PathBuf};

        use super::*;
        use crate::cache::CacheMode;
        use crate::copy::{decode_field, encode_field, split_fields};
        use crate::index::preamble_only;
        use crate::io::LocalFileSource;
        use crate::preamble::{ColumnDef, DatabaseMetadata, DumpMetadata};
        use crate::resolve::{SchemaMode, resolve_columns};
        use crate::scan::ScanOptions;

        /// The majors `scripts/generate_fixtures.py` generates, which is what
        /// `fixtures/` holds a directory per.
        const MAJORS: [u32; 6] = [13, 14, 15, 16, 17, 18];

        /// The six operators this asserts, in `comparisons.tsv`'s own column
        /// order, paired with their cell offset after the four key columns.
        ///
        /// **`=` and `<>` are asked over a wider population than the other
        /// four**: a column the register has no order for still answers `=`
        /// as text, so a case the register does not order is skipped for the
        /// ordering operators and asserted for these two.
        ///
        /// What they add over `<=`/`>=`, which already carried the register's
        /// equality, is the **canonicalization** — the three-way choice
        /// `equality_comparison` makes. Both operands of a cell are values the
        /// server itself stored, so for a canonicalized kind the assertion is
        /// that the file's own spelling compares byte for byte; the content is
        /// in the kinds where it cannot, and every one of them has a case
        /// here: `real`'s `-0` against `0`, a bare `numeric`'s `1.5` against
        /// `1.50` and a `jsonb` number written two ways. `character(10)`'s
        /// trimmed comparison is reached only over values the file pads
        /// alike.
        const ASSERTED: [(usize, PredicateOp); 6] = [
            (0, PredicateOp::Lt),
            (1, PredicateOp::Le),
            (2, PredicateOp::Gt),
            (3, PredicateOp::Ge),
            (4, PredicateOp::Eq),
            (5, PredicateOp::Ne),
        ];

        /// The declared types whose columns the register refuses an ordering
        /// operator on, so none of their ordering cells is asserted: `xml`, an enum with
        /// no labels, a user-defined base type with no operator class, and the
        /// two nested shapes that are refused for reasons of their own.
        /// PostgreSQL orders all of them and this build does not.
        ///
        /// **`json` is not here**, and the difference is the point: it *is*
        /// compared, bytewise, and every cell of it is `E42883` because
        /// PostgreSQL defines no comparison at all — so the walk skips it as
        /// a server refusal rather than as a refusal of ours.
        ///
        /// It is asserted as an exact set, so a type that quietly stops
        /// comparing fails here rather than passing as one more skip.
        const REFUSED: [&str; 6] = [
            // I26: an array whose element is itself an array. The column
            // resolves to text, and the register agrees rather than claiming
            // an order the resolver has already declined.
            "public.intarr[]",
            // A multirange companion **before v14**, where the type does not
            // exist and the range's DDL carries no `multirange_type_name` to
            // find it through (I10). Its v14+ cells are asserted; a case is
            // recorded here if it was refused at *any* major.
            "public.myrange_multi",
            // Scalars with no comparison in the register at all.
            "public.box_domain",
            "public.mybase",
            "public.empty_enum",
            "xml",
        ];

        /// The cases where this build's answer is knowingly not
        /// PostgreSQL's, as `(type, collation, left, right)` — the exception
        /// set, and it is **met**: every entry is a real disagreement in the
        /// committed files, and every disagreement is an entry.
        ///
        /// **Met means met everywhere it is permitted.** An entry is keyed by
        /// the case, so it has to disagree in every major that carries the
        /// case and under every operator its divergence reaches — for a
        /// divergence of order 24 cells, four ordering operators by six
        /// majors. Taking
        /// *somewhere in the walk* as met would instead leave the block
        /// passing after a collation moved for one major, or after the
        /// comparator stopped being antisymmetric under one operator.
        ///
        /// **`=` and `<>` are not among those cells, and that is the
        /// assertion rather than an omission.** Every divergence in this list
        /// but `box`'s, below, is a divergence of *order*
        /// ([`ComparisonDivergence::affects_equality`]) — a libc collation is
        /// deterministic, so `texteq` is a byte comparison whatever the
        /// collation is — so a disagreement under `=` would fail the
        /// announcement check below rather than count toward an entry.
        ///
        /// Each entry's column also has to *announce* its divergence through
        /// [`ComparisonDivergence`], **under this operator**, which is
        /// asserted alongside — so an exception cannot be claimed for a
        /// column the register tells the user it is confident about. **Two
        /// of its populations are one
        /// statement asked at two depths** — a collation the file does not
        /// carry (I32), reached once through a column and once through a
        /// string inside a document:
        ///
        /// - **A `jsonb` string leaf.** `compareJsonbScalarValue` passes
        ///   `DEFAULT_COLLATION_OID` to `varstr_cmp`, so a leaf is ordered by
        ///   the database's collation, which a plain dump does not record
        ///   (I32); everything structural above it — the kind order, a
        ///   container's size, storage order, the raw-scalar wrapper — is
        ///   asserted rather than excepted, which is what makes this list two
        ///   entries rather than the arm.
        /// - **`box`'s area equality**, which is the third population and the
        ///   only one this build could close by writing code. `box_eq`
        ///   compares the two rectangles' *areas*, so the server calls
        ///   `(1,1),(0,0)` and `(3,3),(2,2)` equal where a byte comparison
        ///   does not — and `box` has no comparison in the register, so the
        ///   four ordering operators are refused on it and `=` falls back to
        ///   text. `<>` is not among the cells because PostgreSQL defines no
        ///   `box <> box` at all, which is why each entry is met over six
        ///   cells rather than twenty-four. The column announces
        ///   `ComparisonDivergence::UnmodelledType`, and the deficiency is
        ///   `KD10`.
        /// - **`text` under the database's own collation**, which is glibc's
        ///   `en_US.utf8` on this apparatus: case is a lower-weight
        ///   difference than letter, an accent sorts with its base letter
        ///   rather than after `z`, and punctuation is ignored at the primary
        ///   level, so `_x` sorts where `x` does. The four unordered pairs the
        ///   case table chose for those reasons are here in both directions,
        ///   and `_x` reaches every letter in the alphabet rather than only
        ///   `ax`.
        const EXCEPTIONS: &[(&str, &str, &str, &str)] = &[
            // A `text[]` column inherits its element's collation boundary:
            // the elements are ordered by `varstr_cmp` under the database's
            // collation exactly as a `text` column's values are, one level
            // down. `b` is below `B` under `en_US.utf8` and above it
            // bytewise, and `NULL` is below `a` bytewise and above it under
            // the locale — the same two disagreements the `text` rows below
            // carry, reached through an element.
            ("text[]", "\\N", "{a,b}", "{a,B}"),
            ("text[]", "\\N", "{a,B}", "{a,b}"),
            ("text[]", "\\N", "{\"NULL\"}", "{a,b}"),
            ("text[]", "\\N", "{a,b}", "{\"NULL\"}"),
            ("text[]", "\\N", "{\"NULL\"}", "{a,B}"),
            ("text[]", "\\N", "{a,B}", "{\"NULL\"}"),
            // `box_eq` is an area comparison, not a value one.
            ("public.box_domain", "\\N", "(1,1),(0,0)", "(3,3),(2,2)"),
            ("public.box_domain", "\\N", "(3,3),(2,2)", "(1,1),(0,0)"),
            ("jsonb", "\\N", "{\"a\": \"a\"}", "{\"a\": \"A\"}"),
            ("jsonb", "\\N", "{\"a\": \"A\"}", "{\"a\": \"a\"}"),
            // Case is a lower-weight difference than letter.
            ("text", "default", "A", "a"),
            ("text", "default", "a", "A"),
            ("text", "default", "a", "B"),
            ("text", "default", "B", "a"),
            ("text", "default", "ax", "B"),
            ("text", "default", "B", "ax"),
            // An accent sorts with its base letter, not after `z`.
            ("text", "default", "é", "f"),
            ("text", "default", "f", "é"),
            ("text", "default", "é", "hello"),
            ("text", "default", "hello", "é"),
            // Punctuation is ignored at the primary level, so `_x` sorts
            // where `x` does — above every letter in the alphabet.
            ("text", "default", "_x", "a"),
            ("text", "default", "a", "_x"),
            ("text", "default", "_x", "ax"),
            ("text", "default", "ax", "_x"),
            ("text", "default", "_x", "co-op"),
            ("text", "default", "co-op", "_x"),
            ("text", "default", "_x", "coop"),
            ("text", "default", "coop", "_x"),
            ("text", "default", "_x", "de luge"),
            ("text", "default", "de luge", "_x"),
            ("text", "default", "_x", "deluge"),
            ("text", "default", "deluge", "_x"),
            ("text", "default", "_x", "e"),
            ("text", "default", "e", "_x"),
            ("text", "default", "_x", "é"),
            ("text", "default", "é", "_x"),
            ("text", "default", "_x", "f"),
            ("text", "default", "f", "_x"),
            ("text", "default", "_x", "hello"),
            ("text", "default", "hello", "_x"),
        ];

        fn fixture(major: u32, rest: &str) -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../fixtures/{major}/{rest}"))
        }

        /// Split one COPY TEXT line of a committed oracle file into its
        /// decoded fields — the same L1 decoder that reads a dump, because
        /// the server wrote these files with `COPY ... TO STDOUT`.
        fn fields(line: &[u8]) -> Vec<Option<String>> {
            split_fields(line).map(|f| decode_field(f).unwrap().map(|v| v.into_owned())).collect()
        }

        fn rows(path: &Path) -> Vec<Vec<Option<String>>> {
            let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(fields).collect()
        }

        /// The `COLLATE` clause a dump would carry for a case labelled with
        /// this collation. `C` is written; `default` is *not*, because
        /// `pg_dump` writes a clause only where the column's collation is not
        /// its type's default (I37) — so the dump-realistic form of the
        /// oracle's `COLLATE "default"` case is a bare column, which is the
        /// register's `UnknownCollation` arm. Both reach the same comparison.
        fn clause(collation: Option<&String>) -> Option<&'static str> {
            match collation.map(String::as_str) {
                Some("C") => Some("pg_catalog.\"C\""),
                _ => None,
            }
        }

        /// A one-column schema for a case's declared type and collation,
        /// built by handing `resolve_columns` a one-table `DumpMetadata` —
        /// the same function a real block resolves through.
        ///
        /// **Not a hand-built `ResolvedSchema`**, and the difference is
        /// load-bearing now that `=` is asserted: the resolution, the nested
        /// plan and the comparison plan have to agree the way they do in a
        /// real query, because which of them `resolve_term` consults decides
        /// whether an equality term announces a divergence. A schema claiming
        /// every case is `Mapped` and `Scalar` would put `integer[]` and
        /// `box` in one population that no dump ever produces.
        fn schema(declared: &str, collation: Option<&str>, types: &[TypeDef]) -> ResolvedSchema {
            let column = ColumnDef {
                name: "v".into(),
                declared_type: declared.into(),
                collation: collation.map(str::to_string),
            };
            let metadata = DumpMetadata {
                databases: vec![DatabaseMetadata {
                    name: None,
                    preamble_complete: true,
                    server_version: None,
                    pg_dump_version: None,
                    extensions: Vec::new(),
                    types: types.to_vec(),
                    collations: Vec::new(),
                    tables: [("public.t".to_string(), vec![column])].into_iter().collect(),
                }],
            };
            resolve_columns(
                "public.t",
                &["v".to_string()],
                Some(&metadata),
                None,
                SchemaMode::Typed,
                &[],
            )
        }

        /// The [`Truth`] this build answers a cell with, or the fault it
        /// raised instead — a literal it will not decode, or a field it will
        /// not. `field` is `None` for a SQL NULL one, which reaches the row
        /// as `\N` like any other, and answers `Unknown` exactly where the
        /// server's cell reads `u`.
        ///
        /// The second half of the pair is whether the resolved term
        /// **announces** a divergence, read off `ComparisonNote` — the channel
        /// a user actually sees — rather than off the register's plan. Which
        /// divergences reach which operator is the term's own conclusion
        /// ([`ComparisonDivergence::affects_equality`]), and asking the plan
        /// would be a second copy of that rule living in the test.
        fn answer(
            declared: &str,
            collation: Option<&str>,
            types: &[TypeDef],
            op: PredicateOp,
            field: Option<&str>,
            bound: &str,
        ) -> (std::result::Result<Truth, String>, bool) {
            let resolved = schema(declared, collation, types);
            let p = Predicate { column: "v".into(), op, value: Some(bound.into()) };
            let term = match resolve_term(&p, 0, &resolved, 0) {
                Ok(term) => term,
                Err(e) => return (Err(e.to_string()), false),
            };
            let announced = !term.comparison_notes().is_empty();
            let got = term
                .eval(
                    RawRow::unchecked(&encode_field(field)),
                    &mut RowSplit::default(),
                    "public.t",
                    0,
                )
                .map_err(|e| e.to_string());
            (got, announced)
        }

        /// Every major's `CREATE TYPE`/`CREATE DOMAIN` list, read from the
        /// `types` fixture that major's oracle database was loaded from — the
        /// same DDL, so a case's declared type resolves against the list the
        /// server itself had.
        async fn types_of(major: u32) -> Vec<TypeDef> {
            let source = LocalFileSource::open(fixture(major, "types/default.sql")).unwrap();
            let (metadata, _) =
                preamble_only(&source, &ScanOptions::default(), &CacheMode::DISABLED)
                    .await
                    .unwrap();
            metadata.databases.into_iter().next().expect("a dump names a database").types
        }

        #[tokio::test]
        async fn the_register_answers_every_committed_oracle_cell() {
            // Keyed by case so one disagreement is reported as the pair it
            // is, not as four cells; the value is the majors and operators
            // it showed up under.
            let mut disagreed: BTreeMap<(String, String, String, String), BTreeSet<String>> =
                BTreeMap::new();
            // How many cells each case was asserted over at all, so "met"
            // can mean met everywhere rather than met somewhere.
            let mut walked: BTreeMap<(String, String, String, String), usize> = BTreeMap::new();
            let mut refused: BTreeSet<String> = BTreeSet::new();
            let mut asserted = 0usize;

            for major in MAJORS {
                let types = types_of(major).await;
                // The `*_out` text the server canonicalizes each accepted
                // literal to, keyed by type and literal. It is what a dump
                // *holds*, so every value below is put to the register in
                // this form rather than as the case's input spelling: a
                // `character(10)`'s blank padding, a `jsonb`'s single space
                // after each `:`, a `numeric(10,2)`'s rounded scale.
                let outputs: BTreeMap<(String, String), String> =
                    rows(&fixture(major, "oracle/literals.tsv"))
                        .into_iter()
                        .filter_map(|r| Some(((r[0].clone()?, r[1].clone()?), r[3].clone()?)))
                        .collect();

                for row in rows(&fixture(major, "oracle/comparisons.tsv")) {
                    let declared = row[0].clone().expect("a case names a type");
                    let collation = clause(row[3].as_ref());
                    let plan = comparison_for(&declared, collation, &types, &[]);
                    // A column the register has no comparison for is skipped
                    // for the four ordering operators, which it refuses, and
                    // still asserted for `=`/`<>`, which fall back to text.
                    let ordered = plan.orders();
                    if !ordered {
                        refused.insert(declared.clone());
                    }
                    // How the case names itself in a report and in
                    // `EXCEPTIONS`: the raw cells, so an entry can be copied
                    // out of a failure straight into the table.
                    let case = |left: &str, right: &str| {
                        (
                            declared.clone(),
                            row[3].clone().unwrap_or_else(|| "\\N".into()),
                            left.to_string(),
                            right.to_string(),
                        )
                    };
                    let output = |literal: &str| {
                        outputs.get(&(declared.clone(), literal.to_string())).cloned()
                    };
                    for (offset, op) in ASSERTED {
                        if op.is_ordering() && !ordered {
                            continue;
                        }
                        let cell = row[4 + offset].as_deref().expect("a cell is never NULL");
                        // A cell recording what the server *refused* says
                        // nothing about how it compares.
                        if cell.starts_with('E') {
                            continue;
                        }
                        // A NULL *right* operand has no spelling in the
                        // filter grammar at all — `IS NULL` is how a filter
                        // asks for one — so there is no question to put to
                        // the register.
                        let Some(right) = row[2].as_deref() else { continue };
                        let bound = output(right).expect("an accepted literal has an output");
                        // A NULL *left* operand is the field, and this
                        // build answers `Unknown` for every comparing
                        // operator — the same value the server's `u` records,
                        // asserted as itself rather than as the "excluded"
                        // it collapses to at the root.
                        let Some(left) = row[1].as_deref() else {
                            assert_eq!(cell, "u", "{major} {declared}: a NULL operand");
                            assert_eq!(
                                answer(&declared, collation, &types, op, None, &bound).0,
                                Ok(Truth::Unknown),
                                "{major} {declared}: a NULL field is unknown"
                            );
                            continue;
                        };
                        let field = output(left).expect("an accepted literal has an output");
                        asserted += 1;
                        // `announced` is whether a disagreement is *permitted*
                        // here — the term announces a divergence that reaches
                        // this operator. It is what an exception is checked
                        // against and what the cell count is taken over, so an
                        // entry is never asked to be met under an operator its
                        // divergence does not claim.
                        let (got, announced) =
                            answer(&declared, collation, &types, op, Some(&field), &bound);
                        if announced {
                            *walked.entry(case(left, right)).or_default() += 1;
                        }
                        let expected = match cell {
                            "t" => Truth::True,
                            "f" => Truth::False,
                            other => panic!("{major} {declared}: unexpected cell {other:?}"),
                        };
                        if got != Ok(expected) {
                            // An exception is only ever claimable where the
                            // register has already told the user its answer
                            // may differ *under this operator*.
                            assert!(
                                announced,
                                "{major} {declared} {}: disagrees under {} while announcing no \
                                 divergence that reaches it",
                                row[1].as_deref().unwrap_or("\\N"),
                                op.symbol()
                            );
                            disagreed
                                .entry(case(left, right))
                                .or_default()
                                .insert(format!("{major} {} {got:?}", op.symbol()));
                        }
                    }
                }
            }

            let expected_exceptions: BTreeSet<_> = EXCEPTIONS
                .iter()
                .map(|(t, c, l, r)| (t.to_string(), c.to_string(), l.to_string(), r.to_string()))
                .collect();
            let found: BTreeSet<_> = disagreed.keys().cloned().collect();
            let unexpected: Vec<_> = found.difference(&expected_exceptions).collect();
            let stale: Vec<_> = expected_exceptions.difference(&found).collect();
            assert!(
                unexpected.is_empty(),
                "{} cases disagree with the server and are not in the exception set:\n{}",
                unexpected.len(),
                unexpected
                    .iter()
                    .map(|k| format!("  {k:?} {:?}", disagreed[*k]))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            assert!(stale.is_empty(), "exceptions that no longer disagree: {stale:?}");
            // An entry is keyed by the case, so "met" has to mean met over
            // every cell the case has: a collation that moves for one major,
            // or a comparator that stops being antisymmetric under one
            // operator, would otherwise leave the entry satisfied by the
            // majors and operators it still disagrees under.
            let partial: Vec<_> = expected_exceptions
                .iter()
                .filter(|case| disagreed[*case].len() != walked[*case])
                .map(|case| format!("  {case:?} {:?} of {} cells", disagreed[case], walked[case]))
                .collect();
            assert!(
                partial.is_empty(),
                "{} exceptions disagree in only part of the walk:\n{}",
                partial.len(),
                partial.join("\n")
            );
            assert_eq!(
                refused,
                REFUSED.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
                "the set of declared types the register refuses an ordering operator on"
            );
            // A floor, not a count: the walk skips a cell for four good
            // reasons, and a bug in any of them would leave it asserting
            // almost nothing while passing.
            assert!(asserted > 45_000, "only {asserted} cells asserted");
        }

        /// **The key a filter compares by is a total order over every value
        /// the server wrote**, per declared type the register compares as a
        /// scalar: reflexive, antisymmetric and transitive over the output
        /// spelling of every literal it accepted. A single comparison needs
        /// none of that, and a running minimum, maximum or sortedness flag
        /// needs all of it — two values that never meet in a filter still
        /// have to agree with a third about where they sit.
        ///
        /// Whether that order is *the server's* is the cell walk above; this
        /// is the property that walk cannot see, since it asks each pair
        /// once against a literal.
        #[tokio::test]
        async fn the_key_is_a_total_order_over_every_committed_value() {
            let mut kinds = 0usize;
            let mut triples = 0usize;
            for major in MAJORS {
                let types = types_of(major).await;
                let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    if let (Some(declared), Some("ok"), Some(output)) =
                        (row[0].clone(), row[2].as_deref(), row[3].clone())
                    {
                        outputs.entry(declared).or_default().insert(output);
                    }
                }
                for (declared, values) in outputs {
                    let ComparisonPlan::Compared { kind, .. } =
                        comparison_for(&declared, None, &types, &[])
                    else {
                        continue;
                    };
                    kinds += 1;
                    let keys: Vec<(&String, OrderKey)> = values
                        .iter()
                        .map(|v| {
                            let key = order_key(&kind, v).unwrap_or_else(|| {
                                panic!("{major} {declared}: the server wrote {v:?}")
                            });
                            (v, key)
                        })
                        .collect();
                    for (a, ka) in &keys {
                        assert_eq!(compare_keys(ka, ka), Ordering::Equal, "{major} {declared} {a}");
                        for (b, kb) in &keys {
                            let ab = compare_keys(ka, kb);
                            assert_eq!(
                                ab,
                                compare_keys(kb, ka).reverse(),
                                "{major} {declared}: {a:?} against {b:?}"
                            );
                            for (c, kc) in &keys {
                                triples += 1;
                                let bc = compare_keys(kb, kc);
                                if ab != Ordering::Greater && bc != Ordering::Greater {
                                    let ac = compare_keys(ka, kc);
                                    assert_ne!(
                                        ac,
                                        Ordering::Greater,
                                        "{major} {declared}: {a:?} <= {b:?} <= {c:?}"
                                    );
                                    if ab == Ordering::Equal && bc == Ordering::Equal {
                                        assert_eq!(ac, Ordering::Equal);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            // Floors, not counts: the scalar kinds of six majors.
            assert!(kinds > 150, "only {kinds} declared types walked");
            assert!(triples > 50_000, "only {triples} triples asserted");
        }

        /// The persisted format version and the ordering digest it was pinned
        /// beside, re-pinned together (`golden_order_is_pinned_to_the_format_version`).
        const GOLDEN_ORDER: (u32, u64) = (23, 6_241_334_826_557_786_742);

        /// **Every committed oracle value, sorted under its declared type's
        /// comparison kind, digests to the value pinned beside the cache's
        /// `CACHE_FORMAT_VERSION`.** A stored bound, sortedness or dictionary means
        /// what this build's comparison says, so a change to how any kind
        /// orders or equates values is a persisted reshape that bumps the
        /// version (`docs/design/decisions.md`, "D22"); this fails until it
        /// is bumped and the digest re-pinned. A regenerated oracle moves the
        /// digest with no change to any comparison, and is re-pinned alone.
        ///
        /// The digest is FNV-1a over each major, each declared type and its
        /// values in key order, ties broken by text, with whether each value
        /// keys equal to the one before it — so equality moves it as well as
        /// order.
        #[tokio::test]
        async fn golden_order_is_pinned_to_the_format_version() {
            fn fnv(hash: &mut u64, bytes: &[u8]) {
                for byte in bytes {
                    *hash ^= u64::from(*byte);
                    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            let mut digest = 0xcbf2_9ce4_8422_2325u64;
            let mut values_sorted = 0usize;
            for major in MAJORS {
                let types = types_of(major).await;
                let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    if let (Some(declared), Some("ok"), Some(output)) =
                        (row[0].clone(), row[2].as_deref(), row[3].clone())
                    {
                        outputs.entry(declared).or_default().insert(output);
                    }
                }
                for (declared, values) in outputs {
                    let ComparisonPlan::Compared { kind, .. } =
                        comparison_for(&declared, None, &types, &[])
                    else {
                        continue;
                    };
                    let mut keyed: Vec<(ValueKey, &String)> = values
                        .iter()
                        .map(|v| (ValueKey::of(&kind, v).expect("the server wrote it"), v))
                        .collect();
                    keyed.sort_by(|(ka, a), (kb, b)| ka.compare(kb).then_with(|| a.cmp(b)));
                    fnv(&mut digest, format!("{major}\t{declared}\n").as_bytes());
                    let mut previous: Option<&ValueKey> = None;
                    for (key, value) in &keyed {
                        let tie = previous.is_some_and(|p| p.compare(key) == Ordering::Equal);
                        fnv(&mut digest, if tie { b"=" } else { b"<" });
                        fnv(&mut digest, value.as_bytes());
                        previous = Some(key);
                        values_sorted += 1;
                    }
                }
            }
            assert!(values_sorted > 800, "only {values_sorted} values sorted");
            assert_eq!(
                (crate::cache::CACHE_FORMAT_VERSION, digest),
                GOLDEN_ORDER,
                "the comparison order moved, or CACHE_FORMAT_VERSION did: bump CACHE_FORMAT_VERSION if any \
                 kind orders or equates differently, then re-pin both here"
            );
        }

        /// Whether each [`CompareKind`] orders the values its column emits as
        /// DataFusion orders the emitted array ([`arrow_against`]), and whether
        /// the bounds gathering stores under it are that order's minimum and
        /// maximum of the group, as `(kind, order agrees, bounds agree)`. Asserted as the exact table,
        /// every kind present, so a kind moving either way fails here.
        ///
        /// Every kind whose key is the decoded Arrow value agrees by
        /// construction — a float's too, its `-0` made `0` before the
        /// kernels see it; `MacAddr` agrees because `macaddr_out` writes
        /// fixed-width lowercase hex, whose bytes order as the octets do. The
        /// seven that do not, each with the pair the walk first meets:
        ///
        /// - `Enum`, emitted `Dictionary`: Arrow compares the label text, not
        ///   the declaration order (I33).
        /// - `Interval`, emitted `Interval(MonthDayNano)`: Arrow compares
        ///   months, days and nanoseconds field by field, so `30 days` is
        ///   below `1 mon` where `interval_cmp_value` equates them (I40).
        /// - `Numeric`, `TimeTz`, `Network`, `Jsonb`, emitted `Utf8View`: Arrow
        ///   compares the text bytewise.
        /// - `PaddedText`, emitted `Utf8View` with its padding: Arrow compares
        ///   the padded text, so a byte below the blank orders differently —
        ///   and a stored bound, being unpadded, is no value of the column.
        const ARROW_AGREEMENT: &[(&str, bool, bool)] = &[
            ("Bool", true, true),
            ("Bytea", true, true),
            ("Date", true, true),
            ("Decimal", true, true),
            ("Enum", false, false),
            ("Float32", true, true),
            ("Float64", true, true),
            ("Int", true, true),
            ("Interval", false, false),
            ("Jsonb", false, false),
            ("MacAddr", true, true),
            ("Network", false, false),
            ("Numeric", false, false),
            ("PaddedText", false, false),
            ("Text", true, true),
            ("Time", true, true),
            ("TimeTz", false, false),
            ("Timestamp", true, true),
            ("UnsignedInt", true, true),
            ("Uuid", true, true),
        ];

        /// Values added to the oracle's for a kind whose committed population
        /// happens to order alike under both comparisons, each in the form its
        /// type's `*_out` writes: every IPv4 `inet`/`cidr` case there has a
        /// first octet of two digits or more, so none reaches where a
        /// network's order and its text's part — `9.` against `10.`.
        const SUPPLEMENT: &[(&str, &str)] = &[("inet", "9.0.0.1"), ("cidr", "9.0.0.0/8")];

        /// A stable name per [`CompareKind`] variant, exhaustive so a new kind
        /// cannot go unrecorded in [`ARROW_AGREEMENT`]. The one only
        /// [`CompareKind::arrow_order`] produces is named and never walked.
        fn kind_name(kind: &CompareKind) -> &'static str {
            match kind {
                CompareKind::Bool => "Bool",
                CompareKind::Int => "Int",
                CompareKind::UnsignedInt => "UnsignedInt",
                CompareKind::Float32 => "Float32",
                CompareKind::Float64 => "Float64",
                CompareKind::Decimal(_) => "Decimal",
                CompareKind::Date => "Date",
                CompareKind::Time => "Time",
                CompareKind::Timestamp { .. } => "Timestamp",
                CompareKind::Uuid => "Uuid",
                CompareKind::Bytea => "Bytea",
                CompareKind::Text => "Text",
                CompareKind::PaddedText => "PaddedText",
                CompareKind::Numeric { .. } => "Numeric",
                CompareKind::Enum(_) => "Enum",
                CompareKind::Interval => "Interval",
                CompareKind::TimeTz => "TimeTz",
                CompareKind::Network { .. } => "Network",
                CompareKind::MacAddr { .. } => "MacAddr",
                CompareKind::Jsonb => "Jsonb",
                CompareKind::IntervalFields => "IntervalFields",
            }
        }

        /// DataFusion's order of every element of `array` against element
        /// `j`, read off the `lt`, `eq` and `gt` kernels, or why it has none: a
        /// kernel refusing the type, or an element answering other than
        /// exactly one of the three.
        ///
        /// DataFusion's `apply_cmp` makes a float's `-0` into `0` before the
        /// kernels run (`normalize_float_zero`, `datafusion-common` 55), so
        /// this does too; the kernels alone order `-0` below `0`. The check
        /// against DataFusion itself is the provider crate's, the library
        /// taking no DataFusion dependency.
        fn arrow_against(
            array: &arrow::array::ArrayRef,
            j: usize,
        ) -> std::result::Result<Vec<Ordering>, String> {
            use arrow::array::{Array, AsArray, Scalar};
            use arrow::compute::kernels::cmp::{eq, gt, lt};
            use arrow::datatypes::{DataType, Float32Type, Float64Type};
            let array: arrow::array::ArrayRef = match array.data_type() {
                DataType::Float32 => std::sync::Arc::new(
                    array.as_primitive::<Float32Type>().unary::<_, Float32Type>(|v| v + 0.0),
                ),
                DataType::Float64 => std::sync::Arc::new(
                    array.as_primitive::<Float64Type>().unary::<_, Float64Type>(|v| v + 0.0),
                ),
                _ => array.clone(),
            };
            let array = &array;
            let scalar = Scalar::new(array.slice(j, 1));
            let kernel = |r: std::result::Result<arrow::array::BooleanArray, _>| {
                r.map_err(|e: arrow::error::ArrowError| e.to_string())
            };
            let (lt, eq, gt) = (
                kernel(lt(array, &scalar))?,
                kernel(eq(array, &scalar))?,
                kernel(gt(array, &scalar))?,
            );
            (0..array.len())
                .map(|i| match (lt.value(i), eq.value(i), gt.value(i)) {
                    (true, false, false) => Ok(Ordering::Less),
                    (false, true, false) => Ok(Ordering::Equal),
                    (false, false, true) => Ok(Ordering::Greater),
                    other => Err(format!("element {i} against {j} answers lt/eq/gt {other:?}")),
                })
                .collect()
        }

        /// One kind's findings over every declared type compared by it.
        #[derive(Default)]
        struct Agreement {
            declared: BTreeSet<String>,
            arrow_types: BTreeSet<String>,
            pairs: usize,
            groups: usize,
            /// The first pair the two orders answer differently, or the
            /// first reason Arrow gave none.
            order: Option<String>,
            /// The first group whose stored bounds are not Arrow's extremes.
            bounds: Option<String>,
        }

        /// **Which kinds already order as DataFusion orders the emitted
        /// array**, over every value the server wrote for every declared type
        /// the register compares as a scalar, on six majors: each pair's
        /// [`ValueKey`] order against [`arrow_against`]'s answer on the array
        /// a batch builds from the same texts, and each gathered group's
        /// stored bounds against the group's Arrow minimum and maximum.
        ///
        /// A value the emitted type cannot hold — a `date`'s `infinity`, a
        /// `numeric(p,s)`'s `NaN` (`KD8`) — is left out: no array carries it
        /// for Arrow to order.
        #[tokio::test]
        async fn each_kind_s_order_and_bounds_against_arrow_s() {
            let mut rng = Seeded(0x5EED_0006_0001);
            let mut found: BTreeMap<&'static str, Agreement> = BTreeMap::new();
            for major in MAJORS {
                let types = types_of(major).await;
                let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    if let (Some(declared), Some("ok"), Some(output)) =
                        (row[0].clone(), row[2].as_deref(), row[3].clone())
                    {
                        outputs.entry(declared).or_default().insert(output);
                    }
                }
                for (declared, value) in SUPPLEMENT {
                    outputs.entry(declared.to_string()).or_default().insert(value.to_string());
                }
                for (declared, values) in outputs {
                    let resolved = schema(&declared, None, &types);
                    let ComparisonPlan::Compared { kind, .. } = &resolved.comparisons[0] else {
                        continue;
                    };
                    let data_type = resolved.schema.field(0).data_type();
                    let plan = &resolved.plans[0];
                    let held: Vec<&str> = values
                        .iter()
                        .map(String::as_str)
                        .filter(|v| crate::batch::column_of(data_type, plan, &[v]).is_ok())
                        .collect();
                    if held.is_empty() {
                        continue;
                    }
                    let agreement = found.entry(kind_name(kind)).or_default();
                    agreement.declared.insert(declared.clone());
                    agreement.arrow_types.insert(format!("{data_type}"));
                    let key = |v: &str| {
                        ValueKey::of(kind, v)
                            .unwrap_or_else(|| panic!("{major} {declared}: the server wrote {v:?}"))
                    };
                    let array = crate::batch::column_of(data_type, plan, &held).unwrap();
                    for (j, b) in held.iter().enumerate() {
                        let arrow = match arrow_against(&array, j) {
                            Ok(arrow) => arrow,
                            Err(e) => {
                                agreement.order.get_or_insert(format!("{major} {declared}: {e}"));
                                break;
                            }
                        };
                        for (i, a) in held.iter().enumerate() {
                            agreement.pairs += 1;
                            let ours = key(a).compare(&key(b));
                            if ours != arrow[i] {
                                agreement.order.get_or_insert(format!(
                                    "{major} {declared}: {a:?} against {b:?} is {ours:?} here and \
                                     {:?} in Arrow",
                                    arrow[i]
                                ));
                            }
                        }
                    }
                    // The whole set as one group, and seeded small ones.
                    let mut groups = vec![held.clone()];
                    groups.extend(
                        (0..30).map(|_| (0..1 + rng.below(5)).map(|_| *rng.pick(&held)).collect()),
                    );
                    for group in groups {
                        let Some(stored) = crate::gather::one_group_bounds(kind.clone(), &group)
                        else {
                            continue;
                        };
                        agreement.groups += 1;
                        let mut with_bounds = group.clone();
                        with_bounds.extend([stored.min.as_str(), stored.max.as_str()]);
                        let array = match crate::batch::column_of(data_type, plan, &with_bounds) {
                            Ok(array) => array,
                            Err(v) => {
                                agreement.bounds.get_or_insert(format!(
                                    "{major} {declared}: stored bound {v:?} is not a value of \
                                     {data_type}"
                                ));
                                continue;
                            }
                        };
                        let n = group.len();
                        let problem = (|| {
                            let (min, max) =
                                (arrow_against(&array, n)?, arrow_against(&array, n + 1)?);
                            // Arrow's extremes are the bounds when the bound
                            // is at or below (above) every value and equal to
                            // one of them.
                            let below = min[..n].iter().all(|o| o.is_ge());
                            let above = max[..n].iter().all(|o| o.is_le());
                            let min_held = min[..n].contains(&Ordering::Equal);
                            let max_held = max[..n].contains(&Ordering::Equal);
                            if below && above && min_held && (max_held || !stored.max_exact) {
                                Ok(())
                            } else {
                                Err(format!(
                                    "{:?} and {:?} over {group:?}: below {below}, above {above}, \
                                     min held {min_held}, max held {max_held}",
                                    stored.min, stored.max
                                ))
                            }
                        })();
                        if let Err(e) = problem {
                            agreement.bounds.get_or_insert(format!("{major} {declared}: {e}"));
                        }
                    }
                }
            }
            let table: Vec<(&str, bool, bool)> = found
                .iter()
                .map(|(name, a)| (*name, a.order.is_none(), a.bounds.is_none()))
                .collect();
            let report: Vec<String> = found
                .iter()
                .map(|(name, a)| {
                    format!(
                        "  {name}: {} types as {:?}, {} pairs, {} groups\n    order: {}\n    bounds: {}",
                        a.declared.len(),
                        a.arrow_types,
                        a.pairs,
                        a.groups,
                        a.order.as_deref().unwrap_or("agrees"),
                        a.bounds.as_deref().unwrap_or("agree"),
                    )
                })
                .collect();
            assert_eq!(table, ARROW_AGREEMENT, "\n{}", report.join("\n"));
        }

        /// **Under [`ComparisonSemantics::Arrow`] every comparing operator
        /// answers as DataFusion does** ([`arrow_against`]) over the array a
        /// batch builds, over every value the server wrote for every declared
        /// type the register compares as a scalar, on six majors, each value
        /// in turn the literal — and announces nothing. A nested column refuses
        /// every comparing operator instead.
        ///
        /// It also holds the rule for which stored bounds Arrow's semantics
        /// reads ([`ComparisonPlan::bounds_ordered_in`]) to the evidence
        /// [`ARROW_AGREEMENT`] records: a kind [`CompareKind::arrow_order`]
        /// leaves alone is one whose order and gathered bounds were found
        /// to be DataFusion's.
        #[tokio::test]
        async fn arrow_semantics_answers_as_datafusion_does() {
            const OPS: [PredicateOp; 8] = [
                PredicateOp::Eq,
                PredicateOp::Ne,
                PredicateOp::Lt,
                PredicateOp::Le,
                PredicateOp::Gt,
                PredicateOp::Ge,
                PredicateOp::IsDistinctFrom,
                PredicateOp::IsNotDistinctFrom,
            ];
            let arrow_term = |p: &Predicate, resolved: &ResolvedSchema| {
                crate::predicate::resolve_term(p, 0, resolved, 0, ComparisonSemantics::Arrow)
            };
            let mut kinds: BTreeMap<&'static str, CompareKind> = BTreeMap::new();
            let (mut answered, mut nested_refused) = (0usize, 0usize);
            for major in MAJORS {
                let types = types_of(major).await;
                let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    if let (Some(declared), Some("ok"), Some(output)) =
                        (row[0].clone(), row[2].as_deref(), row[3].clone())
                    {
                        outputs.entry(declared).or_default().insert(output);
                    }
                }
                for (declared, value) in SUPPLEMENT {
                    outputs.entry(declared.to_string()).or_default().insert(value.to_string());
                }
                for (declared, values) in outputs {
                    let resolved = schema(&declared, None, &types);
                    let kind = match &resolved.comparisons[0] {
                        ComparisonPlan::Compared { kind, .. } => kind.clone(),
                        ComparisonPlan::Nested(_) => {
                            let literal = values.first().cloned();
                            for op in OPS {
                                let p =
                                    Predicate { column: "v".into(), op, value: literal.clone() };
                                assert!(
                                    matches!(
                                        arrow_term(&p, &resolved),
                                        Err(Error::UncomparablePredicateColumn { .. })
                                    ),
                                    "{major} {declared} {op:?}"
                                );
                                nested_refused += 1;
                            }
                            continue;
                        }
                        _ => continue,
                    };
                    let data_type = resolved.schema.field(0).data_type();
                    let plan = &resolved.plans[0];
                    let held: Vec<&str> = values
                        .iter()
                        .map(String::as_str)
                        .filter(|v| crate::batch::column_of(data_type, plan, &[v]).is_ok())
                        .collect();
                    if held.is_empty() {
                        continue;
                    }
                    kinds.insert(kind_name(&kind), kind);
                    let array = crate::batch::column_of(data_type, plan, &held).unwrap();
                    for (j, b) in held.iter().enumerate() {
                        let arrow = arrow_against(&array, j)
                            .unwrap_or_else(|e| panic!("{major} {declared}: {e}"));
                        for op in OPS {
                            let p =
                                Predicate { column: "v".into(), op, value: Some(b.to_string()) };
                            let term = arrow_term(&p, &resolved)
                                .unwrap_or_else(|e| panic!("{major} {declared} {op:?} {b:?}: {e}"));
                            assert!(term.comparison_notes().is_empty(), "{major} {declared}");
                            for (i, a) in held.iter().enumerate() {
                                let ord = arrow[i];
                                let want = match op {
                                    PredicateOp::Eq | PredicateOp::IsNotDistinctFrom => ord.is_eq(),
                                    PredicateOp::Ne | PredicateOp::IsDistinctFrom => ord.is_ne(),
                                    PredicateOp::Lt => ord.is_lt(),
                                    PredicateOp::Le => ord.is_le(),
                                    PredicateOp::Gt => ord.is_gt(),
                                    _ => ord.is_ge(),
                                };
                                assert_eq!(
                                    term.eval_value(Some(a)),
                                    Some(Truth::of(want)),
                                    "{major} {declared}: {a:?} {} {b:?}",
                                    op.symbol()
                                );
                                answered += 1;
                            }
                        }
                    }
                }
            }
            let walked: Vec<&str> = kinds.keys().copied().collect();
            let recorded: Vec<&str> = ARROW_AGREEMENT.iter().map(|(name, ..)| *name).collect();
            assert_eq!(walked, recorded, "every kind the register compares is walked");
            for (name, order, bounds) in ARROW_AGREEMENT {
                assert_eq!(
                    kinds[name].arrow_divergence().is_none(),
                    kinds[name].arrow_order() == kinds[name],
                    "{name}: a kind Arrow semantics moves reports a divergence, and only one"
                );
                if kinds[name].arrow_order() == kinds[name] {
                    assert!(*order && *bounds, "{name} keeps its kind and disagrees with Arrow");
                }
            }
            assert!(answered > 40_000 && nested_refused > 0, "{answered} {nested_refused}");
        }

        /// Every `(declared type, COLLATE clause)` case of every major's
        /// `comparisons.tsv`, resolved as a one-column schema, with the output
        /// spelling of every literal the server accepted for its type.
        async fn cases(major: u32) -> Vec<(String, Option<&'static str>, ResolvedSchema)> {
            let types = types_of(major).await;
            let mut seen = BTreeSet::new();
            let mut out = Vec::new();
            for row in rows(&fixture(major, "oracle/comparisons.tsv")) {
                let declared = row[0].clone().expect("a case names a type");
                let collation = clause(row[3].as_ref());
                if seen.insert((declared.clone(), collation)) {
                    let resolved = schema(&declared, collation, &types);
                    out.push((declared, collation, resolved));
                }
            }
            out
        }

        /// **Under PostgreSQL's semantics a column reports exactly what a term
        /// on it would announce**, under `=` and under `<` — the two operator
        /// families — over every case the oracle holds on six majors. The
        /// report is written apart from [`resolve_term`], so this is what
        /// holds the two to one answer.
        #[tokio::test]
        async fn a_column_reports_what_its_terms_announce() {
            let mut compared = 0usize;
            for major in MAJORS {
                let mut outputs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    if let (Some(declared), Some("ok"), Some(output)) =
                        (row[0].clone(), row[2].as_deref(), row[3].clone())
                    {
                        outputs.entry(declared).or_default().insert(output);
                    }
                }
                for (declared, collation, resolved) in cases(major).await {
                    // The first literal both families decode: a refusal of
                    // the operator is an answer, a literal that does not key
                    // is not.
                    let announced = outputs.get(&declared).into_iter().flatten().find_map(|v| {
                        let mut notes = Vec::new();
                        for op in [PredicateOp::Eq, PredicateOp::Lt] {
                            let p = Predicate { column: "v".into(), op, value: Some(v.clone()) };
                            match resolve_term(&p, 0, &resolved, 0) {
                                Ok(term) => notes.extend(term.comparison_notes()),
                                Err(Error::PredicateValueDecode { .. }) => return None,
                                Err(_) => {}
                            }
                        }
                        Some(notes)
                    });
                    let Some(announced) = announced else { continue };
                    let reported = column_divergences(&resolved, ComparisonSemantics::Postgres);
                    let context = format!("{major} {declared} {collation:?}");
                    assert!(
                        announced.iter().all(|n| reported.contains(n)),
                        "{context}: {announced:?} not all in {reported:?}"
                    );
                    assert!(
                        reported.iter().all(|n| announced.contains(n)),
                        "{context}: {reported:?} not all announced ({announced:?})"
                    );
                    compared += 1;
                }
            }
            assert!(compared > 300, "only {compared} cases compared");
        }

        /// **Under Arrow's semantics a column that reports nothing answers as
        /// the server does**: every oracle cell a term in Arrow's semantics
        /// answers differently from PostgreSQL falls on a column reporting a
        /// divergence that reaches the cell's operator, or on one that fell
        /// back to text, whose column note is its finding. A nested column,
        /// whose every term is refused here, is answered as DataFusion
        /// answers it — `make_comparator` over the arrays a batch builds —
        /// and asserted to report its order, so its equality cells hold the
        /// order note to order alone. The other direction is not asserted — a
        /// report is allowed to announce a divergence this population never
        /// exercises.
        #[tokio::test]
        async fn a_column_reporting_no_arrow_divergence_answers_as_the_server() {
            let (mut asserted, mut disagreed) = (0usize, BTreeSet::new());
            let mut nested_asserted = 0usize;
            for major in MAJORS {
                let outputs: BTreeMap<(String, String), String> =
                    rows(&fixture(major, "oracle/literals.tsv"))
                        .into_iter()
                        .filter_map(|r| Some(((r[0].clone()?, r[1].clone()?), r[3].clone()?)))
                        .collect();
                let resolved: BTreeMap<(String, Option<&str>), ResolvedSchema> = cases(major)
                    .await
                    .into_iter()
                    .map(|(declared, collation, resolved)| ((declared, collation), resolved))
                    .collect();
                for row in rows(&fixture(major, "oracle/comparisons.tsv")) {
                    let declared = row[0].clone().expect("a case names a type");
                    let collation = clause(row[3].as_ref());
                    let schema = &resolved[&(declared.clone(), collation)];
                    let report = column_divergences(schema, ComparisonSemantics::Arrow);
                    if matches!(schema.comparisons[0], ComparisonPlan::Nested(_)) {
                        assert!(
                            report
                                .iter()
                                .any(|n| n.divergence == ComparisonDivergence::NestedArrowOrder),
                            "{major} {declared}: {report:?}"
                        );
                    }
                    let output =
                        |literal: &str| outputs.get(&(declared.clone(), literal.to_string()));
                    let (Some(left), Some(right)) = (row[1].as_deref(), row[2].as_deref()) else {
                        continue;
                    };
                    let (Some(field), Some(bound)) = (output(left), output(right)) else {
                        continue;
                    };
                    // A nested column's every term is refused here, so its
                    // answer is DataFusion's own: `make_comparator` over the
                    // two values as a batch emits them.
                    let nested = (schema.plans[0] != NestedPlan::Scalar
                        && schema.columns[0] == ColumnResolution::Mapped)
                        .then(|| {
                            let array = crate::batch::column_of(
                                schema.schema.field(0).data_type(),
                                &schema.plans[0],
                                &[field, bound],
                            )
                            .ok()?;
                            let cmp =
                                arrow::array::make_comparator(&array, &array, Default::default())
                                    .ok()?;
                            Some(cmp(0, 1))
                        });
                    for (offset, op) in ASSERTED {
                        let cell = row[4 + offset].as_deref().expect("a cell is never NULL");
                        let expected = match cell {
                            "t" => Truth::True,
                            "f" => Truth::False,
                            _ => continue,
                        };
                        let got = match nested {
                            Some(None) => continue,
                            Some(Some(ord)) => {
                                nested_asserted += 1;
                                Truth::of(match op {
                                    PredicateOp::Eq => ord.is_eq(),
                                    PredicateOp::Ne => ord.is_ne(),
                                    PredicateOp::Lt => ord.is_lt(),
                                    PredicateOp::Le => ord.is_le(),
                                    PredicateOp::Gt => ord.is_gt(),
                                    _ => ord.is_ge(),
                                })
                            }
                            None => {
                                let p = Predicate {
                                    column: "v".into(),
                                    op,
                                    value: Some(bound.clone()),
                                };
                                let Ok(term) = crate::predicate::resolve_term(
                                    &p,
                                    0,
                                    schema,
                                    0,
                                    ComparisonSemantics::Arrow,
                                ) else {
                                    continue;
                                };
                                let Ok(got) = term.eval(
                                    RawRow::unchecked(&encode_field(Some(field))),
                                    &mut RowSplit::default(),
                                    "public.t",
                                    0,
                                ) else {
                                    continue;
                                };
                                got
                            }
                        };
                        asserted += 1;
                        if got != expected {
                            // A column that fell back to text is reported by
                            // its `Warning` column note, and nothing else.
                            let fell_back = schema.columns[0] != ColumnResolution::Mapped;
                            assert!(
                                fell_back
                                    || report.iter().any(|n| {
                                        op.is_ordering() || n.divergence.affects_equality()
                                    }),
                                "{major} {declared} {collation:?}: {left:?} {} {right:?} is \
                                 {got:?} in Arrow semantics and {cell} on the server, and the \
                                 column reports {report:?}",
                                op.symbol()
                            );
                            disagreed.insert(declared.clone());
                        }
                    }
                }
            }
            // A floor on both counts: a walk that asserted nothing, or found
            // no disagreement at all, would prove nothing about the report.
            assert!(asserted > 40_000, "only {asserted} cells asserted");
            assert!(nested_asserted > 1_000, "only {nested_asserted} nested cells asserted");
            assert!(disagreed.len() >= 8, "only {disagreed:?} disagreed");
        }

        /// A range or multirange literal is put into the form the server
        /// stores it in *before* it is compared, checked against the server's
        /// own rewriting of every such literal in `literals.tsv`.
        ///
        /// **The cell walk above cannot reach this**, and that is why this
        /// test exists rather than one more assertion inside it: it puts both
        /// operands of every cell through `outputs` by construction, so both
        /// sides arrive already canonical and a build that rewrote nothing
        /// would pass. The input spellings are only in `literals.tsv` — and
        /// they are exactly the interesting ones, `int4range '[1,10]'` being
        /// the value the file holds as `[1,11)`.
        ///
        /// So each accepted row is asked `<output> = <input>`, which is
        /// `True` only where the two are one value here as they are on the
        /// server, and each refused row is asked for the refusal — which is
        /// where `[10,1)` lands, a `22000` the container grammar cannot see
        /// (I44) and [`make_range`] raises.
        #[tokio::test]
        async fn a_range_literal_is_canonicalized_before_it_is_compared() {
            let mut asked = 0usize;
            let mut rewritten = 0usize;
            for major in MAJORS {
                let types = types_of(major).await;
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    let declared = row[0].clone().expect("a case names a type");
                    if !matches!(
                        comparison_for(&declared, None, &types, &[]),
                        ComparisonPlan::Nested(
                            NestedCompare::Range { .. } | NestedCompare::Multirange { .. }
                        )
                    ) {
                        continue;
                    }
                    // A SQL NULL input is the `\N` field, not a literal.
                    let Some(input) = row[1].clone() else { continue };
                    let status = row[2].as_deref().expect("a case records its status");
                    // The type does not exist at this major — a user range's
                    // multirange companion before v14 (I10).
                    if status == "E42704" {
                        continue;
                    }
                    asked += 1;
                    let ask = |field: &str| {
                        answer(&declared, None, &types, PredicateOp::Eq, Some(field), &input).0
                    };
                    let Some(output) = row[3].as_deref().filter(|_| status == "ok") else {
                        // The field is never read: the literal is refused
                        // when the block's schema resolves, before a row.
                        assert!(
                            ask("empty").is_err(),
                            "{major} {declared}: accepted {input:?}, which the server refused \
                             with {status}"
                        );
                        continue;
                    };
                    assert_eq!(
                        ask(output),
                        Ok(Truth::True),
                        "{major} {declared}: {input:?} is not the value the server stored it as \
                         ({output:?})"
                    );
                    if input != output {
                        rewritten += 1;
                    }
                }
            }
            // Floors, not counts, and the second is the one that matters: a
            // build that canonicalized nothing would still satisfy the first,
            // most rows being already written the way the server stores them.
            assert!(asked > 150, "only {asked} range literals asserted");
            assert!(rewritten > 30, "only {rewritten} of them needed rewriting");
        }

        /// SplitMix64, seeded, so a failing group is the same group on every
        /// run and on every machine.
        struct Seeded(u64);

        impl Seeded {
            fn next(&mut self) -> u64 {
                self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.0;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^ (z >> 31)
            }

            fn below(&mut self, n: usize) -> usize {
                (self.next() % n as u64) as usize
            }

            fn chance(&mut self, one_in: usize) -> bool {
                self.below(one_in) == 0
            }

            fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
                &items[self.below(items.len())]
            }
        }

        const OPERATORS: [PredicateOp; 10] = [
            PredicateOp::Eq,
            PredicateOp::Ne,
            PredicateOp::IsNull,
            PredicateOp::IsNotNull,
            PredicateOp::Lt,
            PredicateOp::Le,
            PredicateOp::Gt,
            PredicateOp::Ge,
            PredicateOp::IsDistinctFrom,
            PredicateOp::IsNotDistinctFrom,
        ];

        /// A random tree of at most `depth` levels over `terms`, `And`/`Or`
        /// of arity zero to three.
        fn random_tree(rng: &mut Seeded, depth: usize, terms: &[ResolvedTerm]) -> ResolvedExpr {
            let arity = |rng: &mut Seeded| rng.below(4);
            match if depth == 0 { 0 } else { rng.below(4) } {
                0 => ResolvedExpr::Term(rng.pick(terms).clone()),
                1 => ResolvedExpr::And(
                    (0..arity(rng)).map(|_| random_tree(rng, depth - 1, terms)).collect(),
                ),
                2 => ResolvedExpr::Or(
                    (0..arity(rng)).map(|_| random_tree(rng, depth - 1, terms)).collect(),
                ),
                _ => ResolvedExpr::Not(Box::new(random_tree(rng, depth - 1, terms))),
            }
        }

        /// **The truth-set evaluator never rules out an answer a row gives**,
        /// over every value the server wrote for every declared type in the
        /// committed oracle, on six majors: seeded random row groups — NULLs,
        /// repeats, no rows at all — with their statistics taken off the rows
        /// and then weakened the ways a stored statistic can be (a bound
        /// loosened to another value on its side, a count or a dictionary
        /// missing), under random terms and random `And`/`Or`/`Not` trees.
        /// Every row's exact answer through the row evaluator has to be in
        /// the group's set.
        ///
        /// **And it rules out everything it can where the statistics are
        /// exact**, which is what keeps the first half from passing on an
        /// evaluator that answers every set: a single ordering term over
        /// unloosened bounds on a column that orders exactly, and a single
        /// equality term over a dictionary on a column that equates exactly,
        /// each have to answer precisely the set its rows produce.
        ///
        /// Bounds are offered to every column they can be keyed for, exact or
        /// not; which ones a term believes is the evaluator's to decide.
        #[tokio::test]
        async fn a_groups_truths_hold_every_rows_answer() {
            let mut rng = Seeded(0x5EED_0010_0003);
            let (mut answers, mut ruled_out, mut exact_sets) = (0usize, 0usize, 0usize);
            for major in MAJORS {
                let types = types_of(major).await;
                let mut values: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                let mut literals: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    let (Some(declared), Some("ok")) = (row[0].clone(), row[2].as_deref()) else {
                        continue;
                    };
                    if let Some(output) = row[3].clone() {
                        values.entry(declared.clone()).or_default().insert(output.clone());
                        literals.entry(declared.clone()).or_default().insert(output);
                    }
                    if let Some(input) = row[1].clone() {
                        literals.entry(declared).or_default().insert(input);
                    }
                }
                let cases: BTreeSet<(String, Option<&str>)> =
                    rows(&fixture(major, "oracle/comparisons.tsv"))
                        .into_iter()
                        .map(|row| {
                            (row[0].clone().expect("a case names a type"), clause(row[3].as_ref()))
                        })
                        .collect();
                for (declared, collation) in cases {
                    let Some(pool) = values.get(&declared) else { continue };
                    let pool: Vec<&str> = pool.iter().map(String::as_str).collect();
                    let literals: Vec<&str> =
                        literals[&declared].iter().map(String::as_str).collect();
                    let resolved = schema(&declared, collation, &types);
                    let (key_kind, orders_exactly) = match &resolved.comparisons[0] {
                        ComparisonPlan::Compared { kind, divergence } => {
                            let keys = pool.iter().all(|v| order_key(kind, v).is_some());
                            (keys.then_some(kind.clone()), divergence.is_none())
                        }
                        _ => (None, false),
                    };
                    let mut terms = Vec::new();
                    for _ in 0..40 {
                        let op = *rng.pick(&OPERATORS);
                        let value = match op {
                            PredicateOp::IsNull | PredicateOp::IsNotNull => None,
                            _ => Some(rng.pick(&literals).to_string()),
                        };
                        let p = Predicate { column: "v".into(), op, value };
                        if let Ok(term) = resolve_term(&p, 0, &resolved, 0) {
                            terms.push(term);
                        }
                    }
                    if terms.is_empty() {
                        continue;
                    }
                    for _ in 0..30 {
                        let group_rows: Vec<Option<&str>> = (0..rng.below(6))
                            .map(|_| (!rng.chance(4)).then(|| *rng.pick(&pool)))
                            .collect();
                        let present: Vec<&str> = group_rows.iter().flatten().copied().collect();
                        let nulls = group_rows.len() - present.len();
                        let key = |kind: &CompareKind, v: &str| order_key(kind, v).unwrap();
                        let mut loosened = false;
                        let bounds =
                            key_kind.as_ref().filter(|_| !present.is_empty()).map(|kind| {
                                let cmp =
                                    |a: &str, b: &str| compare_keys(&key(kind, a), &key(kind, b));
                                let mut min = present
                                    .iter()
                                    .copied()
                                    .reduce(|a, b| if cmp(b, a).is_lt() { b } else { a })
                                    .unwrap();
                                let mut max = present
                                    .iter()
                                    .copied()
                                    .reduce(|a, b| if cmp(b, a).is_gt() { b } else { a })
                                    .unwrap();
                                if rng.chance(3) {
                                    let below: Vec<&str> = pool
                                        .iter()
                                        .copied()
                                        .filter(|v| cmp(v, min).is_lt())
                                        .collect();
                                    if !below.is_empty() {
                                        min = rng.pick(&below);
                                        loosened = true;
                                    }
                                }
                                if rng.chance(3) {
                                    let above: Vec<&str> = pool
                                        .iter()
                                        .copied()
                                        .filter(|v| cmp(v, max).is_gt())
                                        .collect();
                                    if !above.is_empty() {
                                        max = rng.pick(&above);
                                        loosened = true;
                                    }
                                }
                                (min.to_string(), max.to_string())
                            });
                        let dictionary: Option<Vec<String>> = (!present.is_empty()).then(|| {
                            present
                                .iter()
                                .map(|v| v.to_string())
                                .collect::<BTreeSet<_>>()
                                .into_iter()
                                .collect()
                        });
                        let full = Group {
                            rows: group_rows.len() as u64,
                            nulls: Some(nulls as u64),
                            bounds: bounds.clone(),
                            dictionary: dictionary.clone(),
                        };
                        let weakened = Group {
                            rows: full.rows,
                            nulls: full.nulls.filter(|_| !rng.chance(8)),
                            bounds: full.bounds.clone().filter(|_| !rng.chance(4)),
                            dictionary: full.dictionary.clone().filter(|_| !rng.chance(4)),
                        };
                        let row_answers = |expr: &ResolvedExpr| {
                            group_rows
                                .iter()
                                .map(|v| {
                                    expr.eval(
                                        true,
                                        RawRow::unchecked(&encode_field(*v)),
                                        &mut RowSplit::default(),
                                        "public.t",
                                        0,
                                    )
                                    .unwrap_or_else(|e| {
                                        panic!("{major} {declared}: the server wrote {v:?}: {e}")
                                    })
                                })
                                .fold(TruthSet::EMPTY, |set, t| set.union(TruthSet::of(t)))
                        };
                        let trees: Vec<ResolvedExpr> = terms
                            .iter()
                            .cloned()
                            .map(ResolvedExpr::Term)
                            .chain((0..6).map(|_| random_tree(&mut rng, 3, &terms)))
                            .collect();
                        for expr in &trees {
                            let produced = row_answers(expr);
                            for group in [&full, &weakened] {
                                let set = expr.truths(group);
                                assert_eq!(
                                    set.union(produced),
                                    set,
                                    "{major} {declared} {collation:?}: {expr:?} over {group_rows:?} \
                                     with {group:?} answered {set:?}"
                                );
                                answers += group_rows.len();
                                if !set.contains(Truth::True) && !group_rows.is_empty() {
                                    ruled_out += 1;
                                }
                            }
                            let ResolvedExpr::Term(term) = expr else { continue };
                            let exact = if term.op.is_ordering() {
                                orders_exactly && !loosened && bounds.is_some()
                            } else {
                                !matches!(term.op, PredicateOp::IsNull | PredicateOp::IsNotNull)
                                    && matches!(
                                        resolved.comparisons[0],
                                        ComparisonPlan::Compared { .. }
                                    )
                                    && term.comparison_notes().is_empty()
                                    && dictionary.is_some()
                            };
                            if exact {
                                let only = Group {
                                    bounds: full.bounds.clone().filter(|_| term.op.is_ordering()),
                                    dictionary: full
                                        .dictionary
                                        .clone()
                                        .filter(|_| !term.op.is_ordering()),
                                    ..full.clone()
                                };
                                assert_eq!(
                                    expr.truths(&only),
                                    produced,
                                    "{major} {declared} {collation:?}: {expr:?} over {group_rows:?} is not exact"
                                );
                                exact_sets += 1;
                            }
                        }
                    }
                }
            }
            // Floors, not counts: the first is the property's reach, the
            // other two are what keep it from being met by answering
            // everything.
            assert!(answers > 1_000_000, "only {answers} row answers held");
            assert!(ruled_out > 75_000, "only {ruled_out} groups ruled out");
            assert!(exact_sets > 50_000, "only {exact_sets} sets asserted exact");
        }
    }
}
