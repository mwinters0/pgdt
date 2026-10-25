//! A filter resolved against one `COPY` block: each term's column found in
//! the block's unprojected schema, its comparison settled and its literal
//! read once, so a row is evaluated ([`super::eval`]) and a row group
//! answered ([`super::truths`]) against the same tree.

use pgdt_codec::ResolvedSchema;
use pgdt_codec::compare::{CompareKind, ComparisonPlan, ComparisonSemantics, NestedCompare};
use pgdt_codec::internal::{
    Equality, NestedKey, OrderKey, Side, Spellings, UnrepresentableRead, accepted_form,
    compare_keys, compare_nested, literal_equality, literal_key, nested_accepted_form, nested_key,
    order_key, unlisted_labels,
};
use pgdt_core::{Error, Result};
use pgdt_model::PostgresInvalidValues;

use super::notes::{
    ComparisonNote, DivergingPosition, NESTED, NESTED_IN_DATAFUSION, NO_ORDER, NOT_MAPPED,
    announced, compares_as_declared, nested_refusal, unanswerable_reason,
};
use super::{Expr, Operand, Predicate, PredicateOp};

/// What one term compares, settled once when the block's schema resolves
/// rather than per row.
#[derive(Debug, Clone)]
pub(super) enum Comparison {
    /// The four ordering operators: the field is decoded per row with the
    /// column's own decoder and its key compared against `bound`.
    Ordered { kind: CompareKind, bound: OrderKey },
    /// `=`/`!=` on a scalar column, against the literal's [`Equality`]: its
    /// `*_out` spellings, its text trimmed, or its key
    /// ([`literal_equality`]) — or, where the register gives the column no
    /// comparison, its own text as the one spelling.
    Equal(Equality),
    /// A nested column, under **any** comparing operator: both sides read
    /// into a [`NestedKey`] and compared structurally.
    /// One variant for both operator
    /// families, where a scalar has one for each and its equality three
    /// forms, because a nested value's `=` is byte-comparable only when
    /// *every* leaf beneath it canonicalizes.
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
pub(super) struct NestedComparison {
    pub(super) plan: NestedCompare,
    pub(super) bound: NestedKey,
}

/// Everything a comparing term needs, settled once when the block's schema
/// resolves rather than per row.
#[derive(Debug, Clone)]
pub(super) struct ComparedTerm {
    pub(super) column: String,
    pub(super) comparison: Comparison,
    /// The declared PostgreSQL type, for `Error::FieldDecode`'s context and
    /// for [`ComparisonNote::message`].
    pub(super) declared_type: String,
    /// The register's divergences for this column, already filtered to the
    /// ones that reach *this* term's operator ([`announced`]).
    ///
    /// A list, because a nested column has a position per divergence: a
    /// composite can be on the database's collation twice, through two
    /// different fields. Each entry carries the position's own path and
    /// declared type (`docs/design/decisions.md`, "D59").
    pub(super) divergences: Vec<DivergingPosition>,
    /// Which of a row group's statistics this term reads
    /// ([`ResolvedTerm::truths`]).
    pub(super) statistics: BelievedStatistics,
}

/// Which of a row group's statistics a comparing term reads, settled once
/// with the rest of the term. A statistic this term does not read is not
/// wrong for it; it is one whose meaning the term's comparison does not
/// share.
#[derive(Debug, Clone)]
pub(super) struct BelievedStatistics {
    /// The kind the term compares by, which names the stored set of bounds
    /// it reads in each block ([`pgdt_codec::compare::bounds_set_keyed_by`]), and
    /// its literal read as a key — present only where the term's semantics
    /// believes bounds in that kind ([`ComparisonPlan::bounds_read_by`]).
    /// Carried for the equality operators too, whose [`Comparison`] keeps no
    /// kind but where it decodes. Boxed, as [`Comparison::Nested`] is, because every comparing
    /// leaf carries it.
    pub(super) bounds: Option<Box<(CompareKind, OrderKey)>>,
    /// Whether a group's dictionary answers this term: one of the four
    /// equality operators, on a `Compared` column whose divergence, if any,
    /// does not reach equality — and in DataFusion's semantics not a
    /// `character(n)`, whose entries are stored unpadded
    /// ([`ComparisonPlan::dictionary_answers_in`]).
    pub(super) dictionary: bool,
}

impl BelievedStatistics {
    /// A term no statistic but the counts answers: a nested column, and one
    /// compared as its text for want of a plan outside DataFusion's semantics.
    pub(super) const NONE: Self = Self { bounds: None, dictionary: false };
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
    pub(super) op: PredicateOp,
    pub(super) index: usize,
    /// `None` for the NULL and unrepresentable tests, the four operators that
    /// compare nothing.
    pub(super) compared: Option<ComparedTerm>,
    /// How the query reads a value of this column its type cannot hold, where
    /// the block's column can hold one ([`ResolvedExpr::reading`]).
    pub(super) unrepresentable: Option<UnrepresentableRead>,
    /// What an unrepresentable test tests its column's text by, where the
    /// block's count says the column holds such a value in the tiers the
    /// query's front end reads; `None` for every other operator, and for a
    /// test over a column holding none, where `IS UNREPRESENTABLE` is `False`
    /// on every row and `IS NOT UNREPRESENTABLE` `True`
    /// (`pgdt_codec::unrepresentable::unrepresentable_tests`).
    pub(super) tested: Option<UnrepresentableRead>,
    /// How the query reads a field its type's `*_in` refuses
    /// ([`ResolvedExpr::invalid_values`]).
    pub(super) invalid: PostgresInvalidValues,
    /// The column's name, where its declaration makes it `NOT NULL`, so a
    /// NULL field is refused ([`Error::NullRefused`]) but under
    /// [`PostgresInvalidValues::Ignore`], whatever the operator.
    pub(super) not_null: Option<String>,
}

/// The [`ResolvedTerm::not_null`] of column `index` of `resolved`, named
/// `column`.
fn not_null_column(resolved: &ResolvedSchema, index: usize, column: &str) -> Option<String> {
    resolved.columns().get(index).is_some_and(|c| c.not_null()).then(|| column.to_string())
}

impl ResolvedTerm {
    /// This term's divergences from PostgreSQL's own comparison. Empty for
    /// the operators taking no value, and for every term whose column answers
    /// this operator the way the server does; more than one only for a
    /// nested column with more than one diverging position.
    pub(super) fn comparison_notes(&self) -> Vec<ComparisonNote> {
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

    /// The operator this term applies.
    pub(crate) fn op(&self) -> &PredicateOp {
        &self.op
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
/// resolution already reached (`pgdt_codec::compare::comparison_for`), carried in
/// [`pgdt_codec::ResolvedColumn::comparison`].
///
/// **Under [`ComparisonSemantics::DataFusion`]** a scalar compares by its kind's
/// [`CompareKind::datafusion_order`], a column with no plan — every one of which
/// emits `Utf8View` — compares bytewise under every operator, every
/// comparing operator on a nested column is refused, statistics are read only where they are ordered or equated
/// that way, and no term announces a divergence from PostgreSQL, which is a
/// property of the column rather than of the term ([`column_divergences`](super::column_divergences);
/// `docs/design/decisions.md`, "D40").
pub(super) fn resolve_term(
    predicate: &Predicate,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
) -> Result<ResolvedTerm> {
    let text = match predicate.op.operand() {
        Operand::Literal(text) => text,
        // Unreachable rather than typed away by a value-free mirror of the
        // operator, public or private, which would save one clone a block and
        // restate the operator set.
        Operand::Set(_) => unreachable!("a membership resolves through `resolve_leaf`"),
        Operand::None => {
            return Ok(ResolvedTerm {
                op: predicate.op.clone(),
                index,
                compared: None,
                unrepresentable: None,
                tested: None,
                invalid: PostgresInvalidValues::Default,
                not_null: not_null_column(resolved, index, &predicate.column),
            });
        }
    };
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
    let record = &resolved.columns()[index];
    let declared_type = record.declared().unwrap_or_default().to_string();
    // `kind` is what knows which grammar was applied, so the refusal is built
    // where it is in scope.
    let refuse_literal = |kind: &CompareKind| Error::PredicateValueDecode {
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        value: text.to_string(),
        declared_type: declared_type.clone(),
        accepted: accepted_form(kind),
        unlisted: unlisted_labels(kind),
    };
    let mut plan = Some(record.comparison());
    if !compares_as_declared(record.resolution()) {
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
            plan = None;
        }
    } else if !record.plan().is_scalar() {
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
    let (comparison, statistics) = match plan {
        Some(compared @ ComparisonPlan::Compared { kind, .. }) => {
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
                Comparison::Equal(
                    literal_equality(kind, text, semantics).ok_or_else(|| refuse_literal(kind))?,
                )
            };
            let statistics = BelievedStatistics {
                bounds: believed_bounds(compared, semantics, kind, text),
                dictionary: !ordering && compared.dictionary_answers_in(semantics),
            };
            (comparison, statistics)
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
                        unlisted: 0,
                    }
                })?,
            })),
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
                kind: CompareKind::Text { length: None, varying: false },
                bound: order_key(&CompareKind::Text { length: None, varying: false }, text)
                    .ok_or_else(|| {
                        refuse_literal(&CompareKind::Text { length: None, varying: false })
                    })?,
            },
            BelievedStatistics {
                bounds: believed_bounds(
                    record.comparison(),
                    semantics,
                    &CompareKind::Text { length: None, varying: false },
                    text,
                ),
                dictionary: false,
            },
        ),
        _ if ordering => return Err(refuse(NO_ORDER)),
        _ => (
            Comparison::Equal(Equality::Canonical(Spellings::one(text.to_string()))),
            BelievedStatistics {
                bounds: believed_bounds(
                    record.comparison(),
                    semantics,
                    &CompareKind::Text { length: None, varying: false },
                    text,
                ),
                dictionary: false,
            },
        ),
    };
    // Arrow's answer is not the register's to qualify: a divergence from
    // PostgreSQL is the column's (`docs/design/decisions.md`, "D40").
    let divergences = if arrow { Vec::new() } else { announced(resolved, index, ordering) };
    Ok(ResolvedTerm {
        op: predicate.op.clone(),
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

/// A [`PredicateOp::In`] term resolved against one `COPY` block — and a
/// [`PredicateOp::NotIn`]'s, beneath the [`ResolvedExpr::Not`] it resolves
/// to ([`resolve_leaf`]): the `=` term each
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
    pub(super) index: usize,
    pub(super) terms: Vec<ResolvedTerm>,
    /// Whether the list holds a NULL, which answers `Unknown` wherever no
    /// other value matches.
    pub(super) null: bool,
    pub(super) lookup: Lookup,
    /// As [`ResolvedTerm`]'s.
    pub(super) unrepresentable: Option<UnrepresentableRead>,
    /// As [`ResolvedTerm`]'s.
    pub(super) invalid: PostgresInvalidValues,
    /// As [`ResolvedTerm`]'s.
    pub(super) not_null: Option<String>,
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
pub(super) enum Lookup {
    /// [`Equality::Canonical`]: the literals in every spelling the file
    /// can hold them in, the field probed as it stands.
    Canonical(std::collections::HashSet<String>),
    /// [`Equality::Trimmed`]: the literals trimmed, the field trimmed
    /// before it is probed.
    Trimmed(std::collections::HashSet<String>),
    /// [`Equality::Decoded`]: the literals' keys sorted by
    /// [`compare_keys`](pgdt_codec::internal::compare_keys), the field keyed once and searched for among them.
    Decoded { kind: CompareKind, keys: Vec<OrderKey> },
    /// [`Comparison::Nested`]: as `Decoded`, by [`compare_nested`](pgdt_codec::internal::compare_nested).
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
            None | Some(Comparison::Equal(Equality::Canonical(_))) => comparisons
                .iter()
                .map(|c| match c {
                    Comparison::Equal(Equality::Canonical(spellings)) => {
                        Some(spellings.iter().cloned())
                    }
                    _ => None,
                })
                .collect::<Option<Vec<_>>>()
                .map(|spellings| Self::Canonical(spellings.into_iter().flatten().collect())),
            Some(Comparison::Equal(Equality::Trimmed(_))) => texts(|c| match c {
                Comparison::Equal(Equality::Trimmed(text)) => Some(text),
                _ => None,
            })
            .map(Self::Trimmed),
            Some(Comparison::Equal(Equality::Decoded { kind, .. })) => comparisons
                .iter()
                .map(|c| match c {
                    Comparison::Equal(Equality::Decoded { kind: own, bound }) if own == kind => {
                        Some(bound.clone())
                    }
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
}

/// Resolve the membership of `column` in `values` against the block's
/// unprojected `resolved` schema, its column already found at `index`: each
/// non-NULL value as the `=` term [`resolve_term`] makes of it, in list
/// order, so the first value that refuses is the one refused — its error
/// naming the membership's operator, `op`.
pub(super) fn resolve_membership(
    column: &str,
    values: &[Option<String>],
    op: &'static str,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
) -> Result<ResolvedMembership> {
    let terms = values
        .iter()
        .flatten()
        .map(|value| {
            let equal =
                Predicate { column: column.to_string(), op: PredicateOp::Eq(value.clone()) };
            resolve_term(&equal, index, resolved, header_offset, semantics)
                .map_err(|error| named(error, op))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ResolvedMembership {
        index,
        null: values.iter().any(Option::is_none),
        lookup: Lookup::of(&terms),
        terms,
        unrepresentable: None,
        invalid: PostgresInvalidValues::Default,
        not_null: not_null_column(resolved, index, column),
    })
}

/// A refusal [`resolve_term`] raised for one value's `=`, re-worded as the
/// membership's operator `op`.
fn named(error: Error, op: &'static str) -> Error {
    match error {
        Error::PredicateValueDecode {
            column, value, declared_type, accepted, unlisted, ..
        } => Error::PredicateValueDecode { column, op, value, declared_type, accepted, unlisted },
        Error::UncomparablePredicateColumn { header_offset, column, reason, .. } => {
            Error::UncomparablePredicateColumn { header_offset, column, op, reason }
        }
        other => other,
    }
}

/// One filter term resolved against the block's unprojected `resolved`
/// schema, its column already found at `index`: a comparison or a test as
/// [`resolve_term`] resolves it, `IN` as one [`ResolvedMembership`], and
/// `NOT IN` as the [`ResolvedExpr::Not`] over its membership, which answers
/// as `In`'s truth negated and which a dynamic filter's row evaluation reads
/// no bounds through ([`ResolvedExpr::for_rows`]).
pub(super) fn resolve_leaf(
    predicate: &Predicate,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
) -> Result<ResolvedExpr> {
    let membership = |values: &[Option<String>]| {
        let (column, op) = (&predicate.column, predicate.op.symbol());
        resolve_membership(column, values, op, index, resolved, header_offset, semantics)
            .map(ResolvedExpr::In)
    };
    Ok(match &predicate.op {
        PredicateOp::In(values) => membership(values)?,
        PredicateOp::NotIn(values) => ResolvedExpr::Not(Box::new(membership(values)?)),
        _ => {
            ResolvedExpr::Term(resolve_term(predicate, index, resolved, header_offset, semantics)?)
        }
    })
}

impl ResolvedMembership {
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

    /// The divergence notes for every term in this tree, in the order the
    /// caller wrote them — what `TableStream::comparison_notes` hands back.
    /// Derived from the resolved tree rather than stored beside it, so the two
    /// cannot disagree (`docs/design/decisions.md`, "D59").
    pub(crate) fn comparison_notes(&self) -> Vec<ComparisonNote> {
        let mut out = Vec::new();
        self.collect_notes(&mut out);
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

    /// This node's conjuncts, in order, a conjunction nested in it read as
    /// part of it.
    pub(super) fn conjuncts<'a>(&'a self, out: &mut Vec<&'a ResolvedExpr>) {
        match self {
            Self::And(children) => children.iter().for_each(|child| child.conjuncts(out)),
            other => out.push(other),
        }
    }
}

/// Resolve `filter` against `resolved` — the block's own **unprojected**
/// schema — once per block, returning the same tree with every leaf
/// resolved. The empty conjunction resolves to an empty conjunction.
///
/// **This is where what a term asks is validated against a block**, and the
/// only place (`docs/design/decisions.md`, "D54"): an ordering
/// operator on a column the register gives no order is, in PostgreSQL's
/// semantics, `Error::UnorderedPredicateColumn`, any comparing operator on a column whose
/// server comparison is not a text one is `Error::UncomparablePredicateColumn`,
/// and a literal that is not a value of
/// the column's type — `=` included — is `Error::PredicateValueDecode`. All
/// are raised for the first offending term in a left-to-right walk. A term
/// naming a column this block does not carry is
/// `Error::UnknownPredicateColumn`, which the plan raises ahead of every
/// other refusal, [`check_columns`] walking the whole tree first; here it is
/// reached only by a caller that has not. The plan
/// runs it for every block before any is read, and raises the first refusing
/// block's refusal in file order before any row of the table
/// (the stream's `plan_blocks`). It takes the whole [`ResolvedSchema`] because the ordering
/// refusal reads `columns` and `plans` too.
pub(crate) fn resolve_expr(
    filter: &Expr,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
) -> Result<ResolvedExpr> {
    let branch = |children: &[Expr]| {
        children
            .iter()
            .map(|child| resolve_expr(child, resolved, header_offset, semantics))
            .collect::<Result<Vec<_>>>()
    };
    let column = |name: &str| {
        column_position(resolved, name).ok_or_else(|| Error::UnknownPredicateColumn {
            header_offset,
            column: name.to_string(),
        })
    };
    Ok(match filter {
        Expr::Term(predicate) => {
            resolve_leaf(predicate, column(&predicate.column)?, resolved, header_offset, semantics)?
        }
        Expr::And(children) => ResolvedExpr::And(branch(children)?),
        Expr::Or(children) => ResolvedExpr::Or(branch(children)?),
        Expr::Not(inner) => {
            ResolvedExpr::Not(Box::new(resolve_expr(inner, resolved, header_offset, semantics)?))
        }
    })
}

/// Refuse the first term of `filter`, left to right, naming a column
/// `resolved` — the block's own **unprojected** schema — does not carry, as
/// `Error::UnknownPredicateColumn`. **Names come before what a term asks**:
/// the plan runs this ahead of the strings schema mode's refusal and of
/// [`resolve_expr`]'s (`docs/design/decisions.md`, "D54"). A pass of its
/// own, not a lookup in that walk or a skip in the mode's check: either lets
/// an earlier term's refusal outrank a later term's typo.
pub(crate) fn check_columns(
    filter: &Expr,
    resolved: &ResolvedSchema,
    header_offset: u64,
) -> Result<()> {
    let unknown = |term: &Predicate| column_position(resolved, &term.column).is_none();
    match filter.first_term(&unknown) {
        Some(term) => {
            Err(Error::UnknownPredicateColumn { header_offset, column: term.column.clone() })
        }
        None => Ok(()),
    }
}

/// Where the column `name` sits in the block's unprojected schema.
fn column_position(resolved: &ResolvedSchema, name: &str) -> Option<usize> {
    resolved.schema().fields().iter().position(|f| f.name() == name)
}

/// A `DynamicFilter`'s state resolved as [`resolve_expr`] resolves a static
/// filter, but **never refused**: a term this block cannot resolve stands as
/// whatever keeps every row where it sits — the empty conjunction beneath an
/// even number of `Not`s, the empty disjunction beneath an odd one — since a
/// dynamic filter only ever licenses dropping rows, and a term it cannot
/// state here licenses nothing.
pub(crate) fn resolve_loosened(
    filter: &Expr,
    resolved: &ResolvedSchema,
    header_offset: u64,
    semantics: ComparisonSemantics,
    negated: bool,
) -> ResolvedExpr {
    let branch = |children: &[Expr]| {
        children
            .iter()
            .map(|child| resolve_loosened(child, resolved, header_offset, semantics, negated))
            .collect()
    };
    let leaf = match filter {
        // No producer states one, and a block reading a state holds no test
        // for it to read (`resolve_for_query`'s `testing`), so it stands as
        // whatever keeps every row.
        Expr::Term(predicate) if predicate.op.tests_unrepresentable() => None,
        Expr::Term(predicate) => column_position(resolved, &predicate.column).and_then(|index| {
            resolve_leaf(predicate, index, resolved, header_offset, semantics).ok()
        }),
        _ => None,
    };
    match filter {
        Expr::Term(_) => match (leaf, negated) {
            (Some(leaf), _) => leaf,
            (None, false) => ResolvedExpr::And(Vec::new()),
            (None, true) => ResolvedExpr::Or(Vec::new()),
        },
        Expr::And(children) => ResolvedExpr::And(branch(children)),
        Expr::Or(children) => ResolvedExpr::Or(branch(children)),
        Expr::Not(inner) => ResolvedExpr::Not(Box::new(resolve_loosened(
            inner,
            resolved,
            header_offset,
            semantics,
            !negated,
        ))),
    }
}
