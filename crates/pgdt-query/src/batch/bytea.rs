//! A `bytea` column's builder, which a value is decoded into in place: Arrow's
//! `BinaryBuilder` cannot abandon a value written in pieces, so a decode that
//! may refuse would be a copy beside the one the column keeps
//! (`docs/design/decisions.md`, "D141").

use arrow::array::BinaryArray;
use arrow::array::builder::NullBufferBuilder;
use arrow::buffer::{Buffer, OffsetBuffer, ScalarBuffer};

/// The values, offsets and validity of a `Binary` column under construction,
/// laid out as `BinaryArray` holds them.
pub(crate) struct ByteaBuilder {
    values: Vec<u8>,
    /// One more than the rows: each row's value ends where the next begins.
    offsets: Vec<i32>,
    nulls: NullBufferBuilder,
}

impl ByteaBuilder {
    pub(crate) fn new() -> Self {
        Self { values: Vec::new(), offsets: vec![0], nulls: NullBufferBuilder::new(0) }
    }

    /// A builder with room for `rows` values of `bytes` bytes in all.
    pub(crate) fn with_capacity(rows: usize, bytes: usize) -> Self {
        let mut offsets = Vec::with_capacity(rows + 1);
        offsets.push(0);
        Self { values: Vec::with_capacity(bytes), offsets, nulls: NullBufferBuilder::new(rows) }
    }

    pub(crate) fn len(&self) -> usize {
        self.offsets.len() - 1
    }

    pub(crate) fn append_null(&mut self) {
        self.offsets.push(self.end());
        self.nulls.append_null();
    }

    pub(crate) fn append_value(&mut self, value: &[u8]) {
        self.append_with(|values| {
            values.extend_from_slice(value);
            true
        });
    }

    /// Append the value `write` appends to the column's values, where it
    /// answers `true`; where it answers `false` it has left them as they
    /// were, and no row is appended.
    pub(crate) fn append_with(&mut self, write: impl FnOnce(&mut Vec<u8>) -> bool) -> bool {
        if !write(&mut self.values) {
            return false;
        }
        self.offsets.push(self.end());
        self.nulls.append_non_null();
        true
    }

    /// The column, the builder emptied for the next batch.
    pub(crate) fn finish(&mut self) -> BinaryArray {
        let values = Buffer::from_vec(std::mem::take(&mut self.values));
        let offsets = std::mem::replace(&mut self.offsets, vec![0]);
        BinaryArray::new(
            OffsetBuffer::new(ScalarBuffer::from(offsets)),
            values,
            self.nulls.finish(),
        )
    }

    /// Where the values end, as an offset; past `i32::MAX` it panics, as
    /// Arrow's builder does.
    fn end(&self) -> i32 {
        i32::try_from(self.values.len()).expect("byte array offset overflow")
    }
}

#[cfg(test)]
mod tests {
    use arrow::array::Array;

    use super::*;

    #[test]
    fn a_refused_write_appends_no_row_and_the_next_batch_starts_empty() {
        let mut builder = ByteaBuilder::new();
        builder.append_value(b"ab");
        builder.append_null();
        assert!(!builder.append_with(|values| {
            values.truncate(values.len());
            false
        }));
        assert!(builder.append_with(|values| {
            values.push(b'c');
            true
        }));
        assert_eq!(builder.len(), 3);
        let array = builder.finish();
        assert_eq!(array.len(), 3);
        assert_eq!(array.value(0), b"ab");
        assert!(array.is_null(1));
        assert_eq!(array.value(2), b"c");
        assert_eq!(builder.len(), 0);
        builder.append_value(b"d");
        let array = builder.finish();
        assert_eq!((array.len(), array.value(0)), (1, &b"d"[..]));
        assert_eq!(array.null_count(), 0);
    }
}
