//! What a comparison says where it is not the server's: a term's or a
//! column's divergence from PostgreSQL's own comparison
//! (`docs/design/decisions.md`, "D59"), and the sentence a refused term is
//! refused with.

use pgdt_codec::compare::{
    ComparisonDivergence, ComparisonPlan, ComparisonSemantics, UnanswerableReason,
};
use pgdt_codec::internal::datafusion_position_divergences;
use pgdt_codec::{ColumnResolution, ResolvedSchema};
use pgdt_model::{Finding, Severity};

/// One term of one query whose comparison does not answer what PostgreSQL's
/// own operator for that column would — reported per stream by
/// `crate::stream::TableStream::comparison_notes`, on its own channel rather
/// than as a `Diagnostic` or a [`pgdt_codec::ResolvedColumn`]
/// (`docs/design/decisions.md`, "D59", "D68"), and reaching a caller's
/// `pgdt_model::DiagnosticSink` beside them as a
/// [`pgdt_model::Finding`].
///
/// Per term, a divergence being operator-conditional
/// ([`ComparisonDivergence::affects_equality`]): a `text` column with no
/// `COLLATE` clause earns a note under `<` and none under `=`, so a query
/// filtering it with both operators carries one note. The same record is also
/// a column's, from [`Table::divergences`](crate::Table::divergences),
/// whatever terms a query names.
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

impl ComparisonNote {
    /// **Why the position is compared bytewise**, the clause
    /// [`Finding::message`] ends with after "is compared bytewise: ", whatever
    /// the column — so a caller folding many columns' notes into one sentence
    /// states it once. `None` for a divergence whose sentence is not a
    /// bytewise one.
    pub fn bytewise_reason(&self) -> Option<&'static str> {
        Some(match self.divergence {
            // Not "PostgreSQL orders this differently": it does not order it
            // at all, and the sentence has to say which way the difference
            // runs.
            ComparisonDivergence::AsText => {
                "PostgreSQL defines no comparison for this type at all — no equality, no \
                 ordering, no operator class — so this comparison is one the server does not \
                 have"
            }
            ComparisonDivergence::UnknownCollation => {
                "the column declares no COLLATE clause, so its collation is the database's, which \
                 a plain dump does not record — this matches the server only if that collation is \
                 C or POSIX"
            }
            ComparisonDivergence::NonBytewiseCollation => {
                "the column declares a collation other than C/POSIX, and PostgreSQL orders it by \
                 that collation"
            }
            // The one sentence reporting what the dump *said* rather than
            // what it left out, which is why it names equality outright.
            ComparisonDivergence::NonDeterministicCollation => {
                "the column declares a collation this dump declares non-deterministic, so \
                 PostgreSQL neither orders nor compares it byte for byte — two values spelled \
                 differently can be equal to the server"
            }
            ComparisonDivergence::UnmodelledType => {
                "this build models no comparison for the column's declared type — the four \
                 ordering operators are refused on it, and PostgreSQL's own equality for such a \
                 type need not be a byte comparison (`box` compares areas)"
            }
            ComparisonDivergence::ValueAsText => {
                "DataFusion compares the text it is emitted as, where PostgreSQL compares it by \
                 value — the orders can differ, and so can equality: a literal matches only the \
                 spelling the server writes (`'12:00+00'` misses `12:00:00+00`, `'10.0.0.1/32'` \
                 misses an inet's `10.0.0.1`, uppercase hex misses a macaddr), and some types \
                 write one value two ways (`1.5` and `1.50`)"
            }
            ComparisonDivergence::PaddedText => {
                "DataFusion compares the value with its blank padding, as it is emitted, where \
                 PostgreSQL ignores trailing blanks on both sides — so a literal matches only \
                 when padded to the column's width (`'abc'` misses `abc  ` in a character(5))"
            }
            ComparisonDivergence::JsonbStringCollation
            | ComparisonDivergence::LabelText
            | ComparisonDivergence::IntervalFields
            | ComparisonDivergence::NestedOrder
            | ComparisonDivergence::UnnormalizedZero => return None,
        })
    }
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
        if let Some(why) = self.bytewise_reason() {
            return format!("{subject} is compared bytewise: {why}");
        }
        match self.divergence {
            // Not a bytewise sentence: the structure *is* compared the way
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
            ComparisonDivergence::IntervalFields => format!(
                "{subject} is compared as DataFusion compares an Interval(MonthDayNano) — months, \
                 then days, then the time part — where PostgreSQL compares the whole span, so \
                 `1 mon` and `30 days` are equal to the server and not here"
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
            ComparisonDivergence::AsText
            | ComparisonDivergence::UnknownCollation
            | ComparisonDivergence::NonBytewiseCollation
            | ComparisonDivergence::NonDeterministicCollation
            | ComparisonDivergence::UnmodelledType
            | ComparisonDivergence::ValueAsText
            | ComparisonDivergence::PaddedText => unreachable!("a bytewise sentence is above"),
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
/// ([`ResolvedTerm::comparison_notes`](super::ResolvedTerm::comparison_notes)): an operator refused on the column
/// adds nothing, having no answer to diverge.
///
/// **Under [`ComparisonSemantics::DataFusion`]** they say how DataFusion's
/// comparison of the emitted value differs from the server's, whether or not
/// this build answers a term on the column — `ORDER BY`, `MIN`/`MAX` and every
/// filter a plan keeps reach it too, so a nested column whose every term is
/// refused here still reports [`ComparisonDivergence::NestedOrder`] for
/// its order, and each position inside it its own divergence under its path
/// ([`pgdt_codec::compare::NestedCompare::datafusion_divergences`]). A
/// kind [`CompareKind::datafusion_order`](pgdt_codec::compare::CompareKind::datafusion_order) moves reports
/// [`CompareKind::datafusion_divergence`](pgdt_codec::compare::CompareKind::datafusion_divergence), beside any collation the register
/// already announced; `jsonb`'s string collation is dropped, the whole value
/// being compared as text. A column that fell back to text reports nothing,
/// its `Warning` column record being the finding.
pub fn column_divergences(
    resolved: &ResolvedSchema,
    semantics: ComparisonSemantics,
) -> Vec<ComparisonNote> {
    let mut out = Vec::new();
    for (index, column) in resolved.columns().iter().enumerate() {
        let declared = column.declared().unwrap_or_default().to_string();
        let positions = match semantics {
            ComparisonSemantics::Postgres => postgres_divergences(resolved, index),
            ComparisonSemantics::DataFusion => datafusion_divergences(resolved, index, &declared),
        };
        out.extend(positions.into_iter().map(|(path, declared_type, divergence)| ComparisonNote {
            column: column.name().to_string(),
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
pub(super) fn compares_as_declared(resolution: &ColumnResolution) -> bool {
    matches!(resolution, ColumnResolution::Mapped | ColumnResolution::UnrepresentableValues)
}

/// One diverging position: its path inside the column (`None` for the column
/// itself), its declared type, and the divergence.
pub(super) type DivergingPosition = (Option<String>, String, ComparisonDivergence);

/// What a term on column `index` announces under PostgreSQL's semantics, in
/// the operator family `ordering` names, where resolving the term does not
/// refuse it: the register's divergences for the column, a nested one's per
/// position, each kept where the family is the ordering one or it reaches
/// equality ([`ComparisonDivergence::affects_equality`]).
///
/// A column that *fell out of a tree* — one with an uncompared position,
/// whose `=` compares the container's text — announces per position as a
/// compared tree does: the position that refused the order, and any other
/// whose divergence reaches equality. A position the *resolver* declined
/// instead (I22, I26) never reaches the tree; its column is not `Mapped`.
///
/// A column that does not compare as declared announces off its own
/// **resolution**, and only `UnknownType` and `OpaqueBaseType` do: the file
/// *named* a type this build models nothing for — `box`, `money`, a C-level
/// base type — and `box_eq` compares areas, so bytewise is a guess there
/// ([`ComparisonDivergence::UnmodelledType`]). Every other outcome is silent,
/// there being no declared type to qualify.
pub(super) fn announced(
    resolved: &ResolvedSchema,
    index: usize,
    ordering: bool,
) -> Vec<DivergingPosition> {
    let record = &resolved.columns()[index];
    let declared = record.declared().unwrap_or_default().to_string();
    let reaches = |d: &ComparisonDivergence| ordering || d.affects_equality();
    let column = |divergence: Option<ComparisonDivergence>| {
        divergence.filter(reaches).map(|d| (None, declared.clone(), d)).into_iter().collect()
    };
    if !compares_as_declared(record.resolution()) {
        return match record.resolution() {
            ColumnResolution::UnknownType | ColumnResolution::OpaqueBaseType => {
                column(Some(ComparisonDivergence::UnmodelledType))
            }
            _ => Vec::new(),
        };
    }
    match record.comparison() {
        ComparisonPlan::Unanswerable(_) => Vec::new(),
        ComparisonPlan::Nested(tree) => tree
            .divergences()
            .into_iter()
            .filter(|(_, _, d)| reaches(d))
            .map(|(path, declared, d)| (Some(path), declared, d))
            .collect(),
        _ if !record.plan().is_scalar() => Vec::new(),
        ComparisonPlan::Compared { divergence, .. } => column(*divergence),
        ComparisonPlan::Refused => Vec::new(),
    }
}

/// [`column_divergences`] under PostgreSQL's semantics: what a term on the
/// column announces ([`announced`]) under the widest operator family the
/// column answers.
fn postgres_divergences(resolved: &ResolvedSchema, index: usize) -> Vec<DivergingPosition> {
    // A tree with an uncompared position answers only `=`/`!=`. Any other
    // column refusing `<` announces nothing `<` would add, its one
    // divergence, `UnmodelledType`, reaching `=`.
    let ordering = !matches!(
        resolved.columns()[index].comparison(),
        ComparisonPlan::Nested(tree) if tree.uncomparable().is_some()
    );
    announced(resolved, index, ordering)
}

/// [`column_divergences`] under DataFusion's semantics, read off what the column
/// emits rather than off which terms this build answers.
///
/// A column that did not resolve says nothing here: its fall-back
/// [`pgdt_codec::ResolvedColumn`] is already a `Warning` saying the value is
/// the file's text, which is all DataFusion compares.
fn datafusion_divergences(
    resolved: &ResolvedSchema,
    index: usize,
    declared: &str,
) -> Vec<DivergingPosition> {
    let record = &resolved.columns()[index];
    let column = |divergence| (None, declared.to_string(), divergence);
    if *record.resolution() != ColumnResolution::Mapped {
        return Vec::new();
    }
    if !record.plan().is_scalar() {
        // The container's order, then each position's own divergence; a
        // plan with no tree has no position to name.
        let positions = match record.comparison() {
            ComparisonPlan::Nested(tree) => tree.datafusion_divergences(),
            _ => Vec::new(),
        };
        return std::iter::once(column(ComparisonDivergence::NestedOrder))
            .chain(positions.into_iter().map(|(path, declared, d)| (Some(path), declared, d)))
            .collect();
    }
    match record.comparison() {
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

/// The refusal an ordering operator earns on a column that cannot carry one.
/// Four reasons, each a different fact about the column.
pub(super) const NOT_MAPPED: &str =
    "the column's declared type did not resolve to an Arrow type, so it has no order of its own";
pub(super) const NESTED: &str = "the column is nested (array, composite, range or multirange), and an order over such a \
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
pub(super) fn nested_refusal(
    path: &str,
    declared: &str,
    divergence: Option<ComparisonDivergence>,
) -> String {
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
pub(super) const NO_ORDER: &str = "this build defines no ordering for the column's declared type";
pub(super) const NESTED_IN_DATAFUSION: &str = "the column is nested (array, composite, range or \
     multirange), and this build does not compare a nested value in DataFusion's semantics";

/// The sentence for a column the register can answer **no** operator on,
/// worded here rather than in `pgdt_codec::compare` for the same reason
/// [`accepted_form`](pgdt_codec::internal::accepted_form) is: the register carries the fact, this layer says it in
/// a sentence about the comparison a filter was going to make.
pub(super) fn unanswerable_reason(reason: &UnanswerableReason) -> String {
    match reason {
        UnanswerableReason::RangeCanonical { range_type, function } => format!(
            "the range type `{range_type}` declares a canonical function (`{function}`), \
             which PostgreSQL applies to every value of it before storing or comparing \
             one — arbitrary server-side code this build cannot run, so two spellings the \
             server calls one value would be two values here"
        ),
    }
}
