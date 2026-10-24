//! A filter literal's equality form, by the kind it is compared as: how
//! `=`/`!=` compare a field of a column against the literal, settled once from
//! the literal's text, and the test of a field against the spellings it was
//! rendered in. The ordering bound beside it is
//! [`literal_key`]'s; which of the two a term holds, and when it is timed, is
//! the filter's (`pgdt_query::predicate`).

use crate::compare::{CompareKind, ComparisonSemantics};
use crate::decode;
use crate::key::order::{OrderKey, literal_key, special_order_key};
use crate::value::datetime::{
    render_timestamp_postgres_micros, time_of_day_micros, timestamp_postgres_micros,
};
use crate::value::int::int_in;
use crate::value::network::render_macaddr;
use crate::value::numeric::numeric_in_stores;

/// How `=`/`!=` compare a field of a column against one literal.
///
/// Three canonicalizations, chosen by whether the file's `*_out` text is a
/// *unique* spelling of the value it holds (`docs/design/decisions.md`,
/// "D56"):
///
/// - **[`Equality::Canonical`] — the literal rendered once** into `*_out`
///   form, so the per-row comparison is a byte comparison. `text`'s rendering
///   is the identity.
/// - **[`Equality::Trimmed`] — the field narrowed per row.** `character(n)`
///   and bare `bpchar` alone, `bpchareq` stripping the padding from both sides (I38). It is
///   admissible on the per-row path because it is a reverse scan for `0x20`,
///   not a decode.
/// - **[`Equality::Decoded`] — both sides decoded per row**, for the kinds
///   where `*_out` is *not* injective over the values one file can hold: a
///   bare `numeric` (I33), an `interval` (I40), `jsonb` (I41), and
///   `real`/`double precision`, which have two zeros. A `numeric` whose
///   typmod `Decimal256` cannot carry shares the bare form's kind and so its
///   comparison, `time with time zone`
///   and `inet`/`cidr` decode for a reason about this build instead, and
///   `macaddr` renders (`docs/design/decisions.md`, "D56").
#[derive(Debug, Clone)]
pub enum Equality {
    /// Against the literal already rendered into each `*_out` form the file
    /// can hold it in — a byte comparison per row.
    Canonical(Spellings),
    /// On a `character(n)` column, against the literal with its own trailing
    /// blanks already gone.
    Trimmed(String),
    /// Where the field has to be decoded per row too, because the file's
    /// spelling of a value is not unique.
    Decoded { kind: CompareKind, bound: OrderKey },
}

/// A filter literal's [`Equality`] against a column compared as `kind`, in
/// `semantics`.
///
/// Deficiency register: `deficiency: KD82` — `Canonical` takes the field's
/// spelling for the value's only one, which holds of what `*_out` wrote. A
/// hand-written field the decoder reads in another spelling — `1.005` or
/// `1.0` in a `numeric(10,2)`, which a typed read rounds and pads to `1.01`
/// and `1.00`, or a braced `uuid` (I64) — is missed by `=`, `!=`, `IN` and a
/// group's dictionary, where an ordering operator and a typed read take its
/// value. Closing it means decoding a field whose bytes miss the literal's,
/// a cost every non-matching row of a dump `pg_dump` wrote would pay.
/// **(c) unowned**; promoted by a user meeting it.
///
/// `None` when the literal is not a value of the column's type at all — the
/// same refusal an ordering operator's [`literal_key`] makes, on the same
/// grammar (`docs/design/decisions.md`, "D55").
pub fn literal_equality(
    kind: &CompareKind,
    text: &str,
    semantics: ComparisonSemantics,
) -> Option<Equality> {
    use CompareKind as K;
    let rendered = match kind {
        // Both sides per row. `order_key` reads a special value on the way,
        // so `NaN = NaN` and `Infinity = Infinity` come out of the same order
        // the ordering operators use (I33, I34).
        K::Float32
        | K::Float64
        | K::Numeric { .. }
        | K::Interval { .. }
        | K::IntervalFields { .. }
        | K::Jsonb
        | K::TimeTz
        | K::Network { .. } => {
            let bound = literal_key(kind, text, semantics)?;
            return Some(Equality::Decoded { kind: kind.clone(), bound });
        }
        K::PaddedText { .. } => {
            return Some(Equality::Trimmed(text.trim_end_matches(' ').to_string()));
        }
        // A special value is written in its own type's `*_out` spelling on
        // both sides, so it renders to itself. Only `date`, `timestamp` of
        // either zone and `numeric(p,s)` reach this arm and admit one.
        _ if special_order_key(kind, text).is_some() => text.to_string(),
        K::Bool => decode::render_bool(decode::decode_bool(text)?).to_string(),
        K::Int { bytes } => match semantics {
            ComparisonSemantics::Postgres => int_in(text, *bytes).ok()?,
            ComparisonSemantics::DataFusion => text.parse::<i64>().ok()?,
        }
        .to_string(),
        K::UnsignedInt => text.parse::<u32>().ok()?.to_string(),
        K::Decimal { .. }
            if semantics == ComparisonSemantics::Postgres && !numeric_in_stores(text) =>
        {
            return None;
        }
        K::Decimal { scale, .. } => {
            decode::render_decimal(&decode::decimal_unscaled_digits(text, *scale)?, *scale)
        }
        // A label is its own canonical form; what the lookup buys is the
        // refusal, a string the type does not declare being no value of the
        // column.
        // deficiency: KD86 — the refusal is made whether or not the labels
        // were read exactly (`TypeKind::Enum`'s `exact`), so a literal naming
        // a label an unread `RENAME VALUE` or `ADD VALUE IF NOT EXISTS` gave
        // the type is refused, where the server reads it; an ordering term,
        // and any term comparing an array or composite holding the enum,
        // raises `FieldDecode` on a field naming one, having no position for
        // it. Comparing by text would answer `=` but no ordering.
        K::Enum { labels, .. } => labels.iter().find(|label| label.as_str() == text)?.clone(),
        K::Date => decode::render_date32(decode::decode_date32(text)?),
        K::Time => decode::render_time64_micros(time_of_day_micros(text).ok()?),
        K::Timestamp { with_tz } => render_timestamp_postgres_micros(
            timestamp_postgres_micros(text, *with_tz).ok()?,
            *with_tz,
        ),
        K::MacAddr { octets } => render_macaddr(text, *octets)?,
        K::Uuid => decode::render_uuid(&decode::decode_uuid(text)?),
        K::Bytea => {
            let bytes = decode::decode_bytea(text)?;
            return Some(Equality::Canonical(Spellings {
                first: decode::render_bytea(&bytes),
                other: Some(decode::render_bytea_escape(&bytes)),
            }));
        }
        // The identity: `=` on a text column is a byte comparison, with no
        // per-row work added. A literal past a `varchar(n)`'s length is
        // read, the server coercing it with no typmod.
        K::Text { .. } => text.to_string(),
    };
    Some(Equality::Canonical(Spellings::one(rendered)))
}

/// A literal in each `*_out` form a file can write it in: one, but for a
/// `bytea`, whose `bytea_output` the dump does not pin (I4), so a file holds
/// `hex` or `escape`. No text is a spelling in both (I56), so a field is the
/// literal exactly where it is one of these ([`Spellings::matches`]).
#[derive(Debug, Clone)]
pub struct Spellings {
    first: String,
    other: Option<String>,
}

impl Spellings {
    /// A literal with one spelling, `text`.
    pub fn one(text: String) -> Self {
        Self { first: text, other: None }
    }

    /// The spelling a file under the server's default settings writes.
    pub fn first(&self) -> &str {
        &self.first
    }

    /// Every spelling, the first first.
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.first).chain(&self.other)
    }

    /// Whether the field `text` is the literal: one of its spellings exactly.
    pub fn matches(&self, text: &str) -> bool {
        text == self.first || self.other.as_deref() == Some(text)
    }
}
