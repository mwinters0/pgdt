//! A strict parse's check (`docs/design/decisions.md`, "D103"): what the
//! observer [`checker`](super::checker) builds checks every field of a block
//! by — its type's `*_in` and its column's `NOT NULL` — and the listing of
//! what it does not check.

use std::sync::Arc;

use pgdt_codec::ResolvedSchema;
use pgdt_codec::internal::{FieldWalk, field_refused, text_typmod, walks};
use pgdt_codec::{
    compare::ComparisonPlan,
    grammar::{TextGrammar, Unchecked},
    internal::{column_grammar, unchecked_positions},
};
use pgdt_core::internal::{PRINTOUT_ESCAPED_BYTES, printout_head, unescaped};
use pgdt_core::{CopyHeader, unescape_field};
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
    let types = metadata.and_then(|m| m.database(database)).map_or(&[][..], |db| &db.types[..]);
    header
        .columns
        .iter()
        .zip(resolved.columns())
        .map(|(name, column)| {
            let (plan, not_null) = (column.comparison(), column.not_null());
            let declared_type = declared_column(metadata, database, &qualified, name)
                .map(|def| def.declared_type.clone())
                .unwrap_or_default();
            let grammar = column_grammar(plan, &declared_type, types);
            let reads = grammar.is_some()
                || !matches!(plan, ComparisonPlan::Refused | ComparisonPlan::Unanswerable(_));
            let typmod = grammar.is_none().then(|| text_typmod(plan)).flatten();
            let walks = grammar.is_some() || walks(plan);
            (reads || not_null).then(|| FieldCheck {
                typmod,
                walks,
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
/// in `metadata` for `database`, the one the block's span records, as the parse's own check is
/// (`field_checks`): the same resolution, the same plan and grammar per
/// column, so the two cannot disagree.
///
/// A property of the dump's declarations and the block's own framing, not of
/// any run: the same for a block a strict parse checked and one it never
/// read.
pub fn strict_unchecked(
    block: &CopyBlock,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> StrictUnchecked {
    let header = &block.header;
    let qualified = header.qualified_name();
    let db = metadata.and_then(|m| m.database(database));
    let routed =
        block.partition_root.is_some() || db.is_some_and(|db| db.has_partitions(&qualified));
    let routed_through = routed.then(|| qualified.clone());
    let Some(db) = db.filter(|db| db.tables.contains_key(&qualified)) else {
        return StrictUnchecked { routed_through, ..StrictUnchecked::default() };
    };
    let resolved = stored_resolution(header, metadata, database);
    let mut columns = Vec::new();
    for (name, plan) in header.columns.iter().zip(resolved.columns().iter().map(|c| c.comparison()))
    {
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
    /// Where `plan` refuses a field only past a text's typmod, its length
    /// ([`text_typmod`]).
    typmod: Option<Option<u32>>,
    /// Whether a field is read off its bytes where it lies: by `grammar`, or
    /// by `plan` where it [`walks`] — a `bytea`, a character type, a
    /// nested, `json` or `jsonb` value.
    walks: bool,
}

impl FieldCheck {
    /// Whether a field is read whole, never as its runs go by: one whose
    /// value `plan` checks, a scalar's text a key reads, and that it does not
    /// walk.
    pub(super) fn holds(&self) -> bool {
        self.reads && !self.walks
    }

    /// The walk a field of this column is read by, where it walks — one per
    /// column, readied for each field as the last ends.
    pub(super) fn walk(&self) -> Option<FieldWalk> {
        self.walks.then(|| match self.grammar {
            Some(grammar) => FieldWalk::grammar(grammar),
            None => FieldWalk::new(&self.plan),
        })
    }

    /// The refusal of a field this column's check refuses, quoted as `value`,
    /// at the row at `offset`, the `line`-th the observer was handed.
    pub(super) fn refusal(
        &self,
        offset: u64,
        line: u64,
        column: usize,
        value: Option<String>,
    ) -> FieldRefusal {
        FieldRefusal { offset, line, column, declared_type: self.declared_type.clone(), value }
    }

    /// Whether this column's check refuses `field`, held whole, `walk` being
    /// its [`walk`](Self::walk) where it walks: `Some` of the field as a
    /// restore's error quotes it (I94) where it does.
    pub(super) fn refused(
        &self,
        walk: Option<&mut FieldWalk>,
        field: &[u8],
    ) -> Option<Option<String>> {
        // A field read off its bytes is checked with no unescaped text, copy
        // or tree of it made, in one reading of each byte (D142–D144) — a
        // text whose escaped length, no shorter than its text, is within its
        // typmod read not at all — and named as a restore's error quotes it
        // (I94) where refused, which a field that is not UTF-8 is not, being
        // no value, which no `*_in` is handed. A NULL is answered below.
        if let Some(walk) = walk
            && unescaped(field).is_some()
        {
            if self.within_typmod(field.len()) {
                return None;
            }
            walk.run(field);
            return walk.finish().then(|| printout_head(field));
        }
        // A NULL is refused only by a `NOT NULL`, and a field that is not
        // text is no value at all, which no `*_in` is handed.
        match unescape_field(field) {
            Ok(None) if self.not_null => Some(None),
            // Named as a restore's error quotes it (I94).
            Ok(Some(text)) if self.reads && field_refused(&self.plan, &text) => {
                Some(printout_head(field))
            }
            _ => None,
        }
    }

    /// [`Self::refused`] of a field `escaped` bytes long that `walk` was fed
    /// as its runs went by — never SQL `NULL`, and never one this column
    /// [`holds`](Self::holds) — `head` being its first bytes, no fewer than
    /// [`PRINTOUT_ESCAPED_BYTES`] of them, which it is quoted by.
    pub(super) fn refused_read(
        &self,
        walk: Option<&mut FieldWalk>,
        escaped: usize,
        head: &[u8],
    ) -> Option<Option<String>> {
        debug_assert!(!self.holds(), "a field read whole");
        debug_assert!(head.len() >= PRINTOUT_ESCAPED_BYTES.min(escaped), "a head it is quoted by");
        let refused = walk.is_some_and(|walk| walk.finish());
        (refused && !self.within_typmod(escaped)).then(|| printout_head(head))
    }

    /// Whether a field `escaped` bytes long is within this column's text
    /// typmod, its text being no longer.
    fn within_typmod(&self, escaped: usize) -> bool {
        self.typmod.is_some_and(|length| length.is_none_or(|n| escaped <= n as usize))
    }
}
