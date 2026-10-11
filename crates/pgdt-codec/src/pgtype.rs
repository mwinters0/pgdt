//! A declared PostgreSQL type string to an Arrow `DataType`
//! (`docs/design/decisions.md`, "Type resolution and decoders"): the built-in
//! table, user types through the database's `CREATE TYPE`/`DOMAIN` list,
//! domains, and the canonical extension names. How a column of the type
//! compares is [`crate::compare`]'s, the built-in table answering both
//! (`docs/design/decisions.md`, "D40").
//!
//! Pure, synchronous, no I/O (`docs/design/decisions.md`, "D74"). A declared
//! type is resolved against a single database's [`TypeDef`] list (already
//! selected by the caller — [`crate::resolve_columns`] is the one that picks
//! which database), never against files or offsets.

use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields};
// The canonical extension types live in `arrow-schema`, which `arrow`'s own
// `datatypes` re-export does not cover — see `CanonicalExtension`.
use arrow_schema::extension::{Json, Uuid};

use pgdt_core::internal::canonical_type_name;
use pgdt_model::{CollationDef, TypeDef, TypeKind};

use crate::compare::{
    CompareKind, ComparisonDivergence, ComparisonPlan, Discrete, NestedCompare, TypeCollation,
    collated_text,
};

/// The outcome of mapping one declared type string, mirroring
/// [`crate::resolve::ColumnResolution`] but at single-type granularity.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeOutcome {
    /// The dump alone determines the value; this is the Arrow type it maps
    /// to (`Utf8View` included — `text` and `json` are mapped there, not
    /// merely defaulted), paired with the [`NestedPlan`] that says which
    /// literal form fills it. This module produces a declared type's pair;
    /// `resolve` adds only the `Utf8View` fallback and a census's further
    /// `List` levels, building each pair whole (`docs/design/decisions.md`,
    /// "D43").
    Mapped(DataType, NestedPlan),
    /// A declared type string this build has no mapping for at all — neither
    /// a built-in, nor found in the database's `CREATE TYPE`/`DOMAIN` list,
    /// nor a composite whose declared field list survived parsing
    /// ([`TypeKind::Composite`] is all-or-nothing) — or a domain that bottoms
    /// out at one of those.
    Unknown,
    /// An array whose element type is opaque by construction — `box`, a
    /// C-level base type, or a shell type, through any chain of domains.
    ///
    /// Refused rather than mapped to `List<Utf8View>`: the array separator is
    /// the *element type's* `typdelim` (I22) and `box`'s is `;`
    /// (`docs/design/decisions.md`, "D41"). Held apart from
    /// [`Self::OpaqueBaseType`] so `pgdt info` can say which happened.
    OpaqueElementType,
    /// An array whose element type is *itself* an array, through any chain of
    /// domains — `CREATE DOMAIN d AS integer[]` and a column of `d[]`, the
    /// only DDL shape there is for it (I26). `integer[][]` is **not** one: it
    /// is a spelling of `integer[]`, whose element is `integer` (I28), and it
    /// resolves like any other array.
    ///
    /// Refused for the same reason as [`Self::OpaqueElementType`]
    /// (`docs/design/decisions.md`, "D41"): the element's own literal grammar
    /// is not what the outer split assumes. The value is written **one brace
    /// deep** — `{"{1,2}","{3}"}`, since `array_out` force-quotes any element
    /// whose text contains `{` (I25) — so the literal's leading brace run and
    /// the column's resolved `List` depth are independent for this shape
    /// alone, and `NestedPlan::Array(Array(…))` would have to mean both *one
    /// literal, two dimensions* and *one literal whose elements are
    /// literals*. Held apart from `OpaqueElementType` because the label would
    /// lie: `integer[]` is not opaque, it is understood and declined.
    ///
    /// Deficiency register: `deficiency: KD3` — this and the census's
    /// [`crate::resolve::ColumnResolution::VaryingArrayShape`] leave the
    /// column `Utf8View` with no way for a caller to ask for more, though
    /// both shapes are fully understood. **(c) unowned.** One lossless
    /// representation closes both — a shape-general
    /// `Struct{dims, lbounds, elements}` — and it only ever touches columns
    /// these two refusals already leave as text, which is what makes it
    /// additive.
    NestedArrayElement,
    /// A C-level base type or a shell/undefined type — genuinely
    /// information-free, not merely unimplemented.
    OpaqueBaseType,
    /// A `CREATE TYPE ... AS ENUM ()` with no labels at all — or an inexact
    /// enum no label of which was read, which only `info` tells apart.
    EmptyEnum,
}

/// Which PostgreSQL literal form fills a resolved Arrow type, at every
/// position in it. The Arrow type alone cannot say
/// (`docs/design/decisions.md`, "D43"); it is a tree because the answer
/// differs per nesting level.
///
/// A `Scalar`, `Decimal` or `Text` leaf is anything [`crate::decode`] handles
/// (`Utf8View` included), which is where every branch bottoms out but at
/// `Int2Vector` and a composite of no fields, childless terminals of their
/// own; [`Self::is_scalar`] asks for either. `Serialize` so
/// `pgdt info --json` can export a resolved schema's plans structurally
/// (`docs/design/decisions.md`, "D67"); **not `Deserialize`, and never
/// persisted** — the cache holds what the dump said, never what we concluded
/// (`docs/design/decisions.md`, "D68").
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub enum NestedPlan {
    /// Filled by `crate::decode`'s per-type decoders, or held as text.
    #[default]
    Scalar,
    /// A `numeric(p,s)` in an Arrow decimal, filled as `Scalar` is and put
    /// through `apply_typmod` as `COPY` puts it (I51), whose bound is
    /// `precision` digits: `Decimal(max(p, s), s)` carries the scale and, where
    /// the scale exceeds the precision, not the precision
    /// ([`crate::decode::typmod_unscaled_digits`]).
    Decimal { precision: u16 },
    /// A `character varying(n)` or `character(n)` held as its text, filled
    /// as `Scalar` is and refused past `length` characters but for trailing
    /// blanks, as `COPY` refuses it (I75,
    /// [`crate::decode::char_typmod_refuses`]).
    Text { length: u32 },
    /// `array_out` → `List<child>`. Nested `Array`s are the multi-dimensional
    /// case and **only** that: the plan's depth is the dimensionality the
    /// column was resolved at, and a value that disagrees is a decode
    /// failure. An array whose element type is an array (I26) is refused at
    /// resolution as [`TypeOutcome::NestedArrayElement`], so a nested `Array`
    /// only ever comes from the shape census, whose outer levels hold
    /// sub-arrays and never a NULL.
    Array(Box<Position<NestedPlan>>),
    /// `record_out` → `Struct<…>`, one position per declared field, in
    /// declaration order.
    Record(Vec<Position<NestedPlan>>),
    /// `range_out` → the five-field range struct. The plan is the *bound*
    /// type's, shared by `lower` and `upper`; the three flags are always
    /// `Boolean`.
    Range(Box<NestedPlan>),
    /// `multirange_out` → `List<` the range struct `>`. The plan is again the
    /// bound type's.
    Multirange(Box<NestedPlan>),
    /// `int2vectorout` → `List<Int16>`. No child plan, because it can have
    /// none: `int2vector`'s element type is `smallint` in the catalog and
    /// nothing about a column can vary it (I47). The Arrow type is the one a
    /// `smallint[]` column gets; the literal is a different grammar.
    Int2Vector,
}

impl NestedPlan {
    /// Whether this position is a leaf a decoder fills, rather than a
    /// container: [`Self::Scalar`], [`Self::Decimal`] or [`Self::Text`].
    pub fn is_scalar(&self) -> bool {
        matches!(self, Self::Scalar | Self::Decimal { .. } | Self::Text { .. })
    }

    /// An array of `element`, a NULL element of which is read.
    pub fn array(element: NestedPlan) -> Self {
        Self::Array(Box::new(Position::nullable(element)))
    }

    /// A composite of `fields`, in declaration order, a NULL field of which
    /// is read.
    pub fn record(fields: impl IntoIterator<Item = NestedPlan>) -> Self {
        Self::Record(fields.into_iter().map(Position::nullable).collect())
    }
}

/// **A position a container holds a value at, which a NULL may sit at** — an
/// array's element or a composite's field — and whether its type refuses
/// one there: a `NOT NULL` domain, or a domain over one
/// ([`domain_not_null`]), whose `domain_in` `array_in` and `record_in` hand
/// a NULL element or field (I76). A range's bound is no such position:
/// `range_in` hands an infinite bound to no input function, so its plan is
/// bare. `plan` is a [`NestedPlan`] or a [`NestedCompare`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Position<T> {
    pub plan: T,
    /// Whether a NULL here is refused, but under
    /// [`pgdt_model::PostgresInvalidValues::Ignore`].
    pub not_null: bool,
}

impl<T> Position<T> {
    /// A position a NULL may sit at.
    pub fn nullable(plan: T) -> Self {
        Self { plan, not_null: false }
    }

    /// The position of a value declared `declared`, refusing a NULL where
    /// that is a `NOT NULL` domain.
    pub(crate) fn of(plan: T, declared: &str, types: &[TypeDef]) -> Self {
        Self { plan, not_null: domain_not_null(declared, types) }
    }
}

/// A `numeric(p,s)`'s typmod, in `numerictypmodin`'s ranges: a precision of 1
/// to 1000 and, from PostgreSQL 15, a scale of -1000 to 1000 (I51).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NumericTypmod {
    pub precision: u16,
    pub scale: i16,
}

/// An `interval` column's field qualifier, one of the thirteen `gram.y`'s
/// `opt_interval` writes and `intervaltypmodin` admits. It is part of the
/// column's typmod, which `interval_in` hands `DecodeInterval`: the unit a
/// number written with none takes, and whether `a:b` is minutes and seconds
/// (I84). A column with none reads its fields under the full range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntervalQualifier {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    YearToMonth,
    DayToHour,
    DayToMinute,
    DayToSecond,
    HourToMinute,
    HourToSecond,
    MinuteToSecond,
}

impl IntervalQualifier {
    /// Every qualifier, in `opt_interval`'s order.
    pub const ALL: [Self; 13] = [
        Self::Year,
        Self::Month,
        Self::Day,
        Self::Hour,
        Self::Minute,
        Self::Second,
        Self::YearToMonth,
        Self::DayToHour,
        Self::DayToMinute,
        Self::DayToSecond,
        Self::HourToMinute,
        Self::HourToSecond,
        Self::MinuteToSecond,
    ];

    /// The qualifier `words` spell — folded to lower case, single-spaced, and
    /// without `second`'s precision — as `opt_interval` reads them.
    fn parse(words: &str) -> Option<Self> {
        Some(match words {
            "year" => Self::Year,
            "month" => Self::Month,
            "day" => Self::Day,
            "hour" => Self::Hour,
            "minute" => Self::Minute,
            "second" => Self::Second,
            "year to month" => Self::YearToMonth,
            "day to hour" => Self::DayToHour,
            "day to minute" => Self::DayToMinute,
            "day to second" => Self::DayToSecond,
            "hour to minute" => Self::HourToMinute,
            "hour to second" => Self::HourToSecond,
            "minute to second" => Self::MinuteToSecond,
            _ => return None,
        })
    }

    /// `INTERVAL_RANGE(typmod)`: the `INTERVAL_MASK` bits of each field the
    /// qualifier spans, `MONTH`, `YEAR` and `DAY` being bits 1 to 3 and
    /// `HOUR`, `MINUTE` and `SECOND` 10 to 12 (`datetime.h`).
    pub(crate) fn range(self) -> i32 {
        let (year, month, day, hour, minute, second) =
            (1 << 2, 1 << 1, 1 << 3, 1 << 10, 1 << 11, 1 << 12);
        match self {
            Self::Year => year,
            Self::Month => month,
            Self::Day => day,
            Self::Hour => hour,
            Self::Minute => minute,
            Self::Second => second,
            Self::YearToMonth => year | month,
            Self::DayToHour => day | hour,
            Self::DayToMinute => day | hour | minute,
            Self::DayToSecond => day | hour | minute | second,
            Self::HourToMinute => hour | minute,
            Self::HourToSecond => hour | minute | second,
            Self::MinuteToSecond => minute | second,
        }
    }
}

/// The field names of the range struct, in order. Reserved: a composite type
/// resolves to a `Struct` too, and only the [`NestedPlan`] tells them apart —
/// these names are for a human reading `pgdt info`, never for dispatch.
pub const RANGE_STRUCT_FIELDS: [&str; 5] =
    ["lower", "upper", "lower_inclusive", "upper_inclusive", "empty"];

/// Split `declared` into its base type name and typmod contents, if any
/// (`numeric(38,10)` -> `("numeric", Some("38,10"))`), where the typmod ends
/// the string. It says whether a name is schema-qualified; a built-in's is read
/// by [`builtin_name`]. Only `numeric` and `float` read the typmod's *value*
/// for their Arrow type, every other mapping below being
/// `Microsecond`-precision or otherwise typmod-independent; a `character`'s
/// and a `bit`'s length and an `interval`'s qualifier are read for its
/// comparison and grammar. A `(` inside a quoted name is the name's (I29).
pub(crate) fn split_typmod(s: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    let open = s.bytes().position(|b| {
        quoted ^= b == b'"';
        b == b'(' && !quoted
    });
    match open {
        Some(i) if s.ends_with(')') => (s[..i].trim_end(), Some(&s[i + 1..s.len() - 1])),
        _ => (s, None),
    }
}

/// A built-in's declared spelling — no `.` in it (I8) — as the one name
/// [`builtin_scalar`] and [`builtin_range_subtype`] match on, its typmod and
/// an `interval`'s field qualifier, read the way PostgreSQL's grammar reads
/// them (`docs/design/roadmap.md`, "The input contract is valid PostgreSQL").
///
/// - **A typmod may sit mid-name**: `format_type` writes
///   `timestamp(3) with time zone`, `time(2) without time zone` and
///   `interval day to second(3)`, so the one parenthesized group is cut out
///   wherever it falls.
/// - **`interval`'s field qualifier is the rest of its typmod**, so
///   `interval year to month` is `interval`, its qualifier
///   [`IntervalQualifier::YearToMonth`]; only `opt_interval`'s thirteen
///   spellings are one. Quoted, the typmod's first integer is the qualifier's
///   `INTERVAL_MASK` bits, as `intervaltypmodin` reads `"interval"(4)`.
/// - **An unquoted name is folded to lower case and its whitespace to one
///   space**, then read through the grammar's own keywords (`int`, `char`,
///   `varchar`, `dec`, `float(p)`, bare `timestamp`, …) and then as a catalog
///   name (`int4`, `bpchar`, `timestamptz`, …). `format_type` writes one of the
///   latter itself: `bpchar`, for a `character` column with no typmod.
/// - **A quoted name is a catalog name alone**, case and all, as the grammar
///   takes it: `"char"` is the one-byte internal type, not `character`, and
///   reaches no arm; nor does `"integer"` or any spelling only the grammar
///   knows, which names a type `pg_catalog` does not hold and so answers
///   quoted.
pub(crate) fn builtin_name(
    declared: &str,
) -> (std::borrow::Cow<'static, str>, Option<&str>, Option<IntervalQualifier>) {
    let (words, typmod) = match (declared.find('('), declared.rfind(')')) {
        (Some(open), Some(close)) if open < close => (
            format!("{} {}", &declared[..open], &declared[close + 1..]),
            Some(&declared[open + 1..close]),
        ),
        _ => (declared.to_owned(), None),
    };
    let words = words.trim();
    if let Some(quoted) = words.strip_prefix('"').and_then(|w| w.strip_suffix('"')) {
        let name = match catalog_name(quoted) {
            Some(sql) => sql.to_owned(),
            None if CATALOG_NAMES.iter().any(|&(_, sql)| sql == quoted) => words.to_owned(),
            None => quoted.to_owned(),
        };
        let qualifier = (name == "interval").then_some(typmod).flatten().and_then(|typmod| {
            let range = typmod.split(',').next()?.trim().parse::<i32>().ok()?;
            IntervalQualifier::ALL.into_iter().find(|q| q.range() == range)
        });
        return (name.into(), typmod, qualifier);
    }
    let words = words.split_ascii_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    let keyword = match words.as_str() {
        "int" => "integer",
        "dec" | "decimal" => "numeric",
        // `float(p)` is `real` through 24 bits of precision and `double
        // precision` past them; the precision is spent choosing, and the type
        // chosen takes no typmod.
        "float" => {
            let real = typmod.and_then(|p| p.trim().parse::<u8>().ok()).is_some_and(|p| p <= 24);
            return ((if real { "real" } else { "double precision" }).into(), None, None);
        }
        // The SQL word with no length is `character(1)`, as the grammar
        // gives it one; `bpchar`, the catalog name, is the type with none,
        // which is how `format_type` writes such a column (I8).
        "character" | "char" | "nchar" | "national char" | "national character" => {
            return ("character".into(), typmod.or(Some("1")), None);
        }
        "char varying"
        | "nchar varying"
        | "national char varying"
        | "national character varying" => "character varying",
        "timestamp" => "timestamp without time zone",
        "time" => "time without time zone",
        w => {
            let qualifier = w.strip_prefix("interval ").and_then(IntervalQualifier::parse);
            if qualifier.is_some() {
                return ("interval".into(), typmod, qualifier);
            }
            let name = catalog_name(w).map_or_else(|| words.clone(), str::to_owned);
            return (name.into(), typmod, None);
        }
    };
    (keyword.into(), typmod, None)
}

/// Each catalog type name whose SQL spelling is another, beside that
/// spelling. The right-hand names are the grammar's alone: `pg_catalog` holds
/// no type called `integer`.
const CATALOG_NAMES: [(&str, &str); 13] = [
    ("int2", "smallint"),
    ("int4", "integer"),
    ("int8", "bigint"),
    ("float4", "real"),
    ("float8", "double precision"),
    ("bool", "boolean"),
    ("bpchar", "character"),
    ("varchar", "character varying"),
    ("timestamp", "timestamp without time zone"),
    ("timestamptz", "timestamp with time zone"),
    ("time", "time without time zone"),
    ("timetz", "time with time zone"),
    ("varbit", "bit varying"),
];

/// A catalog type name whose SQL spelling is another, as that spelling —
/// `None` where the two agree or the name is not a built-in's.
fn catalog_name(name: &str) -> Option<&'static str> {
    CATALOG_NAMES.iter().find(|&&(catalog, _)| catalog == name).map(|&(_, sql)| sql)
}

/// `numeric(p,s)` -> `Decimal128`/`Decimal256` of precision `max(p, s)` at
/// scale `s` when that fits; bare `numeric`, and a typmod no Arrow decimal
/// carries — a precision or a positive scale past `Decimal256`'s 76 digits,
/// or a scale below `i8`'s -128 — stays `Utf8View`, same as arbitrary
/// precision (I4: `NaN` is reachable through any numeric column regardless,
/// and is a decode-time concern, not a mapping one).
///
/// **The precision widens to the scale** because PostgreSQL 15 and later
/// admit a scale from -1000 to 1000 whatever the precision
/// (`numerictypmodin`, I51), and Arrow refuses a positive scale past its
/// precision: `numeric(2,5)` holds values below `0.001` written with five
/// fractional digits (`0.00012`), at most `p` of them significant, so
/// `Decimal128(5, 5)` holds every one exactly and stays typed. A negative
/// scale is below any precision and needs no widening. Rejected: text for
/// every scale past its precision, which gives up an exact typed column Arrow
/// can carry.
///
/// The `Utf8View` arms are still *ordered*: [`CompareKind::Numeric`]
/// normalizes the text the file holds and compares by value, which is
/// `cmp_var_common`'s order and insensitive to trailing zeros (I33). The
/// typed arms carry the column's own scale instead, so both sides of one are
/// unscaled integers.
///
/// **Only the bare form admits an infinity**: `apply_typmod_special` rejects
/// `±Infinity` under any typmod (I34), so accepting the spelling in a
/// *filter's literal* on a typmod'd text arm would accept a value the server
/// refuses.
fn map_numeric(typmod: Option<&str>) -> (DataType, ComparisonPlan) {
    let text =
        |infinities, typmod| ComparisonPlan::agrees(CompareKind::Numeric { infinities, typmod });
    let Some(typmod) = typmod else { return (DataType::Utf8View, text(true, None)) };
    let mut parts = typmod.split(',').map(str::trim);
    let precision: Option<u16> = parts.next().and_then(|p| p.parse().ok());
    let scale: Option<i16> = parts.next().map_or(Some(0), |s| s.parse().ok());
    let (Some(precision), Some(scale)) = (precision, scale) else {
        return (DataType::Utf8View, text(false, None));
    };
    let held_as_text = (DataType::Utf8View, text(false, Some(NumericTypmod { precision, scale })));
    let Ok(scale) = i8::try_from(scale) else { return held_as_text };
    let decimal = ComparisonPlan::agrees(CompareKind::Decimal { precision, scale });
    match u8::try_from(precision.max(u16::from(scale.max(0).unsigned_abs()))) {
        Ok(p @ 1..=38) => (DataType::Decimal128(p, scale), decimal),
        Ok(p @ 39..=76) => (DataType::Decimal256(p, scale), decimal),
        _ => held_as_text,
    }
}

/// A `character varying(n)`'s or `character(n)`'s length, from its typmod:
/// `None` where it states none, and where it is zero or no integer, DDL no
/// server holds.
fn char_length(typmod: Option<&str>) -> Option<u32> {
    typmod?.trim().parse::<u32>().ok().filter(|&n| n > 0)
}

/// **The built-in scalar table**: every declared type with no `.` in its name
/// (I8) that is not a range. `None` means the base name isn't a built-in this
/// build recognises (e.g. `money`).
///
/// **Each arm answers both questions at once**: which Arrow type the column
/// gets, and how two of its values compare. That pairing is the comparison
/// register's exhaustiveness check — a type added here without a comparison
/// does not compile — and it is why the register is keyed on the declared
/// type rather than on the Arrow type many unrelated declared types share
/// (`docs/design/decisions.md`, "D40").
///
/// `collation` is the column's own `COLLATE` clause, verbatim, and
/// `collations` is what the dump's own `CREATE COLLATION` statements said
/// about it; only the four collatable arms read either, and [`map_builtin`] —
/// whose Arrow type depends on neither — passes `None` and an empty list.
/// `qualifier` is an `interval`'s field qualifier ([`builtin_name`]), which
/// only that arm reads and `map_builtin` passes as `None` too.
///
/// Deficiency register: `deficiency: KD13` — `money`'s absent arm is the one
/// place this table falls below the ADBC floor: the driver answers `int64`
/// and the column resolves `Utf8View`, because `cash_out` renders through the
/// monetary locale and `pg_dump` sets `lc_monetary` nowhere, so the file
/// cannot say which locale wrote a value. **(a) deliberate tradeoff** — an
/// arm here would have to guess a locale or ask for one, which the bar
/// refuses for every other type; the column still filters as text.
pub(crate) fn builtin_scalar(
    base: &str,
    typmod: Option<&str>,
    qualifier: Option<IntervalQualifier>,
    collation: Option<&str>,
    collations: &[CollationDef],
) -> Option<(DataType, ComparisonPlan)> {
    use CompareKind as K;
    use DataType::*;
    use arrow_schema::IntervalUnit::MonthDayNano;
    use arrow_schema::TimeUnit::Microsecond;
    let agrees = ComparisonPlan::agrees;
    let text = ComparisonPlan::AS_TEXT;
    Some(match base {
        "smallint" => (Int16, agrees(K::Int { bytes: 2 })),
        "integer" => (Int32, agrees(K::Int { bytes: 4 })),
        "bigint" => (Int64, agrees(K::Int { bytes: 8 })),
        // `oidout` is `snprintf("%u")`, so the file holds an unsigned 32-bit
        // integer and `UInt32` is what it says (I39). The ADBC driver maps
        // `oid` to `Int32`, which misreads every OID at or above 2^31
        // (`docs/design/decisions.md`, "D38").
        //
        // `UnsignedInt`, not `Int`: the difference shows only on a filter's
        // literal, where `oidin` accepts a leading minus and wraps (I39) and
        // this refuses with `Error::PredicateValueDecode`. No *field* is
        // affected — no dump ever writes a signed OID.
        "oid" => (UInt32, agrees(K::UnsignedInt)),
        // `false < true`, PostgreSQL's own boolean order.
        "boolean" => (Boolean, agrees(K::Bool)),
        // IEEE's three specials are representable and `pg_float_cmp` orders
        // them PostgreSQL's way, so nothing here is carried as a position.
        "real" => (Float32, agrees(K::Float32)),
        "double precision" => (Float64, agrees(K::Float64)),
        "numeric" => map_numeric(typmod),
        // The three collatable arms. `text`/`varchar` default to the
        // database's collation and `name` to `C` (I37), which is why a bare
        // `name` column agrees and a bare `text` column cannot be said to.
        // A `varchar(n)` carries its length, which its field is held to (I75).
        "text" => (
            Utf8View,
            collated_text(K::Text { length: None }, collation, TypeCollation::Database, collations),
        ),
        "character varying" => (
            Utf8View,
            collated_text(
                K::Text { length: char_length(typmod) },
                collation,
                TypeCollation::Database,
                collations,
            ),
        ),
        "name" => (
            Utf8View,
            collated_text(K::Text { length: None }, collation, TypeCollation::Bytewise, collations),
        ),
        // The fourth collatable arm, with a comparison of its own: the dump
        // writes every `character(n)` value blank-padded to `n` and
        // `bpcharcmp` calls `bcTruelen` on both sides before consulting a
        // collation (I38). `K::PaddedText` strips the padding; what is left
        // is the `text` question.
        "character" => (
            Utf8View,
            collated_text(
                K::PaddedText { length: char_length(typmod) },
                collation,
                TypeCollation::Database,
                collations,
            ),
        ),
        // `infinity`/`-infinity` are ordered rather than refused, as
        // positions rather than numbers (`special_order_key`, I34).
        "date" => (Date32, agrees(K::Date)),
        "timestamp without time zone" => {
            (Timestamp(Microsecond, None), agrees(K::Timestamp { with_tz: false }))
        }
        "timestamp with time zone" => {
            (Timestamp(Microsecond, Some("UTC".into())), agrees(K::Timestamp { with_tz: true }))
        }
        "time without time zone" => (Time64(Microsecond), agrees(K::Time)),
        // Held as text in Arrow and *ordered* all the same, by the comparison
        // its own type defines (I40): the UTC instant, then a tie broken on
        // the stored zone.
        "time with time zone" => (Utf8View, agrees(K::TimeTz)),
        // Arrow's `Interval(MonthDayNano)` carries months, days and a time
        // part as three independent fields, exactly as PostgreSQL's
        // `Interval` does — the mapping is the struct, not a reading of it.
        // The *comparison* is `interval_cmp_value`'s fused span (I40), and
        // the field qualifier what its field is read under (I84).
        "interval" => (Interval(MonthDayNano), agrees(K::Interval { qualifier })),
        // `uuid_internal_cmp` is `memcmp` over 16 bytes, and `byteacmp` is
        // `memcmp` then length — both are `[u8]`'s own order (I33).
        "uuid" => (FixedSizeBinary(16), agrees(K::Uuid)),
        "bytea" => (Binary, agrees(K::Bytea)),
        // PostgreSQL defines *no* comparison for `json` — no `=`, no order,
        // no operator class — so bytewise offers more than the server does
        // rather than less. It is the register's one text-held row.
        "json" => (Utf8View, text),
        // `jsonb` has a full order and this implements it structurally (I41).
        // The string leaves go through the database's own collation, which
        // the file does not carry (I32), so the plan agrees about the shape
        // and announces the residue.
        "jsonb" => (
            Utf8View,
            ComparisonPlan::diverging(K::Jsonb, ComparisonDivergence::JsonbStringCollation),
        ),
        // `network_cmp_internal`'s order for the two address types, and the
        // plain byte order for the two MAC types (I40). `cidr` differs from
        // `inet` only in refusing a literal with a bit set below its netmask.
        "inet" => (Utf8View, agrees(K::Network { cidr: false })),
        "cidr" => (Utf8View, agrees(K::Network { cidr: true })),
        "macaddr" => (Utf8View, agrees(K::MacAddr { octets: 6 })),
        "macaddr8" => (Utf8View, agrees(K::MacAddr { octets: 8 })),
        // The one built-in that maps to a container without being spelled
        // like one. `int2vectorout` writes space-separated `int16` with no
        // quoting, no escaping and no possible NULL element (I47), so the
        // value space is exactly `List<Int16>`'s; the comparison is
        // `array_cmp`'s, via `anyarray` polymorphism.
        "int2vector" => (list_of(Int16), ComparisonPlan::Nested(NestedCompare::Int2Vector)),
        _ => return None,
    })
}

/// A canonical Arrow extension type one of our columns claims — the *name*
/// half of the mapping, which the Arrow type alone cannot carry
/// (`docs/design/decisions.md`, "D37").
///
/// Two exist for us, the two whose storage type is already what we emit:
/// `arrow.uuid` over `FixedSizeBinary(16)` and `arrow.json` over `Utf8View`.
/// Neither changes a column's Arrow type or a byte of its data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalExtension {
    /// `arrow.uuid`, over `FixedSizeBinary(16)`.
    Uuid,
    /// `arrow.json`, over `Utf8` / `LargeUtf8` / `Utf8View`.
    Json,
}

impl CanonicalExtension {
    /// Stamp `field` with this extension's `ARROW:extension:*` metadata.
    ///
    /// arrow-rs's own types do the writing and its `supports_data_type` the
    /// checking: `arrow.json`'s metadata key must be *present and empty*, and
    /// a reader calling `Field::try_canonical_extension_type` rejects the
    /// field when it is absent.
    ///
    /// The `expect` is by construction: [`extension_for`] answers `Some` only
    /// where it walked to the same base name [`builtin_scalar`] maps to that
    /// extension's storage type. `the_extension_names_fit_their_storage_type`
    /// keeps the two in step.
    fn apply(self, mut field: Field) -> Field {
        let stamped = match self {
            Self::Uuid => field.try_with_extension_type(Uuid),
            Self::Json => field.try_with_extension_type(Json::default()),
        };
        stamped.expect("an extension is only ever named for a type mapped to its storage type");
        field
    }
}

/// The canonical Arrow extension a **column** of `declared` carries, if any.
///
/// The walk is [`comparison_for`]'s: an array first — an extension names the
/// element type, not the column's — then a `.`-qualified user type, where only
/// a domain can bottom out at a built-in, then the built-in name itself.
///
/// **Only a top-level column's field is stamped**
/// (`docs/design/decisions.md`, "D37"): a `uuid` inside a composite or an
/// array element keeps its `FixedSizeBinary(16)` with no name on it. Nothing
/// here is load-bearing for decoding — the metadata is a claim about the
/// bytes, never an input to producing them.
///
/// [`comparison_for`]: crate::compare::comparison_for
pub fn extension_for(declared: &str, types: &[TypeDef]) -> Option<CanonicalExtension> {
    let mut declared = declared.trim();
    // Bounded as `domain_terminal` is: a chain visits each `CREATE DOMAIN` at
    // most once, so a walk still going past the list's length is in a cycle.
    for _ in 0..=types.len() {
        if array_element(declared).is_some() {
            return None;
        }
        let (base, _) = split_typmod(declared);
        if !base.contains('.') {
            return match &*builtin_name(declared).0 {
                "uuid" => Some(CanonicalExtension::Uuid),
                "json" | "jsonb" => Some(CanonicalExtension::Json),
                _ => None,
            };
        }
        // A domain is the only user-defined kind that can reach a built-in;
        // an enum, composite, range or opaque base type maps to a type no
        // canonical extension names.
        let TypeKind::Domain { base_type, .. } = &find_type(base, types)?.kind else {
            return None;
        };
        declared = base_type.trim();
    }
    None
}

/// [`extension_for`], applied — the one call site's whole job, kept here so
/// that `apply` need not be public.
pub(crate) fn with_extension(field: Field, declared: &str, types: &[TypeDef]) -> Field {
    match extension_for(declared, types) {
        Some(extension) => extension.apply(field),
        None => field,
    }
}

/// The built-in half of the mapping: [`builtin_scalar`], plus PostgreSQL's
/// twelve built-in range and multirange types. Unlike a user-defined range
/// (`CREATE TYPE ... AS RANGE`), those never appear schema-qualified and have
/// no `CREATE TYPE` anywhere in the file (I10), so they need bare-name
/// recognition here or they fall through to `Unknown`.
fn map_builtin(
    base: &str,
    typmod: Option<&str>,
    types: &[TypeDef],
    visits: Visits,
) -> Option<TypeOutcome> {
    // No collation or qualifier: an Arrow type never depends on either, and
    // the comparison half of the pair is read here only for a decimal's
    // precision and a text's length.
    if let Some((mapped, comparison)) = builtin_scalar(base, typmod, None, None, &[]) {
        // The literal form is this walk's to say: a `smallint[]` column and
        // an `int2vector` one are both `List<Int16>` and are written in
        // different grammars (`docs/design/decisions.md`, "D43"). Exactly one
        // built-in is a container.
        let plan = match (base, comparison) {
            ("int2vector", _) => NestedPlan::Int2Vector,
            (_, ComparisonPlan::Compared { kind: CompareKind::Decimal { precision, .. }, .. }) => {
                NestedPlan::Decimal { precision }
            }
            (
                _,
                ComparisonPlan::Compared {
                    kind:
                        CompareKind::Text { length: Some(length) }
                        | CompareKind::PaddedText { length: Some(length) },
                    ..
                },
            ) => NestedPlan::Text { length },
            _ => NestedPlan::Scalar,
        };
        // The pairing is checked rather than trusted: a built-in mapped to a
        // container type without a plan named above would fill it with a
        // scalar builder and decode every value as text.
        debug_assert_eq!(
            matches!(mapped, DataType::List(_) | DataType::Struct(_)),
            !plan.is_scalar(),
            "`builtin_scalar`'s `{base}` arm and `map_builtin`'s plan table disagree about \
             whether it is a container",
        );
        return Some(TypeOutcome::Mapped(mapped, plan));
    }
    // Built-in ranges, and their PG14+ multirange counterparts (I10): both
    // appear bare, never schema-qualified, so both need this table rather
    // than the user-defined lookup (I8).
    let BuiltinRange { subtype, multi, .. } = builtin_range_subtype(base)?;
    let (bound, bound_plan) = resolve_nested(subtype, types, visits);
    Some(if multi {
        TypeOutcome::Mapped(
            list_of(range_struct(bound)),
            NestedPlan::Multirange(Box::new(bound_plan)),
        )
    } else {
        TypeOutcome::Mapped(range_struct(bound), NestedPlan::Range(Box::new(bound_plan)))
    })
}

/// One of PostgreSQL's twelve built-in range/multirange types, as the two
/// walks over it need it.
pub(crate) struct BuiltinRange {
    /// The bound type, spelled as DDL spells it.
    pub(crate) subtype: &'static str,
    /// Whether the name was the multirange half of the pair.
    pub(crate) multi: bool,
    /// The canonical function values of this type are rewritten through on
    /// the way in, where there is one — the half of I46 the comparison
    /// reproduces. A fact about the *range type*, not about the subtype.
    pub(crate) discrete: Option<Discrete>,
}

/// The definition of one of PostgreSQL's twelve built-in range/multirange
/// types. `None` for anything else.
///
/// **Hardcoded because the catalog holds it and the DDL does not** (I10):
/// `TypeKind::Range::subtype` is populated only for a user-defined range. The
/// six multirange names carry the *same* subtypes as their range counterparts
/// and a different literal form, so they are told apart here.
pub(crate) fn builtin_range_subtype(name: &str) -> Option<BuiltinRange> {
    let (subtype, multi, discrete) = match name {
        "int4range" => ("integer", false, Some(Discrete::Int4)),
        "int8range" => ("bigint", false, Some(Discrete::Int8)),
        "numrange" => ("numeric", false, None),
        "tsrange" => ("timestamp without time zone", false, None),
        "tstzrange" => ("timestamp with time zone", false, None),
        "daterange" => ("date", false, Some(Discrete::Date)),
        "int4multirange" => ("integer", true, Some(Discrete::Int4)),
        "int8multirange" => ("bigint", true, Some(Discrete::Int8)),
        "nummultirange" => ("numeric", true, None),
        "tsmultirange" => ("timestamp without time zone", true, None),
        "tstzmultirange" => ("timestamp with time zone", true, None),
        "datemultirange" => ("date", true, Some(Discrete::Date)),
        _ => return None,
    };
    Some(BuiltinRange { subtype, multi, discrete })
}

/// `List<child>`, with the element field named and nullable the way every
/// nested position is.
fn list_of(child: DataType) -> DataType {
    DataType::List(Arc::new(Field::new("item", child, true)))
}

/// The five-field range struct, in [`RANGE_STRUCT_FIELDS`] order. The two
/// bounds share `bound` by construction; the three flags are always present,
/// so they are the one non-nullable thing this module builds.
fn range_struct(bound: DataType) -> DataType {
    DataType::Struct(Fields::from(vec![
        Field::new(RANGE_STRUCT_FIELDS[0], bound.clone(), true),
        Field::new(RANGE_STRUCT_FIELDS[1], bound, true),
        Field::new(RANGE_STRUCT_FIELDS[2], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[3], DataType::Boolean, false),
        Field::new(RANGE_STRUCT_FIELDS[4], DataType::Boolean, false),
    ]))
}

/// The definition `reference` names — a declared type, a domain's base —
/// found by comparing both sides in their canonical spelling, so a quoted and
/// an unquoted spelling of one name agree and the server's case folding is
/// applied to each (I29). `None` for a reference that is not one type name.
pub(crate) fn find_type<'a>(reference: &str, types: &'a [TypeDef]) -> Option<&'a TypeDef> {
    let key = canonical_type_name(reference)?;
    types.iter().find(|t| canonical_type_name(&t.name).is_some_and(|name| name == key))
}

/// The range whose `multirange_type_name` is `reference`, compared as
/// [`find_type`] compares — the only trace a multirange companion has (I10).
pub(crate) fn companion_range<'a>(reference: &str, types: &'a [TypeDef]) -> Option<&'a TypeDef> {
    let key = canonical_type_name(reference)?;
    types.iter().find(|t| {
        matches!(&t.kind, TypeKind::Range { multirange_type_name: Some(n), .. }
            if canonical_type_name(n).is_some_and(|name| name == key))
    })
}

/// How many more `CREATE TYPE`/`CREATE DOMAIN` definitions one path of a
/// recursive type walk may visit — [`resolve_declared_type`]'s and
/// [`comparison_for`]'s, through domains, composites, ranges and array
/// elements alike.
///
/// PostgreSQL's type graph is acyclic (I24), so a path visits each definition
/// at most once and the list's length is always enough. A path that runs out
/// has visited one twice: a cycle a hand-edited file made, or one the
/// name-keyed list holds where the server's did not, a rename the preamble
/// does not follow being enough. That walk answers `Unknown` — and a
/// comparison `Refused` — rather than recursing until the stack is gone
/// (`docs/design/decisions.md`, "D41").
///
/// [`comparison_for`]: crate::compare::comparison_for
#[derive(Debug, Clone, Copy)]
pub(crate) struct Visits(usize);

impl Visits {
    /// The allowance for one walk over `types`.
    pub(crate) fn over(types: &[TypeDef]) -> Self {
        Self(types.len())
    }

    /// One definition visited: what is left, or `None` where nothing was.
    pub(crate) fn spend(self) -> Option<Self> {
        self.0.checked_sub(1).map(Self)
    }
}

/// Resolve a type sitting *inside* a nested one — an array's element, a
/// composite's field, a range's bound.
///
/// Every non-`Mapped` outcome becomes `Utf8View` **in that position**, which
/// is what the same type would have become at top level.
fn resolve_nested(declared: &str, types: &[TypeDef], visits: Visits) -> (DataType, NestedPlan) {
    match resolve_walk(declared, types, visits) {
        TypeOutcome::Mapped(data_type, plan) => (data_type, plan),
        _ => (DataType::Utf8View, NestedPlan::Scalar),
    }
}

/// The user-defined half: look the schema-qualified `name` up in `types`
/// through [`find_type`] and resolve by kind. A domain recurses on its base
/// type — legal to nest (a domain over a domain) — and every visit is spent
/// from `visits`, which bounds a cycle.
fn resolve_user_type(name: &str, types: &[TypeDef], visits: Visits) -> TypeOutcome {
    let Some(visits) = visits.spend() else { return TypeOutcome::Unknown };
    let Some(def) = find_type(name, types) else {
        // Not a type of its own — but it might be a range's auto-created
        // multirange companion, which `pg_dump` emits no `CREATE TYPE` for
        // (I10). Its only trace is the `multirange_type_name` parameter
        // inside the range's own DDL, which is also where the companion's
        // bound type comes from.
        return match companion_range(name, types).map(|t| &t.kind) {
            Some(TypeKind::Range { subtype, .. }) => {
                let (bound, plan) = range_bound(subtype.as_deref(), types, visits);
                TypeOutcome::Mapped(list_of(range_struct(bound)), NestedPlan::Multirange(plan))
            }
            _ => TypeOutcome::Unknown,
        };
    };
    match &def.kind {
        TypeKind::Enum { labels, .. } if labels.is_empty() => TypeOutcome::EmptyEnum,
        TypeKind::Enum { .. } => TypeOutcome::Mapped(
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
            NestedPlan::Scalar,
        ),
        TypeKind::Domain { base_type, .. } => resolve_walk(base_type, types, visits),
        // The field list is all-or-nothing: `None` means the grammar could
        // not read the body, and a `Struct` built from a short list would
        // refuse every valid row (see `TypeKind::Composite`). A zero-field
        // composite is a real type and maps to a zero-field `Struct` (I23).
        TypeKind::Composite { fields: None } => TypeOutcome::Unknown,
        TypeKind::Composite { fields: Some(fields) } => {
            let mut arrow_fields = Vec::with_capacity(fields.len());
            let mut plans = Vec::with_capacity(fields.len());
            for field in fields {
                let (data_type, plan) = resolve_nested(&field.declared_type, types, visits);
                arrow_fields.push(Field::new(&field.name, data_type, true));
                plans.push(Position::of(plan, &field.declared_type, types));
            }
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(arrow_fields)),
                NestedPlan::Record(plans),
            )
        }
        TypeKind::Range { subtype, .. } => {
            let (bound, plan) = range_bound(subtype.as_deref(), types, visits);
            TypeOutcome::Mapped(range_struct(bound), NestedPlan::Range(plan))
        }
        // Both genuinely information-free — one diagnostic bucket for both.
        TypeKind::Base { .. } | TypeKind::Shell => TypeOutcome::OpaqueBaseType,
    }
}

/// A range's bound type, from the `subtype = ...` parameter the DDL carried.
/// A range whose parameter list the grammar could not read keeps its struct
/// shape with `Utf8View` bounds — the bounds are still exactly the text the
/// file holds, which is what every other unmapped position falls back to.
fn range_bound(
    subtype: Option<&str>,
    types: &[TypeDef],
    visits: Visits,
) -> (DataType, Box<NestedPlan>) {
    let (data_type, plan) = match subtype {
        Some(subtype) => resolve_nested(subtype, types, visits),
        None => (DataType::Utf8View, NestedPlan::Scalar),
    };
    (data_type, Box::new(plan))
}

/// The whole decision for an array column, from its already-normalized
/// element type: the two refusals, and otherwise the `List` and the
/// [`NestedPlan::Array`] that fills it.
///
/// **Both refusals test the terminal of one domain walk, not the declared
/// spelling** (I22, I26; `docs/design/decisions.md`, "D41"): a domain records
/// neither the `typdelim` it inherited nor the array-ness of its base.
///
/// The order between them decides a label, never a type — both answer
/// `Utf8View`; opaque is tested first because its delimiter makes even the
/// element boundaries unrecoverable. No input reaches both, so a terminal of
/// `box[]` answers `NestedArrayElement`, true but silent about the delimiter.
/// *Rejected: testing opaqueness recursively through the element's own array
/// levels, which buys a better diagnostic on a shape `pg_dump` cannot write
/// (I21) rather than a better type.*
///
/// `box` is checked by name because it is a built-in with no `CREATE TYPE` of
/// its own; a user-defined base type is refused wholesale, `TypeKind::Base`
/// and `Shell` alike, whatever `DELIMITER` it declares, its elements being
/// text no reader here holds the grammar of. The
/// array-ness test reads the terminal through [`array_element`] rather than
/// looking for a trailing `[]`, because `CREATE DOMAIN d AS integer ARRAY` is
/// as legal as any other spelling (I28).
///
/// See [`TypeOutcome::OpaqueElementType`] and
/// [`TypeOutcome::NestedArrayElement`] for why each shape is refused.
fn resolve_array(element: &str, types: &[TypeDef], visits: Visits) -> TypeOutcome {
    let terminal = domain_terminal(element, types);
    let opaque = is_box(terminal)
        || matches!(
            find_type(terminal, types).map(|t| &t.kind),
            Some(TypeKind::Base { .. } | TypeKind::Shell)
        );
    if opaque {
        return TypeOutcome::OpaqueElementType;
    }
    if array_element(terminal).is_some() {
        return TypeOutcome::NestedArrayElement;
    }
    // The element resolves through `resolve_declared_type`, so nesting
    // composes with no special case: `public.comp[]` is `List<Struct<…>>`.
    let (data_type, plan) = resolve_nested(element, types, visits);
    TypeOutcome::Mapped(
        list_of(data_type),
        NestedPlan::Array(Box::new(Position::of(plan, element, types))),
    )
}

/// The element type of an array declaration, in any of the six spellings
/// PostgreSQL's `Typename` production accepts — or `None` when `declared` is
/// not an array declaration at all.
///
/// **All six are one type, one array level deep** (I28): the parser keeps
/// "array of the element" and discards both the bracket count and the bounds,
/// so `integer[]`, `integer[3]`, `integer[][]`, `integer[3][4]`,
/// `integer ARRAY` and `integer ARRAY[4]` all answer `Some("integer")`.
/// `pg_dump` writes only the first (I21); the rest reach us from hand-written
/// SQL or another producer, which is inside the input contract
/// (`roadmap.md`, "The input contract is valid PostgreSQL").
///
/// The grammar is followed rather than approximated, in both directions.
/// `opt_array_bounds` is left-recursive with no cap, so a bracket run of any
/// length is still one array level even though `MAXDIM` is 6. But the `ARRAY`
/// alternatives take *at most one* bound, so `integer ARRAY[4][5]` is not a
/// declaration and answers `None` — as does text a server would reject
/// outright (`integer[abc]`, `integer[`). Inventing an array type for input
/// PostgreSQL refuses is the same mistake as reading a spelling more
/// literally than PostgreSQL does, pointed the other way.
///
/// **A quoted type name is never misread as an array** (I29). A name may
/// legally contain `[`, `]`, a space or the `ARRAY` keyword, and `pg_dump`
/// writes it quoted wherever it appears — so a column of `s."x ARRAY"` ends in
/// a `"` and both strip helpers bail, while `s."x ARRAY"[]` sheds the bound
/// outside the quotes and answers the name with its quotes intact, which
/// [`find_type`] then finds. The quotes are what keep the production
/// unambiguous, and stripping a suffix without checking for a closing quote
/// first is exactly what would break that.
pub(crate) fn array_element(declared: &str) -> Option<&str> {
    let declared = declared.trim();
    // `SimpleTypename ARRAY '[' Iconst ']'` and `SimpleTypename ARRAY`: at
    // most one bound and only in the `[n]` form, since the production spells
    // out `Iconst`. Tried before the unbounded run below and never after it.
    let head = match strip_bound(declared) {
        Some((head, true)) => head,
        _ => declared,
    };
    if let Some(element) = strip_array_keyword(head) {
        // What precedes the keyword is a bare `SimpleTypename`: a bracket of
        // its own or a second `ARRAY` is a syntax error, not a deeper array.
        return (strip_bound(element).is_none() && strip_array_keyword(element).is_none())
            .then_some(element);
    }
    // `SimpleTypename opt_array_bounds`: one or more `[]`/`[n]` pairs.
    let (mut element, _) = strip_bound(declared)?;
    while let Some((head, _)) = strip_bound(element) {
        element = head;
    }
    // `integer ARRAY[4][5]` arrives here, having shed both bounds: the
    // keyword took a second one, which the grammar has no production for.
    strip_array_keyword(element).is_none().then_some(element)
}

/// Strip one trailing `[]` or `[n]`, answering the head and whether the bound
/// was an `Iconst`: `opt_array_bounds` takes either form, the `ARRAY`
/// alternatives only the second. A bound that is neither empty nor an
/// unsigned integer is not a bound at all, and is left in place.
fn strip_bound(declared: &str) -> Option<(&str, bool)> {
    let inner = declared.trim_end().strip_suffix(']')?;
    let open = inner.rfind('[')?;
    let bound = inner[open + 1..].trim();
    let iconst = !bound.is_empty() && bound.bytes().all(|b| b.is_ascii_digit());
    (iconst || bound.is_empty()).then(|| (inner[..open].trim_end(), iconst))
}

/// Strip a trailing `ARRAY` keyword, case-insensitively — it is a keyword, so
/// `INTEGER ARRAY` and `Integer Array` are the same declaration (I28) — and
/// only where it stands as its own token, so the type name `myarray` is not
/// mistaken for an array of `my`.
fn strip_array_keyword(declared: &str) -> Option<&str> {
    let head = declared.trim_end();
    let split = head.len().checked_sub("ARRAY".len())?;
    let (element, keyword) = (head.get(..split)?, head.get(split..)?);
    if !keyword.eq_ignore_ascii_case("ARRAY") {
        return None;
    }
    let trimmed = element.trim_end();
    (trimmed.len() < split && !trimmed.is_empty()).then_some(trimmed)
}

/// Walk a chain of domains to the type name it bottoms out at — the declared
/// spelling of the first non-domain it reaches, or of `name` itself when that
/// is not a domain. The terminal is returned as the DDL spelled it, and a
/// reader asking whether it is a `box` tests it with [`is_box`].
///
/// [`resolve_array`] and the register's `array_comparison` test this terminal
/// rather than the declared spelling (I22, I26; `docs/design/decisions.md`,
/// "D41").
///
/// The loop is bounded by the type list's length — a domain chain visits each
/// `CREATE DOMAIN` at most once — which keeps a hand-edited file from
/// spinning rather than merely failing.
pub(crate) fn domain_terminal<'a>(name: &'a str, types: &'a [TypeDef]) -> &'a str {
    let mut name = name.trim();
    for _ in 0..=types.len() {
        match find_type(name, types).map(|t| &t.kind) {
            Some(TypeKind::Domain { base_type, .. }) => name = base_type.trim(),
            _ => break,
        }
    }
    name
}

/// Whether `declared` is a domain declaring `NOT NULL`, or a domain over one
/// — a chain [`domain_terminal`] walks — which `domain_in` refuses a NULL
/// under, `COPY` handing it each NULL field, and `array_in` and `record_in`
/// each NULL element and field (I76): asked of a column's declared type and
/// of each [`Position`]'s. An array of such a domain is not one: its column
/// holds a NULL array.
// pg-refuses: I76 — a domain's `NOT NULL`, which `domain_in` checks of a NULL.
pub(crate) fn domain_not_null(declared: &str, types: &[TypeDef]) -> bool {
    let mut name = declared.trim();
    for _ in 0..=types.len() {
        match find_type(name, types).map(|t| &t.kind) {
            Some(TypeKind::Domain { not_null: true, .. }) => return true,
            Some(TypeKind::Domain { base_type, .. }) => name = base_type.trim(),
            _ => return false,
        }
    }
    false
}

/// The `typdelim` of `declared`, through any chain of domains, a domain
/// copying its base type's (I22): `box`'s `;`, a base type's `DELIMITER`, and
/// otherwise `,` — `None` for a base type declaring one the preamble cannot
/// read, and for a shell, which no array is of.
pub(crate) fn typdelim(declared: &str, types: &[TypeDef]) -> Option<u8> {
    let terminal = domain_terminal(declared, types);
    if is_box(terminal) {
        return Some(b';');
    }
    match find_type(split_typmod(terminal).0, types).map(|t| &t.kind) {
        Some(TypeKind::Base { delimiter }) => *delimiter,
        Some(TypeKind::Shell) => None,
        _ => Some(b','),
    }
}

/// Whether `terminal` names the built-in `box`, read as [`builtin_name`]
/// reads every built-in: unqualified, and bare in any case or quoted in the
/// catalog's, as `--quote-all-identifiers` writes it (I8).
pub(crate) fn is_box(terminal: &str) -> bool {
    !split_typmod(terminal).0.contains('.') && builtin_name(terminal).0 == "box"
}

/// Map one declared type string — exactly as `pg_dump` wrote it, e.g. from
/// [`pgdt_model::DatabaseMetadata::tables`] — against `types`, that
/// same database's `CREATE TYPE`/`CREATE DOMAIN` list.
///
/// The array check runs first because a declared array type still carries its
/// element type's own qualification (`public.mood[]` contains a `.` too), and
/// `pg_dump` never preserves dimensionality (I21). It runs through
/// [`array_element`], so every spelling of the array-bounds production
/// collapses to the element type plus one array level (I28) before anything
/// else looks at the string.
pub fn resolve_declared_type(declared: &str, types: &[TypeDef]) -> TypeOutcome {
    resolve_walk(declared, types, Visits::over(types))
}

/// [`resolve_declared_type`], at one step of its walk.
fn resolve_walk(declared: &str, types: &[TypeDef], visits: Visits) -> TypeOutcome {
    let declared = declared.trim();
    if let Some(element) = array_element(declared) {
        return resolve_array(element, types, visits);
    }
    let (base, _) = split_typmod(declared);
    if base.contains('.') {
        return resolve_user_type(base, types, visits);
    }
    let (base, typmod, _) = builtin_name(declared);
    map_builtin(&base, typmod, types, visits).unwrap_or(TypeOutcome::Unknown)
}

#[cfg(test)]
mod tests;
