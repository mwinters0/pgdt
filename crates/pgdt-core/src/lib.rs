//! What every other `pgdt-*` crate reads and nothing else owns: the error
//! type, the `introspect` hooks, the SQL lexer and the `COPY` text format
//! (`docs/design/decisions.md`, "D68"). The error type names no dependency's
//! error; a dependency's is a cause it carries opaquely (D74).

mod copy;
mod error;
pub mod instrument;
mod lex;

pub use copy::{
    COPY_BLOCK_TERMINATOR, COPY_TEXT_DELIMITER, CopyHeader, FieldRanges, RawRow, RowSplit,
    decode_field, encode_field, field_ranges, is_terminator, parse_copy_header, split_fields,
    validated_prefix,
};
pub use error::{Error, RowEndingRefusal, Unusable};

pub type Result<T> = std::result::Result<T, Error>;

/// What a crate above reaches that is not this crate's API: public only
/// because a crate boundary forced it, and named here so a caller reaching
/// past the API names the word.
pub mod internal {
    pub use crate::copy::{Cursor, hex_val};
    pub use crate::error::OVERWRITE_WAYS_OUT;
    pub use crate::instrument::hooks::{
        DecodeCounter, StatisticsScope, statistics_account_updated,
    };
    #[cfg(feature = "introspect")]
    pub use crate::instrument::hooks::{evaluated_row, statistics_loaded, timed_span};
    pub use crate::lex::{
        Lexer, LineLex, Quote, Region, canonical_type_name, ident_cont, parse_type_name,
        standard_conforming_strings,
    };
    pub use crate::{row_evaluated, timed};
}
