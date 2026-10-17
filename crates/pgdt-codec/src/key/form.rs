//! The form a filter's literal has to take, by the kind it is compared as:
//! what [`literal_key`](crate::key::order::literal_key) and its nested counterpart
//! read, written as the text a refusal naming a literal quotes.

use crate::compare::{CompareKind, NestedCompare};
use crate::value::int::int_range;

/// The form a **nested** literal has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence: the container's own grammar, then
/// how to spell what is inside it — as the dump does, which is each leaf
/// type's own output form.
///
/// That second half is advice rather than the boundary of what is accepted:
/// [`order_key`]'s integer arms are `str::parse`, so they take a leading `+`
/// and leading zeros no `*_out` writes, one of the exceptions [`accepted_form`]
/// lists (`docs/design/decisions.md`, "D55"). There is no per-leaf clause list, [`nested_key`]
/// answering only "not a value of this type".
///
/// [`order_key`]: crate::key::order::order_key
/// [`nested_key`]: crate::value::nested::key::nested_key
pub fn nested_accepted_form(plan: &NestedCompare) -> String {
    // The one form with no leaf clause to add, because it has no leaf: an
    // `int2vector`'s elements are read by `int2vectorin` itself, so the
    // superset reaches all the way down and the sentence below would be false
    // here.
    if matches!(plan, NestedCompare::Int2Vector { .. }) {
        return "as whole numbers from -32768 to 32767 separated by spaces, and as nothing at \
                all for the empty vector"
            .to_string();
    }
    let container = match plan {
        NestedCompare::Array { element, .. } if element.not_null => {
            "as an array literal — `{a,b}`, `{}` — with no `NULL` element, which its element \
             type, a `NOT NULL` domain, refuses"
        }
        NestedCompare::Array { .. } => "as an array literal — `{a,b}`, `{}`, a bare `NULL` element",
        NestedCompare::Record { fields, .. } if fields.iter().any(|(_, field)| field.not_null) => {
            "as a composite literal — `(a,b)`, a field left empty for NULL but where its type is \
             a `NOT NULL` domain"
        }
        NestedCompare::Record { .. } => {
            "as a composite literal — `(a,b)`, a field left empty for NULL"
        }
        NestedCompare::Range { .. } => {
            "as a range literal — `[a,b)`, `empty`, a bound left empty for unbounded — whose \
             lower bound is not above its upper"
        }
        NestedCompare::Multirange { .. } => {
            "as a multirange literal — `{[a,b),[c,d)}`, `{}` — each member a range whose lower \
             bound is not above its upper"
        }
        // Answered above, before the leaf clause this arm cannot carry.
        NestedCompare::Int2Vector { .. } => "as an int2vector literal",
        // Neither is reachable: a leaf plan is never a column's whole
        // comparison, and an uncomparable one refuses before a literal is
        // read.
        NestedCompare::Leaf { .. } | NestedCompare::Uncomparable { .. } => "as a nested literal",
    };
    format!(
        "{container} — with each element, field or bound spelled as the dump spells it, in that \
         type's own output form"
    )
}

/// The form a literal of `kind` has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence — read after "which is written".
///
/// It lives beside the grammar rather than beside [`CompareKind`] because it
/// describes what [`literal_key`] and `pgdt_query::predicate::resolve::equality_comparison` accept, which is
/// each type's `*_out` form, widened by an integer's sign and leading zeros,
/// a float's sign, bare point, exponent case and `inf` or `nan` in any case,
/// a `uuid` or `macaddr` hex digit's case, a `uuid`'s omitted hyphens,
/// hyphens after any group of four and enclosing braces,
/// either `numeric` kind's leading or trailing point and leading zeros, a
/// network value's full-width or zero-padded netmask, an IPv6 address's
/// uncompressed, zero-padded or upper-case groups, a `cidr` written with no
/// netmask, and a time's trailing fractional zeros (`docs/design/decisions.md`, "D55").
/// `jsonb` needs the least here, its grammar being the whole of `jsonb_in`.
///
/// Two arms answer with the kind's own payload, because there the payload
/// *is* the answer: an enum's declared labels, and a `numeric(p,s)`'s scale
/// — see [`ENUM_LABELS_SHOWN`] and [`decimal_accepted_form`].
///
/// [`CompareKind::Text`] and [`CompareKind::PaddedText`] never refuse a
/// literal, and an enum with no labels is refused by [`comparison_for`]
/// outright, so both arms are unreachable; they are written out rather than
/// `unreachable!` so a future kind's mistake is not a panic on a diagnostic
/// path.
///
/// [`literal_key`]: crate::key::order::literal_key
/// [`comparison_for`]: crate::compare::comparison_for
pub fn accepted_form(kind: &CompareKind) -> String {
    use CompareKind as K;
    match kind {
        K::Bool => "`t` or `f`".into(),
        // The width is half the sentence, as for `oid`: past it is the
        // refusal a well-formed literal meets (I60).
        K::Int { bytes } => {
            let range = int_range(*bytes);
            format!("as a whole number from {} to {}", range.start(), range.end())
        }
        // The width *is* the refusal: `oidin` wraps a negative and this does
        // not, so the range is the useful half of the sentence.
        K::UnsignedInt => "as a whole number from 0 to 4294967295".into(),
        K::Float32 => {
            "as a number within `real`'s range, or `Infinity`, `-Infinity` or `NaN`".into()
        }
        K::Float64 => {
            "as a number within `double precision`'s range, or `Infinity`, `-Infinity` or `NaN`"
                .into()
        }
        // A typmod rejects an infinity (I34), so a `numeric(p,s)` — and a
        // `numeric` past 76 digits — has `NaN` and nothing else. Only the
        // typed arm carries a scale to be finer than: a `p > 76` column is
        // held as text and compared by value through `NumericKey`, which
        // normalizes rather than rescaling.
        // The digit bounds are `numeric_in`'s storage format's (I63), which
        // a typed arm's scale and `i256` already fall inside.
        K::Decimal { scale, .. } => decimal_accepted_form(*scale),
        K::Numeric { infinities: false, .. } => format!("{NUMERIC_DIGITS}, or `NaN`"),
        K::Numeric { infinities: true, .. } => {
            format!("{NUMERIC_DIGITS}, or `NaN`, `Infinity` or `-Infinity`")
        }
        K::Enum { labels, .. } if !labels.is_empty() => enum_accepted_form(labels),
        K::Enum { .. } => "as one of the type's own declared labels".into(),
        // The range is half the sentence, as for an integer: a day the
        // calendar lacks, or one past it, is the refusal a well-formed
        // literal meets (I61).
        K::Date => "`YYYY-MM-DD`, optionally suffixed ` BC`, a day from `4714-11-24 BC` to \
                    `5874897-12-31`, or `infinity`/`-infinity`"
            .into(),
        K::Time => {
            "`HH:MM:SS`, optionally with a fractional second, no later than `24:00:00`".into()
        }
        K::Timestamp { with_tz: false } => {
            "`YYYY-MM-DD HH:MM:SS`, optionally with a fractional second and suffixed ` BC`, from \
             `4714-11-24 00:00:00 BC` to `294276-12-31 23:59:59.999999`, or \
             `infinity`/`-infinity`"
                .into()
        }
        K::Timestamp { with_tz: true } => {
            "`YYYY-MM-DD HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, \
             optionally with a fractional second and suffixed ` BC`, from `4714-11-24 00:00:00+00 \
             BC` to `294276-12-31 23:59:59.999999+00`, or `infinity`/`-infinity`"
                .into()
        }
        K::TimeTz => {
            "`HH:MM:SS+HH`, the offset required and at most `15:59:59` either way, optionally \
             with a fractional second, no later than `24:00:00`"
                .into()
        }
        // `interval_out` under `IntervalStyle = postgres` (I4), which is the
        // only style a `pg_dump` connection writes; the bounds are I62's.
        K::Interval { .. } | K::IntervalFields { .. } => {
            "the way `interval` prints it — `1 year 2 mons 3 days`, `-01:00:00`, `00:00:00` for \
             zero — each unit once, its minutes under `60` and seconds at most `60`, or \
             `infinity`/`-infinity`"
                .into()
        }
        // `network_in`'s grammar (I67), which reads more than this says.
        K::Network { cidr: false } => {
            "as an IPv4 or IPv6 address, optionally followed by `/bits`, an IPv4 one omitting no \
             octet but those past the netmask's whole bytes"
                .into()
        }
        K::Network { cidr: true } => {
            "as an IPv4 or IPv6 network, optionally followed by `/bits`, with no bit set below \
             the netmask, an IPv4 one omitting any trailing octets, which are zero"
                .into()
        }
        K::MacAddr { octets: 8 } => "as eight colon-separated hex pairs".into(),
        K::MacAddr { .. } => "as six colon-separated hex pairs".into(),
        K::Uuid => {
            "as 32 hex digits, grouped `8-4-4-4-12`, a hyphen allowed only after a group of four \
             but the last, the whole optionally in braces"
                .into()
        }
        K::Bytea => {
            "as `\\x` followed by hex pairs, or as `bytea_output = escape` writes it".into()
        }
        K::Jsonb => "as a JSON document".into(),
        K::Text { .. } | K::PaddedText { .. } => "as any text".into(),
    }
}

/// How many of an enum's labels [`accepted_form`] names before it stops
/// counting them out.
///
/// A count cap rather than a length cap (pgdt-model's `SPAN_STORED_TEXT_MAX_BYTES` is the other
/// shape): every label a message prints is printed whole, where a length cap
/// would hand the user a spelling that is not a label. Nothing bounds how
/// many labels a type declares.
///
/// The overflow clause has somewhere to send the reader: `pgdt info --detail`
/// prints every label of an enum column *and* lists every user-defined type
/// with its labels, both uncapped (`docs/design/decisions.md`, "The CLI").
const ENUM_LABELS_SHOWN: usize = 12;

/// The enum clause: the declared labels themselves, which are the whole of
/// what a refused enum literal is missing.
///
/// Each label is single-quoted with any interior quote doubled — the spelling
/// the dump's own `CREATE TYPE … AS ENUM (…)` writes and the CLI's `dequote`
/// accepts — so a printed label pastes straight back into
/// `--filter "col=<label>"`. The CLI's own `label_list` renders the same way
/// and is not shared with it, sitting a layer above.
fn enum_accepted_form(labels: &[String]) -> String {
    let shown = labels.len().min(ENUM_LABELS_SHOWN);
    let list = labels[..shown]
        .iter()
        .map(|label| format!("'{}'", label.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ");
    match labels.len() - shown {
        0 => format!("as one of the type's declared labels: {list}"),
        more => format!(
            "as one of the type's declared labels: {list}, and {more} more; see `info --detail`"
        ),
    }
}

/// A scale-free `numeric` literal's shape: `numeric_in`'s storage bounds
/// (I63), stated as the digits either side of the point.
const NUMERIC_DIGITS: &str =
    "as a number of at most 131072 digits before the point and 16383 after it";

/// The `numeric(p,s)` clause, which is about the **scale** because that is
/// what the refusal is about: `decode::decimal_unscaled_digits` drops a
/// trailing digit only when it is zero, so `--filter 'price>1.005'` on a
/// `numeric(10,2)` is refused.
///
/// Precision says nothing here, binding a field alone: a literal wider than
/// the column can hold still compares against every value in it, as the
/// server coerces it with no typmod (I63). A negative scale is legal from
/// PostgreSQL 15 and means the column stores multiples of a power of ten,
/// which the literal's reader enforces by refusing to drop a non-zero digit
/// off the integer part.
fn decimal_accepted_form(scale: i8) -> String {
    match scale {
        0 => "as a whole number, or `NaN`".into(),
        s if s > 0 => {
            let plural = if s == 1 { "" } else { "s" };
            format!("as a number with at most {s} decimal place{plural}, or `NaN`")
        }
        s => {
            let step = format!("1{}", "0".repeat(usize::from(s.unsigned_abs())));
            format!("as a whole number that is a multiple of {step}, or `NaN`")
        }
    }
}

#[cfg(test)]
mod tests;
