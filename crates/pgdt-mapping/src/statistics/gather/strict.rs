//! A strict parse's check (`docs/design/decisions.md`, "D103"): the observer
//! that checks every field of a block against its type's `*_in` and its
//! column's `NOT NULL`, and the listing of what it does not check.

use std::sync::Arc;

use pgdt_codec::ResolvedSchema;
use pgdt_codec::internal::database_for_name;
use pgdt_codec::internal::{field_refused, grammar_refuses};
use pgdt_codec::{
    compare::ComparisonPlan,
    grammar::{TextGrammar, Unchecked},
    internal::{column_grammar, unchecked_positions},
};
use pgdt_core::{CopyHeader, decode_field, split_fields};
use pgdt_model::{CopyBlock, DumpMetadata, FieldRefusal};

use crate::statistics::stored::{declared_column, stored_resolution};

/// What a strict parse checks each of `header`'s columns by, positionally:
/// its comparison under `resolved`, which is what names the type a field is
/// read as ([`field_refused`]), or for a column ordered by nothing its
/// declared type's input grammar ([`column_grammar`]), and whether its
/// declaration makes it `NOT NULL`; `None` for a column read as no type and
/// declared nullable.
pub(super) fn field_checks(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    resolved: &ResolvedSchema,
) -> Arc<[Option<FieldCheck>]> {
    let qualified = header.qualified_name();
    let types =
        metadata.and_then(|m| database_for_name(m, database)).map_or(&[][..], |db| &db.types[..]);
    header
        .columns
        .iter()
        .zip(resolved.comparisons.iter().zip(&resolved.not_null))
        .map(|(name, (plan, &not_null))| {
            let declared_type = declared_column(metadata, database, &qualified, name)
                .map(|def| def.declared_type.clone())
                .unwrap_or_default();
            let grammar = column_grammar(plan, &declared_type, types);
            let reads = grammar.is_some()
                || !matches!(plan, ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_));
            (reads || not_null).then(|| FieldCheck {
                plan: plan.clone(),
                grammar,
                declared_type,
                reads,
                not_null,
            })
        })
        .collect()
}

/// **What a strict parse checks nothing of in a `COPY` block** of a table
/// ([`strict_unchecked`]): what PostgreSQL refuses there is decided by
/// something besides the field and its column's declaration, or no reader
/// here reads it — the boundary a clean strict parse's promise stops at,
/// named per dump, since nothing else lets that promise be checked against
/// one (`docs/design/roadmap.md`, "A literal is guaranteed in `*_out`'s form
/// and never read past `*_in`'s").
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct StrictUnchecked {
    /// Whether the dump declares the table. Where it does not, as in a
    /// `--data-only` dump, no column has a type or a `NOT NULL` to check by,
    /// so no field of it is checked and `columns` lists none.
    pub declared: bool,
    /// Each column, or position within one, whose fields a strict parse
    /// checks for nothing, in column order and each column's in walk order.
    pub columns: Vec<UncheckedColumn>,
    /// The `CHECK` constraints a restore evaluates against each row, its own
    /// and those it inherits.
    pub checks: Vec<UncheckedCheck>,
    /// The partition bounds a restore checks each row against where the
    /// block loads into a partition: its own, then each ancestor's that is a
    /// partition too (I86).
    pub bounds: Vec<UncheckedBound>,
    /// The partitioned table a restore routes each row through to the
    /// partition whose bound admits it, refusing one none admits: the
    /// block's table, where it carries `pg_dump`'s `load via partition root`
    /// marker or the preamble declares a partition of it (I86).
    pub routed_through: Option<String>,
}

impl StrictUnchecked {
    /// Whether a strict parse checks everything a restore refuses a row by
    /// in this block but its post-data constraints.
    pub fn is_empty(&self) -> bool {
        self.declared
            && self.columns.is_empty()
            && self.checks.is_empty()
            && self.bounds.is_empty()
            && self.routed_through.is_none()
    }
}

/// One position a strict parse checks nothing of ([`StrictUnchecked`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UncheckedColumn {
    pub column: String,
    /// The position within the column's value, spelled as
    /// [`pgdt_codec::compare::NestedCompare::uncomparable`] spells one — `[]` an element,
    /// `.name` a field, `.bound` a range's bound — and empty for the value.
    pub path: String,
    /// The type declared at that position, as the dump wrote it; empty for
    /// a column declared nowhere.
    pub declared: String,
    #[serde(flatten)]
    pub why: Unchecked,
}

/// A `CHECK` constraint a restore evaluates against each row of a table
/// ([`StrictUnchecked`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UncheckedCheck {
    /// Its name, `None` for one written without.
    pub name: Option<String>,
    /// The ancestor declaring it, `None` for the table's own.
    pub inherited_from: Option<String>,
}

/// A partition bound a restore checks each row of a block against
/// ([`StrictUnchecked`]): an expression over the partition key, compared
/// under its operator classes and collations.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct UncheckedBound {
    /// The partition it bounds: the block's table, or an ancestor of it.
    pub partition: String,
    /// The partitioned table it is a partition of.
    pub parent: String,
    /// Its text ([`pgdt_model::PartitionOf::bound`]), `None` where the
    /// preamble names the parent and the bound is not read.
    pub bound: Option<String>,
}

/// What a strict parse checks nothing of in `block`, read off the preamble
/// in `metadata` for the block's database as the parse's own check is
/// ([`field_checks`]): the same resolution, the same plan and grammar per
/// column, so the two cannot disagree.
///
/// A property of the dump's declarations and the block's own framing, not of
/// any run: the same for a block a strict parse checked and one it never
/// read.
pub fn strict_unchecked(block: &CopyBlock, metadata: Option<&DumpMetadata>) -> StrictUnchecked {
    let header = &block.header;
    let database = block.database.as_deref();
    let qualified = header.qualified_name();
    let db = metadata.and_then(|m| database_for_name(m, database));
    let routed =
        block.partition_root.is_some() || db.is_some_and(|db| db.has_partitions(&qualified));
    let routed_through = routed.then(|| qualified.clone());
    let Some(db) = db.filter(|db| db.tables.contains_key(&qualified)) else {
        return StrictUnchecked { routed_through, ..StrictUnchecked::default() };
    };
    let resolved = stored_resolution(header, metadata, database);
    let mut columns = Vec::new();
    for (name, plan) in header.columns.iter().zip(&resolved.comparisons) {
        let Some(def) = db.declared_column(&qualified, name) else {
            columns.push(UncheckedColumn {
                column: name.clone(),
                path: String::new(),
                declared: String::new(),
                why: Unchecked::Undeclared,
            });
            continue;
        };
        let grammar = column_grammar(plan, &def.declared_type, &db.types);
        columns.extend(
            unchecked_positions(&def.declared_type, plan, grammar, &db.types).into_iter().map(
                |(path, declared, why)| UncheckedColumn {
                    column: name.clone(),
                    path,
                    declared,
                    why,
                },
            ),
        );
    }
    let checks = db
        .table_checks(&qualified)
        .into_iter()
        .map(|(check, from)| UncheckedCheck {
            name: check.name.clone(),
            inherited_from: from.map(str::to_string),
        })
        .collect();
    let bounds = db
        .partition_bounds(&qualified)
        .into_iter()
        .map(|(partition, of)| UncheckedBound {
            partition: partition.to_string(),
            parent: of.parent.clone(),
            bound: of.bound.clone(),
        })
        .collect();
    StrictUnchecked { declared: true, columns, checks, bounds, routed_through }
}

/// One column's part of a strict parse's check ([`field_checks`]).
pub(super) struct FieldCheck {
    plan: ComparisonPlan,
    /// Read in place of `plan`, which then orders nothing.
    grammar: Option<TextGrammar>,
    declared_type: String,
    /// Whether a value is checked, by `grammar` or `plan`.
    reads: bool,
    /// Whether a NULL is refused (I76).
    not_null: bool,
}

/// The first field of the row at `offset`, the `line`-th the observer was
/// handed, that its column's check finds refused, in column order.
pub(super) fn check_row(
    checks: &[Option<FieldCheck>],
    offset: u64,
    line: u64,
    raw: &[u8],
) -> Option<FieldRefusal> {
    for (column, (field, check)) in split_fields(raw).zip(checks).enumerate() {
        let Some(check) = check else { continue };
        // A NULL is refused only by a `NOT NULL`, and a field that is not
        // text is no value at all, which no `*_in` is handed.
        let value = match decode_field(field) {
            Ok(None) if check.not_null => None,
            Ok(Some(text)) if check.reads => {
                let refused = match check.grammar {
                    Some(grammar) => grammar_refuses(grammar, &text),
                    None => field_refused(&check.plan, &text),
                };
                if !refused {
                    continue;
                }
                Some(text.into_owned())
            }
            _ => continue,
        };
        return Some(FieldRefusal {
            offset,
            line,
            column,
            declared_type: check.declared_type.clone(),
            value,
        });
    }
    None
}
