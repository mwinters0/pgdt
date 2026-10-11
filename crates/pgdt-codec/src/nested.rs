//! The nested literal: arrays, records, ranges, multiranges and
//! `int2vector`, one grammar with two scanners (`docs/design/decisions.md`,
//! "D45"), read and written as text. Their order is `crate::key`'s.

pub use crate::value::nested::{
    ArrayLiteral, RangeLiteral, RecordLiteral, decode_array, decode_int2vector, decode_multirange,
    decode_range, decode_record, parse_array, parse_array_delimited, parse_int2vector,
    parse_multirange, parse_range, parse_record, render_array, render_int2vector,
    render_multirange, render_range, render_record,
};
