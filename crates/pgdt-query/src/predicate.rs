//! Post-parse row filtering (`docs/design/decisions.md`, "Predicates"): the
//! filter an embedder builds, and beneath it one tree — the filter
//! [`resolve`]d against a block, a row's three-valued verdict ([`eval`]), a
//! row group's ([`truths`]), and what a comparison says where it is not the
//! server's ([`notes`]).

mod eval;
mod notes;
mod resolve;
mod truths;

use pgdt_codec::compare::ComparisonSemantics;

pub use notes::ComparisonNote;
pub(crate) use notes::column_divergences;
pub(crate) use resolve::{
    ResolvedExpr, ResolvedTerm, check_columns, resolve_expr, resolve_loosened,
};
pub(crate) use truths::GroupStatistics;

/// A filter term's operator, carrying the operand it compares against
/// ([`Self::operand`]).
///
/// The eight comparing operators each hold their literal, the text a
/// `pgdt query --filter` value or a DataFusion literal spells; the two
/// memberships hold their list; the NULL and unrepresentable tests hold
/// none. So a test cannot be given a value and a comparison cannot be built
/// without one.
///
/// Every operator but the NULL and unrepresentable tests compares typed,
/// through the column's own [`ComparisonPlan`](pgdt_codec::compare::ComparisonPlan): the four ordering operators decode both sides and
/// compare the values, `Eq`/`Ne` take the cheapest of three canonicalizations
/// that gives the server's answer for that column ([`literal_equality`](pgdt_codec::internal::literal_equality)).
/// A nested column with a plan is compared structurally under every operator,
/// and a query's DataFusion semantics refuses one under every operator instead.
/// Where the register has no plan for a column — it did not resolve, one of
/// its nested positions is not compared, or this build orders its type not at
/// all — `Eq`/`Ne` compare the canonical `*_out` text the file holds and the
/// ordering operators are refused, where a query's DataFusion semantics orders a
/// scalar one bytewise instead.
///
/// The value is read with the column's own decoder wherever the register
/// gives the column a comparison, whatever the operator, once when the
/// block's schema resolves rather than per row — so a literal that is not a
/// value of the column's type is `Error::PredicateValueDecode` before any row
/// is read, and both sides of a `numeric(p,s)` comparison held as a decimal
/// carry that column's scale. An ordering operator keeps the decoded key and
/// decodes the field to match; `Eq`/`Ne` usually keep the literal rendered
/// back into the `*_out` spelling the file holds and compare bytes
/// ([`literal_equality`](pgdt_codec::internal::literal_equality)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateOp {
    Eq(String),
    Ne(String),
    /// `col IS NULL` — matches a NULL field (`\N`), and under
    /// [`pgdt_codec::UnrepresentableMode::Null`] a value the query reads as NULL,
    /// which [`Self::IsUnrepresentable`] tells apart. Rounds out the NULL
    /// semantics `Eq`/`Ne` deliberately can't express: a NULL field is
    /// [`Truth::Unknown`] under every comparing operator.
    IsNull,
    /// `col IS NOT NULL` — matches every field [`Self::IsNull`] does not.
    IsNotNull,
    /// `col IS UNREPRESENTABLE` — matches a value PostgreSQL accepts for the
    /// column's **declared** type and the query's front end cannot hold
    /// (`docs/design/decisions.md`, "D101"): past Arrow's format spec in
    /// PostgreSQL's semantics, past the engine's calendar too in DataFusion's
    /// — exactly the values [`pgdt_codec::UnrepresentableMode::Null`] reads as
    /// NULL there. Two-valued, as `IsNull` is; answered
    /// under every [`pgdt_codec::UnrepresentableMode`], and refused under
    /// [`pgdt_codec::SchemaMode::Strings`], which reads no declared type.
    IsUnrepresentable,
    /// `col IS NOT UNREPRESENTABLE` — matches every field
    /// [`Self::IsUnrepresentable`] does not, a NULL field among them.
    IsNotUnrepresentable,
    Lt(String),
    Le(String),
    Gt(String),
    Ge(String),
    /// `col IS DISTINCT FROM <value>` — [`Self::Ne`] with NULL counted as a
    /// value rather than as unknown, so a NULL field answers [`Truth::True`]
    /// where `!=` answers [`Truth::Unknown`]
    /// (`docs/design/decisions.md`, "D53").
    IsDistinctFrom(String),
    /// `col IS NOT DISTINCT FROM <value>` — [`Self::Eq`] with the same NULL
    /// rule, answering [`Truth::False`] on a NULL field.
    IsNotDistinctFrom(String),
    /// `col IN (<value>, …)`, the values in the order written, answering
    /// exactly as the [`Expr::Or`] of one [`Self::Eq`] term per value would —
    /// but finding and decoding its field once a row, and answering by one
    /// lookup however long the list (`docs/design/decisions.md`, "D53").
    ///
    /// A value is read as an `Eq` term's is, once per block, so a literal that
    /// is not of the column's type is `Error::PredicateValueDecode` for the
    /// first such value, naming the operator `IN`. **`None` is SQL's `NULL`**,
    /// which no row equals: it never makes the membership true, and wherever
    /// no other value matches it makes it [`Truth::Unknown`] rather than
    /// `False`. A NULL field is `Unknown` against any list with a value in it;
    /// the empty list is `False` on every row, as the empty disjunction is,
    /// and a list holding nothing but NULLs reads no field at all, as `= NULL`
    /// reads none.
    In(Vec<Option<String>>),
    /// `col NOT IN (<value>, …)` — [`Self::In`]'s truth negated, as
    /// [`Expr::Not`] over it answers, so a list holding a NULL keeps no row.
    /// Only `In` lets a dynamic filter's row evaluation drop the bounds it
    /// implies (`docs/design/decisions.md`, "D93").
    NotIn(Vec<Option<String>>),
}

/// What a [`PredicateOp`] compares its column against
/// ([`PredicateOp::operand`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operand<'a> {
    /// The NULL and unrepresentable tests, which read none.
    None,
    /// The literal a comparing operator holds.
    Literal(&'a str),
    /// The list a membership holds, `None` a NULL.
    Set(&'a [Option<String>]),
}

impl PredicateOp {
    /// Whether this operator compares by the column's own order rather than
    /// as text — the four that need, in PostgreSQL's semantics, a column
    /// compared as its declared type (`compares_as_declared`).
    pub fn is_ordering(&self) -> bool {
        matches!(self, Self::Lt(_) | Self::Le(_) | Self::Gt(_) | Self::Ge(_))
    }

    /// How an error message names this operator — the same spelling
    /// `pgdt query --filter` accepts.
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Eq(_) => "=",
            Self::Ne(_) => "!=",
            Self::IsNull => "IS NULL",
            Self::IsNotNull => "IS NOT NULL",
            Self::Lt(_) => "<",
            Self::Le(_) => "<=",
            Self::Gt(_) => ">",
            Self::Ge(_) => ">=",
            Self::IsDistinctFrom(_) => "IS DISTINCT FROM",
            Self::IsNotDistinctFrom(_) => "IS NOT DISTINCT FROM",
            Self::IsUnrepresentable => "IS UNREPRESENTABLE",
            Self::IsNotUnrepresentable => "IS NOT UNREPRESENTABLE",
            Self::In(_) => "IN",
            Self::NotIn(_) => "NOT IN",
        }
    }

    /// What this operator compares against: one literal, a membership's
    /// list, or nothing for the NULL and unrepresentable tests.
    pub fn operand(&self) -> Operand<'_> {
        match self {
            Self::Eq(value)
            | Self::Ne(value)
            | Self::Lt(value)
            | Self::Le(value)
            | Self::Gt(value)
            | Self::Ge(value)
            | Self::IsDistinctFrom(value)
            | Self::IsNotDistinctFrom(value) => Operand::Literal(value),
            Self::In(values) | Self::NotIn(values) => Operand::Set(values),
            Self::IsNull
            | Self::IsNotNull
            | Self::IsUnrepresentable
            | Self::IsNotUnrepresentable => Operand::None,
        }
    }

    /// Whether this is one of the two unrepresentable tests.
    pub fn tests_unrepresentable(&self) -> bool {
        matches!(self, Self::IsUnrepresentable | Self::IsNotUnrepresentable)
    }

    /// Whether this operator reads its column's declared type under
    /// `semantics`, so that a query resolving none —
    /// [`SchemaMode::Strings`](pgdt_codec::SchemaMode::Strings) — cannot
    /// answer it: an unrepresentable test always, and an ordering operator in
    /// PostgreSQL's semantics, DataFusion's ordering the text bytewise.
    pub(crate) fn reads_declared_type(&self, semantics: ComparisonSemantics) -> bool {
        self.tests_unrepresentable()
            || (self.is_ordering() && semantics == ComparisonSemantics::Postgres)
    }
}

/// A single-column post-parse filter, `column <op>`, and the **leaf** of an
/// [`Expr`] — a query carries one expression and keeps a row only if its
/// root evaluates [`Truth::True`] (`QueryOptions::filter`). `column` is
/// matched against the queried table's column names, the `COPY` header's
/// list; `op` carries what it compares against, if anything.
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

impl Expr {
    /// Whether this tree holds an unrepresentable test
    /// ([`PredicateOp::tests_unrepresentable`]).
    pub fn tests_unrepresentable(&self) -> bool {
        self.first_term(&|term: &Predicate| term.op.tests_unrepresentable()).is_some()
    }

    /// The first term in this tree, left to right, that `wanted` picks.
    pub(crate) fn first_term(&self, wanted: &impl Fn(&Predicate) -> bool) -> Option<&Predicate> {
        match self {
            Self::Term(predicate) => wanted(predicate).then_some(predicate),
            Self::And(children) | Self::Or(children) => {
                children.iter().find_map(|child| child.first_term(wanted))
            }
            Self::Not(inner) => inner.first_term(wanted),
        }
    }
}

impl Default for Expr {
    fn default() -> Self {
        Self::all([])
    }
}

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
