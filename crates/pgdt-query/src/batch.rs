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
mod bytea;
mod chunks;

#[cfg(test)]
pub(crate) use self::builder::column_of;
use self::builder::reserved_column_builder;
pub(crate) use self::builder::{
    ColumnBuilder, Factor, append_typed, column_factor, finish_column, new_column_builder,
};
pub(crate) use self::chunks::RetainedChunks;

use std::ops::Range;

use arrow::array::{ArrayRef, RecordBatch, RecordBatchOptions};
use arrow::datatypes::{DataType, SchemaRef};

use pgdt_codec::ResolvedSchema;
use pgdt_codec::internal::{FieldText, UnrepresentableRead, decode_bytea_into, escaped_utf8_error};
use pgdt_codec::pgtype::NestedPlan;
use pgdt_core::internal::{RawField, RawRow, RowSplit, printout, printout_head, unescaped};
use pgdt_core::{Error, Result};
use pgdt_model::PostgresInvalidValues;

use self::builder::{append_null, append_walked, is_walked, text_refused};
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
    /// Rows a batch is flushed at: the stream's, not the query's.
    max_rows: usize,
    /// Parallel to `columns`: each column's type and plan, which a walked
    /// value longer than `long_field` is decoded into builders reserved
    /// from ([`reserved_column_builder`]).
    plans: Vec<(DataType, NestedPlan)>,
    /// Whether any column's value is walked ([`Self::walks_column`]).
    walks: bool,
    /// The length past which a walked value is long — its read's chunk, the
    /// term a line past it draws beyond.
    long_field: usize,
    /// Whether the in-flight batch holds a long walked value, so it flushes
    /// after its row and no later row grows its reserved builders.
    holds_long: bool,
}

impl RowBatcher {
    /// `resolved` is the **projected** schema — what this batcher's
    /// `RecordBatch`es carry — and `field_targets` maps the block's own
    /// fields onto it. Both come from `crate::stream::replay::plan::project`;
    /// `unrepresentable` is parallel to `field_targets`, `None` for each
    /// field that is not tested. A batch is flushed at `max_rows` rows, or
    /// where `options` bounds its bytes or its span first, and around a
    /// nested value longer than `long_field` ([`Self::cuts_before`]).
    pub(crate) fn new(
        resolved: &ResolvedSchema,
        table: String,
        options: QueryOptions,
        max_rows: usize,
        long_field: usize,
        field_targets: Vec<Option<usize>>,
        unrepresentable: Vec<Option<UnrepresentableRead>>,
    ) -> Self {
        let schema = resolved.schema().clone();
        let mut projected = vec![None; schema.fields().len()];
        for (field, target) in field_targets.iter().enumerate() {
            if let Some(target) = *target {
                projected[target] = unrepresentable.get(field).cloned().flatten();
            }
        }
        let declared_types =
            resolved.columns().iter().map(|c| c.declared().map(str::to_string)).collect();
        let columns: Vec<ColumnBuilder> = resolved
            .columns()
            .iter()
            .map(|column| new_column_builder(column.data_type(), column.plan()))
            .collect();
        let walks = columns.iter().any(is_walked);
        let plans =
            resolved.columns().iter().map(|c| (c.data_type().clone(), c.plan().clone())).collect();
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
            not_null: resolved.columns().iter().map(|column| column.not_null()).collect(),
            options,
            max_rows,
            plans,
            walks,
            long_field,
            holds_long: false,
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
        self.holds_long
            || self.rows_in_batch >= self.max_rows
            || self.options.max_bytes.is_some_and(|max| self.bytes_in_batch >= max)
            || self.source_span_reached()
    }

    /// **Whether the in-flight batch must be flushed before `row`**: it holds
    /// a row, and `row` has a nested value its batch walks longer than
    /// `long_field`, which is decoded into builders reserved to it, so they
    /// must start empty (`docs/design/decisions.md`, "D154"). The row's
    /// boundaries are `split`'s, which [`Self::push_row`] reads again.
    pub(crate) fn cuts_before(&self, row: RawRow<'_>, split: &mut RowSplit) -> bool {
        let raw = row.bytes();
        if !self.walks || self.rows_in_batch == 0 || raw.len() <= self.long_field {
            return false;
        }
        let mut start = 0;
        split.complete(raw).iter().zip(&self.field_targets).any(|(&end, target)| {
            let long = end - start > self.long_field
                && target.is_some_and(|target| self.walks_column(target));
            start = end + 1;
            long
        })
    }

    /// Whether column `col`'s value is walked into its builders
    /// ([`append_walked`]): a nested column, which its tier test, where it
    /// has one, walks first.
    fn walks_column(&self, col: usize) -> bool {
        is_walked(&self.columns[col])
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
    /// per column — `RawRow::decode` and the builder append — never an early
    /// stop at the last projected field, because this is the system's only
    /// field-count check, beside a counted row's: the two raise
    /// `Error::ColumnCountMismatch` alone, through `hold_to_width`, the mapping
    /// pass never erroring on a
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
        self.hold_to_width(header_offset, row_offset, ends.len())?;
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
        self.count_row(row_offset, row_offset + raw.len() as u64);
        Ok(())
    }

    /// Append one row its read loop counted without holding its bytes, its
    /// `fields` counted as they went by (`pgdt_dump::StreamedRow`), `len`
    /// bytes long with its line ending: a batcher decoding no field takes it
    /// as [`push_row`](Self::push_row) takes a row, held to the block's
    /// width alike (`docs/design/decisions.md`, "D28").
    pub(crate) fn push_counted_row(
        &mut self,
        header_offset: u64,
        row_offset: u64,
        fields: usize,
        len: u64,
    ) -> Result<()> {
        debug_assert!(!self.decodes_fields, "a row whose fields decode was counted");
        self.hold_to_width(header_offset, row_offset, fields)?;
        self.count_row(row_offset, row_offset + len);
        Ok(())
    }

    /// Refuse a row of `found` fields unless the block's own column list is
    /// that wide: the one width check a row meets, whether its fields were
    /// split or counted.
    #[inline]
    fn hold_to_width(&self, header_offset: u64, row_offset: u64, found: usize) -> Result<()> {
        let expected = self.field_targets.len();
        if found != expected {
            return Err(Error::ColumnCountMismatch {
                header_offset,
                row_offset,
                expected,
                // Saturated one past the width, though every field of the row
                // was counted.
                found: if found > expected { expected + 1 } else { found },
            });
        }
        Ok(())
    }

    /// Take a row from `row_offset` to `row_end` into the batch and its span.
    #[inline]
    fn count_row(&mut self, row_offset: u64, row_end: u64) {
        self.rows_in_batch += 1;
        self.span = Some(match self.span {
            Some((start, _)) => (start, row_end),
            None => (row_offset, row_end),
        });
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
        // A `bytea` is decoded from its field into its column, no unescaped
        // text of it made (`docs/design/decisions.md`, "D141"); a NULL, or a
        // field that reads as no value, goes on below, as any type's does.
        if let ColumnBuilder::Binary(b) = &mut self.columns[col]
            && self.unrepresentable[col].is_none()
            && let Some(text) = unescaped(&row.bytes()[field.clone()])
            && b.append_with(|values| decode_bytea_into(text, values))
        {
            return Ok(());
        }
        // A nested value is walked off its field into its builders, an
        // escaped one unescaped as it is read, so no text of it is made
        // (`docs/design/decisions.md`, "D153"); a NULL goes on below.
        let walked = self.walks_column(col);
        let invalid = self.options.postgres_invalid_values;
        let read = walked.then(|| row.read(field.clone())).transpose()?;
        let text = match read {
            Some(RawField::Escaped(bytes)) => {
                if let Some(valid_up_to) = escaped_utf8_error(bytes) {
                    return Err(Error::InvalidUtf8 { valid_up_to });
                }
                Some(FieldText::escaped(bytes))
            }
            Some(RawField::Plain(text)) => Some(FieldText::plain(text)),
            Some(RawField::Null) | None => None,
        };
        if let Some(text) = text {
            // One whose tier test finds a leaf the query cannot hold is NULL
            // whole, the test a walk of its own, staging nothing
            // (`docs/design/decisions.md`, "D153").
            if let Some(tier) = &self.unrepresentable[col]
                && tier.cannot_hold_walked(text.clone())
            {
                append_null(&mut self.columns[col]);
                return Ok(());
            }
            // A long one, in a batch holding no row, is decoded into builders
            // reserved to it, its batch flushed after its row, so no buffer of
            // its copy grows by doubling (`docs/design/decisions.md`, "D154").
            if field_len > self.long_field && self.rows_in_batch == 0 {
                let (data_type, plan) = &self.plans[col];
                self.columns[col] = reserved_column_builder(data_type, plan, field_len);
                self.holds_long = true;
            }
            return append_walked(&mut self.columns[col], text, invalid).map_err(|()| {
                let value = match read {
                    Some(RawField::Escaped(bytes)) => printout_head(bytes),
                    Some(RawField::Plain(text)) => Some(printout(text)),
                    Some(RawField::Null) | None => None,
                };
                self.field_decode(col, row_offset, value)
            });
        }
        let decoded = row.decode(field)?;
        // Disjoint-field borrow: `columns[col]` is mutated below while
        // `schema`/`table` are only ever read, on the (rare) error path.
        let Self { schema, table, columns, unrepresentable, not_null, options, .. } = self;
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
        // The scan's own path for a text column: a view into the retained
        // read buffer where it can be one, ahead of `append_typed`'s copy,
        // which also answers the text its length typmod refuses.
        if let ColumnBuilder::Utf8View(b, length) = builder
            && !text_refused(&text, *length, options.postgres_invalid_values)
        {
            push_utf8view_field(b, col, field_offset, field_len, text, chunks);
            return Ok(());
        }
        append_typed(builder, &text, options.postgres_invalid_values)
            .map_err(|()| self.field_decode(col, row_offset, Some(printout(&text))))
    }

    /// The `Error::FieldDecode` projected column `col` raises over the
    /// field of the row at `row_offset` that does not decode, quoted as
    /// `value` says.
    fn field_decode(&self, col: usize, row_offset: u64, value: Option<String>) -> Error {
        Error::FieldDecode {
            table: self.table.clone(),
            column: self.schema.field(col).name().clone(),
            line_offset: row_offset,
            declared_type: self.declared_types[col].clone().unwrap_or_default(),
            value: value.unwrap_or_default(),
        }
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
        self.holds_long = false;
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
