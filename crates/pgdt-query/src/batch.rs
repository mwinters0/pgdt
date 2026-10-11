//! Row/batch assembly: turns rows inside a `COPY` block into typed Arrow
//! `RecordBatch`es, one column builder per [`pgdt_codec::ResolvedSchema`]
//! field (`docs/design/decisions.md`, "D46").
//!
//! A `Utf8View` field that needs no unescaping is appended as a zero-copy
//! view into the Arrow `Buffer` backing the read chunk it came from, rather
//! than copied into the builder's own storage, the chunks those views point
//! into being held by [`chunks`]. Every other mapped type always copies: its
//! decoded value has its own representation (an `i32`, a `[u8; 16]`, …), not
//! a byte range of the original field ([`builder`]).

mod builder;
mod chunks;

#[cfg(test)]
pub(crate) use self::builder::column_of;
pub(crate) use self::builder::{
    ColumnBuilder, append_typed, finish_column, new_column_builder, text_refused,
};
pub(crate) use self::chunks::RetainedChunks;

use std::ops::Range;

use arrow::array::{ArrayRef, RecordBatch, RecordBatchOptions};
use arrow::datatypes::SchemaRef;

use pgdt_codec::ResolvedSchema;
use pgdt_codec::internal::UnrepresentableRead;
use pgdt_core::{Error, Result};
use pgdt_core::{RawRow, RowSplit};
use pgdt_model::PostgresInvalidValues;

use self::builder::append_null;
use self::chunks::push_utf8view_field;
use crate::options::QueryOptions;

/// Accumulates rows from a single `COPY` block into typed `RecordBatch`es,
/// one [`ColumnBuilder`] per field of the query's
/// [`pgdt_codec::ResolvedSchema`].
pub(crate) struct RowBatcher {
    schema: SchemaRef,
    /// Qualified table name, for `Error::FieldDecode`'s context.
    table: String,
    /// One entry per field of the **block's own** column list, in file
    /// order: the `columns` index that field feeds, or `None` for a field no
    /// projection asked for. Its length — not `columns.len()` — is the field
    /// count a row is checked against.
    field_targets: Vec<Option<usize>>,
    /// Parallel to `schema.fields()` — the declared PostgreSQL type string
    /// behind each `Mapped` column, for the same error.
    declared_types: Vec<Option<String>>,
    columns: Vec<ColumnBuilder>,
    rows_in_batch: usize,
    bytes_in_batch: usize,
    /// The source byte span the in-flight batch covers: `Some((start, end))`
    /// once a row has been pushed, where `start` is the first pushed row's
    /// offset and `end` is one past the latest pushed row's last byte. A row
    /// a predicate rejected never reaches `push_row`, so it neither opens a
    /// span nor extends one, which is what stops an empty batch from flushing
    /// across a long stretch that matches nothing.
    span: Option<(u64, u64)>,
    /// Whether any field of this block feeds a projected column. When
    /// nothing does — `COUNT(*)`, `pgdt query --no-columns` — the batcher
    /// decodes no field, and where the filter reads none either the read loop
    /// skips the bulk UTF-8 validation (`docs/design/decisions.md`, "D27").
    decodes_fields: bool,
    /// Parallel to `columns`: how each projected column's values its type
    /// cannot hold are read, where it can hold one
    /// ([`pgdt_codec::internal::unrepresentable_reads`]).
    unrepresentable: Vec<Option<UnrepresentableRead>>,
    /// Parallel to `columns`: whether each column's declaration makes it `NOT
    /// NULL`, so a NULL in it is refused ([`Error::NullRefused`]).
    not_null: Vec<bool>,
    options: QueryOptions,
}

impl RowBatcher {
    /// `resolved` is the **projected** schema — what this batcher's
    /// `RecordBatch`es carry — and `field_targets` maps the block's own
    /// fields onto it. Both come from `crate::stream::replay::plan::project`;
    /// `unrepresentable` is parallel to `field_targets`, `None` for each
    /// field that is not tested.
    pub(crate) fn new(
        resolved: &ResolvedSchema,
        table: String,
        options: QueryOptions,
        field_targets: Vec<Option<usize>>,
        unrepresentable: Vec<Option<UnrepresentableRead>>,
    ) -> Self {
        let schema = resolved.schema.clone();
        let mut projected = vec![None; schema.fields().len()];
        for (field, target) in field_targets.iter().enumerate() {
            if let Some(target) = *target {
                projected[target] = unrepresentable.get(field).cloned().flatten();
            }
        }
        let declared_types = resolved.notes.iter().map(|n| n.declared.clone()).collect();
        // `plans` is positional and parallel to `schema.fields()`:
        // `resolve_columns` fills one entry per column, `NestedPlan::Scalar`
        // included.
        let columns = schema
            .fields()
            .iter()
            .zip(&resolved.plans)
            .map(|(f, plan)| new_column_builder(f.data_type(), plan))
            .collect();
        Self {
            schema,
            table,
            decodes_fields: field_targets.iter().any(Option::is_some),
            field_targets,
            declared_types,
            columns,
            rows_in_batch: 0,
            bytes_in_batch: 0,
            span: None,
            unrepresentable: projected,
            not_null: resolved.not_null.clone(),
            options,
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows_in_batch == 0
    }

    /// Where in the source the in-flight batch begins — the offset of its
    /// first pushed row — or `None` while no row has landed in it. It is the
    /// `span`'s lower bound, so read it **before** [`Self::flush`], which
    /// clears the span. It is the key a caller merging several partitions of
    /// one replay back into file order sorts on
    /// (`docs/design/decisions.md`, "D51").
    pub(crate) fn batch_start(&self) -> Option<u64> {
        self.span.map(|(start, _)| start)
    }

    /// The block's qualified table name — context for the
    /// `Error::FieldDecode` an ordering predicate raises on a value that is
    /// not of its mapped type, worded as this batcher words its own.
    pub(crate) fn table(&self) -> &str {
        &self.table
    }

    /// Whether pushing a row here decodes anything — see the field.
    pub(crate) fn decodes_fields(&self) -> bool {
        self.decodes_fields
    }

    /// Whether any of the three flush triggers has fired. All three are
    /// evaluated after a row has been appended, so each may be overshot by at
    /// most one row. The span trigger cannot fire on an empty batch whatever
    /// its cap, `span` staying `None` until a row lands.
    pub(crate) fn should_flush(&self) -> bool {
        self.rows_in_batch >= self.options.max_rows
            || self.options.max_bytes.is_some_and(|max| self.bytes_in_batch >= max)
            || self.source_span_reached()
    }

    fn source_span_reached(&self) -> bool {
        let (Some(max), Some((start, end))) = (self.options.max_source_span, self.span) else {
            return false;
        };
        end.saturating_sub(start) >= max as u64
    }

    /// Append one raw (still-escaped) COPY TEXT data row.
    ///
    /// **The whole row is walked whatever the projection is.** Skipping is
    /// per column — `decode_field` and the builder append — never an early
    /// stop at the last projected field, because this is the system's only
    /// field-count check: the sole site that raises
    /// `Error::ColumnCountMismatch`, the mapping pass never erroring on a
    /// count (`docs/design/decisions.md`, "D28").
    ///
    /// **The walk is `split`'s**, which the caller has already offered to the
    /// filter, so a boundary a term crossed is not crossed again here and one
    /// nothing read is found now. There is no second entry point walking the
    /// row directly for the unfiltered case
    /// (`docs/design/decisions.md`, "D28"; `measurements.md`,
    /// `predicate-terms`).
    ///
    /// **A block of zero fields takes only empty lines** — one listing no
    /// columns (I5) — where `split` reads an empty line as one empty field,
    /// so any other line is the mismatch it is against a wider block.
    pub(crate) fn push_row(
        &mut self,
        header_offset: u64,
        row_offset: u64,
        row: RawRow<'_>,
        split: &mut RowSplit,
        chunks: &mut RetainedChunks,
    ) -> Result<()> {
        let raw = row.bytes();
        let expected = self.field_targets.len();
        let ends: &[usize] =
            if expected == 0 && raw.is_empty() { &[] } else { split.complete(raw) };
        let found = ends.len();
        if found != expected {
            return Err(Error::ColumnCountMismatch {
                header_offset,
                row_offset,
                expected,
                // Saturated one past the width, though the split counted
                // every field of the row.
                found: if found > expected { expected + 1 } else { found },
            });
        }
        let mut pos = 0usize;
        for (col, &end) in ends.iter().enumerate() {
            if let Some(target) = self.field_targets[col] {
                self.push_field(
                    target,
                    row_offset,
                    row_offset + pos as u64,
                    row,
                    pos..end,
                    chunks,
                )?;
                self.bytes_in_batch += end - pos;
            }
            pos = end + 1;
        }
        self.rows_in_batch += 1;
        let row_end = row_offset + raw.len() as u64;
        self.span = Some(match self.span {
            Some((start, _)) => (start, row_end),
            None => (row_offset, row_end),
        });
        Ok(())
    }

    /// Append one field's text to projected column `col`, a NULL its
    /// column's `NOT NULL` refuses being [`Error::NullRefused`].
    ///
    /// **A value the query reads as NULL is NULL before it is decoded**, as a
    /// whole: an array, range or composite holding one leaf its type cannot
    /// hold is the NULL, a NULL range bound meaning unbounded
    /// (`docs/design/decisions.md`, "D98"). The refuse mode's plan refused
    /// any column holding one before a row was read, so a value that does not
    /// decode here does not parse: [`Error::FieldDecode`].
    fn push_field(
        &mut self,
        col: usize,
        row_offset: u64,
        field_offset: u64,
        row: RawRow<'_>,
        field: Range<usize>,
        chunks: &mut RetainedChunks,
    ) -> Result<()> {
        let field_len = field.len();
        let decoded = row.decode(field)?;
        // Disjoint-field borrow: `columns[col]` is mutated below while
        // `schema`/`table`/`declared_types` are only ever read, on the
        // (rare) error path.
        let Self {
            schema, table, declared_types, columns, unrepresentable, not_null, options, ..
        } = self;
        let builder = &mut columns[col];
        let Some(text) = decoded else {
            if not_null[col] && options.postgres_invalid_values != PostgresInvalidValues::Ignore {
                return Err(Error::NullRefused {
                    table: table.clone(),
                    column: schema.field(col).name().clone(),
                    line: None,
                    line_offset: row_offset,
                });
            }
            append_null(builder);
            return Ok(());
        };
        let read = unrepresentable[col].as_ref();
        if read.is_some_and(|read| read.cannot_hold(&text)) {
            append_null(builder);
            return Ok(());
        }
        let appended = match builder {
            ColumnBuilder::Utf8View(_, length)
                if text_refused(&text, *length, options.postgres_invalid_values) =>
            {
                Err(text.into_owned())
            }
            ColumnBuilder::Utf8View(b, _) => {
                push_utf8view_field(b, col, field_offset, field_len, text, chunks);
                Ok(())
            }
            _ => append_typed(builder, &text, options.postgres_invalid_values),
        };
        appended.map_err(|value| Error::FieldDecode {
            table: table.clone(),
            column: schema.field(col).name().clone(),
            line_offset: row_offset,
            declared_type: declared_types[col].clone().unwrap_or_default(),
            value,
        })
    }

    /// Finish the in-flight batch. The row count is passed explicitly rather
    /// than inferred from the arrays, since a zero-column projection has none
    /// to infer it from; stating it unconditionally keeps one path for both
    /// widths, and `try_new_with_options` still checks every array against
    /// it.
    pub(crate) fn flush(&mut self) -> Result<RecordBatch> {
        let rows = self.rows_in_batch;
        self.rows_in_batch = 0;
        self.bytes_in_batch = 0;
        self.span = None;
        let arrays: Vec<ArrayRef> = self.columns.iter_mut().map(finish_column).collect();
        RecordBatch::try_new_with_options(
            self.schema.clone(),
            arrays,
            &RecordBatchOptions::new().with_row_count(Some(rows)),
        )
        .map_err(Error::arrow)
    }
}

#[cfg(test)]
mod tests;
