//! Declared PostgreSQL type string -> Arrow `DataType`
//! (`docs/design/decisions.md`, "Type resolution and decoders").
//!
//! Pure, synchronous, no I/O (`docs/design/decisions.md`, "D74"). A declared
//! type is resolved against a single database's [`TypeDef`] list (already
//! selected by the caller — [`crate::resolve`] is the one that picks which
//! database), never against files or offsets.

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Fields};
// The canonical extension types live in `arrow-schema`, which `arrow`'s own
// `datatypes` re-export does not cover — see `CanonicalExtension`.
use arrow_schema::extension::{Json, Uuid};

use crate::copy::Cursor;
use crate::preamble::{CollationDef, TypeDef, TypeKind, canonical_type_name};
use crate::statistics::BoundsSet;

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
    /// [`crate::PostgresInvalidValues::Ignore`].
    pub not_null: bool,
}

impl<T> Position<T> {
    /// A position a NULL may sit at.
    pub fn nullable(plan: T) -> Self {
        Self { plan, not_null: false }
    }

    /// The position of a value declared `declared`, refusing a NULL where
    /// that is a `NOT NULL` domain.
    fn of(plan: T, declared: &str, types: &[TypeDef]) -> Self {
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

/// How one column's field text becomes a value two sides of a comparison can
/// be ordered by — the decoding half of a [`ComparisonPlan`], and the only
/// thing `crate::predicate` needs in order to read a side.
///
/// A small closed vocabulary rather than the Arrow type, which does not
/// correspond to it (`docs/design/decisions.md`, "D40").
///
/// **Not `Copy`**: several variants carry the column's own facts — an enum's
/// labels, a typmodded `numeric`'s precision and scale, whether one held as
/// text admits the infinities — and the labels are not `Copy`.
/// The kind is cloned into a term as the term's literal is resolved
/// (`crate::predicate`'s `Comparison::Decoded`), once per term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareKind {
    Bool,
    /// A signed integer of `bytes` bytes — `smallint`, `integer` and `bigint`
    /// at 2, 4 and 8. Every value orders as an `i64`; the width is what a
    /// filter literal in PostgreSQL's semantics is refused past, as
    /// `int2in`, `int4in` and `int8in` refuse it (I60).
    Int {
        bytes: u8,
    },
    /// An unsigned 32-bit integer — `oid`. Held apart from [`Self::Int`]
    /// because the two differ on a *literal* carrying a minus sign, which
    /// `oidin` wraps and this refuses; the values themselves order
    /// identically.
    UnsignedInt,
    Float32,
    Float64,
    /// A `numeric(p,s)` typed as an Arrow decimal at `scale`, compared as an
    /// unscaled integer there: a field put through its typmod
    /// ([`crate::decode::typmod_unscaled_digits`]), a literal read exactly
    /// ([`crate::decode::decimal_unscaled_digits`]).
    Decimal {
        precision: u16,
        scale: i8,
    },
    Date,
    Time,
    Timestamp {
        with_tz: bool,
    },
    Uuid,
    Bytea,
    /// Bytewise over the text the file holds. `length` is a `character
    /// varying(n)`'s `n`, past which a field is refused but for trailing
    /// blanks ([`crate::decode::char_typmod_refuses`]); `None` for every
    /// other text, and for a literal's reading, which no typmod reaches.
    /// The length refuses a field and never moves a key.
    Text {
        length: Option<u32>,
    },
    /// `character(n)`: bytewise over the text the file holds, after **both**
    /// sides give up their trailing blanks. A dump writes every value padded
    /// to `n` and `bpcharcmp` calls `bcTruelen` on both operands before
    /// consulting a collation (I38), so the padding is not part of the value
    /// and the clause decides the verdict, never the comparison. `length` is
    /// `n`, as [`Self::Text`]'s is; `None` for `bpchar`.
    PaddedText {
        length: Option<u32>,
    },
    /// Arbitrary-precision decimal read straight out of the text the file
    /// holds — a bare `numeric`, or one whose typmod `Decimal256` cannot
    /// carry: a precision or scale past 76 digits, a scale below -128, or a
    /// typmod that does not read. There is no scale to carry both sides to, so
    /// the comparison normalizes rather than rescales: `1.5` and `1.50` are
    /// one value written two ways.
    ///
    /// `infinities` says whether `Infinity`/`-Infinity` are values of the
    /// column. Any typmod rejects an infinity (I34), so only the bare form
    /// admits the spelling, and it is the bare form's field alone that
    /// `numeric_in`'s storage bounds reach, a typmod rounding first (I63).
    ///
    /// `typmod` is the declared one, which a field is put through
    /// ([`crate::decode::typmod_unscaled_digits`]) before it is normalized;
    /// `None` for the bare form, and for a typmod this build cannot read.
    Numeric {
        infinities: bool,
        typmod: Option<NumericTypmod>,
    },
    /// An enum, compared by each label's position in the type's own
    /// declaration order (I33) rather than by its text. The labels are
    /// `TypeKind::Enum`'s, verbatim and in that order, which is what
    /// `CREATE TYPE … AS ENUM (…)` writes and what a `--binary-upgrade`
    /// dump's `ALTER TYPE … ADD VALUE` run is folded back into (I6).
    ///
    /// `exact` is `TypeKind::Enum`'s: the labels are all the type holds, so
    /// a field naming another is one `enum_in` refuses (I70).
    Enum {
        labels: Arc<[String]>,
        exact: bool,
    },
    /// `interval`, compared by `interval_cmp_value`'s span: months collapse
    /// to 30 days and days to 86400 seconds, so `1 mon`, `30 days` and
    /// `720:00:00` are one value written three ways (I40). The span needs 128
    /// bits.
    ///
    /// Carries the two infinities unconditionally, in `date_out`'s spellings
    /// rather than `numeric_out`'s (I34). They are v17 values, read on an
    /// older file under the union rule (I35).
    ///
    /// `qualifier` is the column's field qualifier, under which `interval_in`
    /// reads a field and a strict parse checks one
    /// ([`crate::datetime_in::interval_reads`]); `None` for a column with
    /// none, and for a literal's reading, which no typmod reaches. It refuses
    /// a field and never moves a key.
    Interval {
        qualifier: Option<IntervalQualifier>,
    },
    /// `time with time zone`, compared by the UTC-equivalent instant first
    /// and by the stored zone second, so two values are equal only when both
    /// halves are (I40) — `00:00:00+00` and `01:00:00+01` are the same
    /// instant and are *not* equal.
    TimeTz,
    /// `inet` and `cidr`, compared by `network_cmp_internal`: family, then
    /// the shorter netmask's worth of address bits, then the netmask length,
    /// then the whole address (I40). Not a byte order.
    ///
    /// `cidr` is the only difference between the two: `cidr_in` refuses a
    /// value with a bit set below its netmask and `inet_in` accepts one.
    Network {
        cidr: bool,
    },
    /// `macaddr` (six octets) and `macaddr8` (eight), compared as their
    /// bytes (I40). The width is carried because it is the whole of what
    /// separates the two types.
    MacAddr {
        octets: usize,
    },
    /// `jsonb`, compared as `compareJsonbContainers` compares it: a walk down
    /// two containers in lockstep, deciding on the first position where they
    /// differ — the *kind* there first (an object outranks an array, an array
    /// every scalar, a boolean a number), then a container's element or pair
    /// count, then the members themselves (I41).
    ///
    /// It cannot reproduce a string leaf: every JSON string, object keys
    /// included, is ordered by `varstr_cmp` under `DEFAULT_COLLATION_OID`
    /// (I41) — the database's collation, absent from a plain dump (I32) — so
    /// the column carries [`ComparisonDivergence::JsonbStringCollation`].
    Jsonb,
    /// `interval` under Arrow's order of `Interval(MonthDayNano)`: months,
    /// then days, then the time part, each compared alone, so `30 days` is
    /// below `1 mon` where [`Self::Interval`] equates them. Produced only by
    /// [`Self::datafusion_order`], which keeps the qualifier, the field being
    /// the one the server read.
    IntervalFields {
        qualifier: Option<IntervalQualifier>,
    },
}

/// Which order a query's comparisons answer in.
///
/// **One mode per query**, never per term: DataFusion decides per plan which
/// filters reach a provider, so a filter answered in one semantics when
/// pushed and another when not makes a query's rows depend on its plan
/// (`docs/design/decisions.md`, "D40").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComparisonSemantics {
    /// PostgreSQL's comparison of the declared type, as the register gives
    /// it — `pgdt`'s, and the default.
    #[default]
    Postgres,
    /// DataFusion's comparison of the value the column emits — Arrow's `cmp`
    /// kernels over the batch this build produces, a float's `-0` first made
    /// `0` (DataFusion's `apply_cmp`). A comparison this build cannot answer
    /// that way is refused rather than answered in the other semantics.
    DataFusion,
}

impl CompareKind {
    /// The kind whose order over this kind's field text is
    /// [`ComparisonSemantics::DataFusion`]'s order over the value its column emits.
    /// A kind whose key *is* the emitted value is its own. A float is its own
    /// too: with `-0` made `0`, IEEE `totalOrder` over what a dump holds — one
    /// `NaN`, which it writes as `NaN` — is `float8_cmp`'s. **Every kind
    /// emitted as text becomes [`Self::Text`]**, an enum (emitted
    /// `Dictionary`, compared by label text) and `character(n)` (emitted
    /// padded) among them, and `macaddr` too, though the file's text orders
    /// as its octets do (`docs/design/decisions.md`, "D40"). `interval`
    /// becomes its field-wise variant.
    ///
    /// A special value the emitted type cannot hold (D96) keeps its rank in
    /// the key, which no Arrow value contradicts.
    pub fn datafusion_order(&self) -> CompareKind {
        match self {
            Self::Interval { qualifier } => Self::IntervalFields { qualifier: *qualifier },
            Self::Enum { .. }
            | Self::Numeric { .. }
            | Self::TimeTz
            | Self::Network { .. }
            | Self::MacAddr { .. }
            | Self::Jsonb => Self::Text { length: None },
            // The emitted text is the field's, so its length refuses as it
            // did.
            Self::PaddedText { length } => Self::Text { length: *length },
            kind => kind.clone(),
        }
    }

    /// How [`Self::datafusion_order`]'s comparison differs from this kind's —
    /// `None` exactly where it leaves the kind alone. A float inside a nested
    /// column differs besides ([`NestedCompare::datafusion_divergences`]).
    pub fn datafusion_divergence(&self) -> Option<ComparisonDivergence> {
        match self {
            Self::Enum { .. } => Some(ComparisonDivergence::LabelText),
            Self::Numeric { .. }
            | Self::TimeTz
            | Self::Network { .. }
            | Self::MacAddr { .. }
            | Self::Jsonb => Some(ComparisonDivergence::ValueAsText),
            Self::Interval { .. } => Some(ComparisonDivergence::IntervalFields),
            Self::PaddedText { .. } => Some(ComparisonDivergence::PaddedText),
            _ => None,
        }
    }
}

/// What a position of `kind` diverges in under [`ComparisonSemantics::DataFusion`],
/// given the register's own `divergence` for it: [`CompareKind::datafusion_divergence`],
/// then the register's, but for `jsonb`'s string collation, the whole value
/// being compared as text there.
pub(crate) fn datafusion_position_divergences(
    kind: &CompareKind,
    divergence: Option<ComparisonDivergence>,
) -> Vec<ComparisonDivergence> {
    kind.datafusion_divergence()
        .into_iter()
        .chain(divergence.filter(|d| *d != ComparisonDivergence::JsonbStringCollation))
        .collect()
}

/// How a comparison here differs from PostgreSQL's own for the same declared
/// type. The declared type then sharpens the sentence a user reads, since
/// several declared types share one divergence for different reasons (see
/// `crate::predicate::ComparisonNote::message`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonDivergence {
    /// The column is held as text and compared bytewise where the server has
    /// no comparison at all. **`json` is its one member**: PostgreSQL defines
    /// no `=`, no order and no operator class for it, so bytewise offers
    /// *more* than the server does rather than less. Every other type the
    /// register holds as text carries a comparison of its own, and a
    /// collatable one either agrees or carries one of the three collation
    /// variants below; a type it does not model is [`Self::UnmodelledType`].
    AsText,
    /// A collatable text column whose collation the file does not state: it
    /// carries no `COLLATE` clause and its type's default collation is the
    /// *database's*, which a plain dump never records (I32). Bytewise is
    /// PostgreSQL's answer only if that collation is `C`/`POSIX`.
    ///
    /// deficiency: KD40 — a `--create` or `pg_dumpall` dump *does* state the
    /// database's collation, in its `CREATE DATABASE`, which `preamble.rs`
    /// leaves `Unparsed`; so this fires there too, and its sentence says the
    /// dump does not record what it does. Read, a `C`/`POSIX` database would
    /// make its uncollated columns agree outright and any other would make
    /// them [`Self::NonBytewiseCollation`], per database of a `pg_dumpall`.
    /// **(c) unowned**.
    UnknownCollation,
    /// A collatable column that states a collation other than `C`/`POSIX`,
    /// whose order this build does not implement.
    ///
    /// Deficiency register: `deficiency: KD7` — this and
    /// [`Self::NonDeterministicCollation`] are the two rows whose divergence
    /// the file states and this build does not implement, so the row set is
    /// not the server's: under `<`/`>` always, and under `=`/`!=` where the
    /// dump declares the collation non-deterministic (I42). **(c) unowned**;
    /// closing it means a comparison per named collation, up to a provider
    /// version. The variants either side are *not* this: [`Self::UnknownCollation`]
    /// names a fact no plain dump carries (I32), and a stated collation that is
    /// bytewise in fact but not named `C`/`POSIX` lands here with correct rows
    /// and a spurious note.
    NonBytewiseCollation,
    /// A collatable column stating a collation the *same dump* declares
    /// `deterministic = false` (I42). It is [`Self::NonBytewiseCollation`]
    /// plus one fact: under a non-deterministic collation `texteq`/`bpchareq`
    /// are not byte comparisons either, so two values that differ byte for
    /// byte can be equal to the server and this build calls them distinct.
    ///
    /// **It is the only divergence a plain dump states outright** — the
    /// collation's *order* still needs a provider version the file does not
    /// carry, but `pg_dump` writes `, deterministic = false` unconditionally
    /// (I42).
    ///
    /// Deficiency `KD7` reaches one operator further here; its detail is at
    /// the marker on [`Self::NonBytewiseCollation`].
    NonDeterministicCollation,
    /// A `jsonb` column, whose *structure* is compared exactly and whose
    /// string leaves and object keys are not: `compareJsonbScalarValue` orders
    /// every one of them by `varstr_cmp` under `DEFAULT_COLLATION_OID` (I41),
    /// which is the database's collation and is absent from a plain dump
    /// (I32). Held apart from [`Self::UnknownCollation`] because the column
    /// states nothing and could not: `jsonb` is not collatable.
    JsonbStringCollation,
    /// A column whose declared type is not nested and has no comparison in
    /// this register at all — `box`, `money`, `xml`, a user-defined base type,
    /// each of which resolves to text as `UnknownType` or `OpaqueBaseType`. An ordering operator is *refused* on such a
    /// column ([`ComparisonPlan::Refused`]); `=`/`!=` are not, because the
    /// text a dump holds is canonical `*_out` form and a byte comparison over
    /// it is right for most of these types. This says it is not right for all
    /// of them.
    ///
    /// Deficiency register: `deficiency: KD10` — that bytewise `=` is not the
    /// server's answer for the geometric types, so the row set is wrong:
    /// `box_eq` compares *areas*, calling `(1,1),(0,0)` and `(3,3),(2,2)`
    /// equal where the bytes differ (the committed oracle holds it,
    /// `fixtures/<13-18>/oracle/comparisons.tsv`, `public.box_domain`), and
    /// the announcement misses a type reached through a container (`box[]`).
    /// **(c) unowned.** Closing it means a comparison for each such type;
    /// which member is like `box` and which like `money` — whose `*_out` is
    /// unique per value, so bytewise agrees — is what this register does not
    /// know, and announcing is the conservative answer.
    ///
    /// *Rejected: refusing `=` here as ordering is refused.* It takes a
    /// working capability away from every type in the group to protect the
    /// geometric handful, where an announcement protects both — and for
    /// `xml`, whose `=` PostgreSQL does not define at all, filtering by exact
    /// text is a thing a user legitimately wants.
    UnmodelledType,
    /// An enum under [`ComparisonSemantics::DataFusion`]: DataFusion compares the
    /// emitted `Dictionary` by its label text, where PostgreSQL orders labels
    /// by declaration (I33). Equality agrees, a label being unique.
    ///
    /// This and the five variants after it are DataFusion semantics' own:
    /// produced by [`CompareKind::datafusion_divergence`],
    /// [`NestedCompare::datafusion_divergences`] and
    /// `crate::predicate::column_divergences`, never by the register — as
    /// [`CompareKind::IntervalFields`] is produced by
    /// [`CompareKind::datafusion_order`] alone.
    LabelText,
    /// A type emitted as `Utf8View` whose PostgreSQL comparison is by value —
    /// a bare `numeric` or one `Decimal256` cannot carry, `timetz`, `inet`/`cidr`, `macaddr`/`macaddr8`,
    /// `jsonb` — which DataFusion compares bytewise. Equality too: a literal
    /// matches only the text the server writes, so `'12:00+00'` misses
    /// `12:00:00+00`, `'10.0.0.1/32'` misses an `inet`'s `10.0.0.1` and
    /// `'08:00:2B:01:02:03'` misses `08:00:2b:01:02:03`, and a bare `numeric`
    /// writes one value two ways (`1.5` and `1.50`).
    ///
    /// A `macaddr`'s reaches only a literal not in the server's spelling: the
    /// file's fixed-width lowercase hex orders as the octets do, so its order
    /// over the file's values agrees, and an uppercase literal still moves an
    /// ordering term's answer (`v < 'A0:…'`).
    ValueAsText,
    /// `interval` as DataFusion compares `Interval(MonthDayNano)`: months,
    /// then days, then the time part, where PostgreSQL compares the span
    /// (I40), so `1 mon` and `30 days` are equal only to the server.
    IntervalFields,
    /// `character(n)` compared with its blank padding, as it is emitted,
    /// where `bpcharcmp` trims both sides first (I38). Equality too: an
    /// unpadded literal (`'abc'` against a `character(5)`'s `abc  `) matches
    /// nothing, and a bare `bpchar` keeps a value's trailing blanks.
    PaddedText,
    /// A nested column — array, composite, range, multirange — which
    /// DataFusion orders with `make_comparator` under default options, an
    /// order upstream marks for change and this build does not evaluate
    /// (`docs/design/decisions.md`, "D40"). **Order alone**: it puts a NULL
    /// element or field first where `array_cmp` and `record_cmp` put it
    /// last, and both call two NULLs equal (`array_eq`, `record_eq`), so what
    /// reaches equality is a position's own divergence, reported under its
    /// path beside this one.
    NestedOrder,
    /// A float position inside a nested column, which `make_comparator`
    /// orders by IEEE `totalOrder` with no `-0` made `0` — the normalization
    /// DataFusion's `apply_cmp` gives a float column and not one nested in a
    /// list or struct — so `-0` is below `0` and unequal to it, where
    /// `float8eq` equates them. Upstream, as of DataFusion 55.1.0: `apply_cmp`
    /// runs `normalize_cmp_input` over flat operands, and
    /// `compare_op_for_nested` calls `make_comparator` with nothing before it;
    /// a release normalizing there too makes this variant a false note.
    ///
    /// Announced from the schema alone, at every float position, as
    /// [`Self::UnknownCollation`] is on the possibility: the census does not
    /// record whether a `-0` occurs, and pricing a warning is no reason to make
    /// it.
    ///
    /// *Rejected: emitting a nested `-0` as `0`.* Filters would then agree
    /// with the server, and a projection would not — `{0}` shown where
    /// PostgreSQL writes `{-0}`, and a flat float column keeping its `-0`
    /// beside it. Upstream normalizes inside the comparison, never the value.
    UnnormalizedZero,
}

impl ComparisonDivergence {
    /// Whether this divergence reaches `=`/`!=` as well as the four ordering
    /// operators.
    ///
    /// **Determinism decides it for the collation variants**: a
    /// *deterministic* collation makes `varstr_cmp` return zero exactly when
    /// the bytes are equal, so `texteq`/`bpchareq` are byte comparisons
    /// whatever that collation otherwise orders, leaving three of the four
    /// divergences of *order* alone. [`Self::NonDeterministicCollation`] is
    /// the one a dump states outright (I42); the other six `true` answers are
    /// not about a collation at all.
    ///
    /// There is no `affects_ordering` beside this: every variant does.
    pub fn affects_equality(self) -> bool {
        match self {
            // The server has no `=` to disagree with (`json`), or the
            // register models none.
            Self::AsText | Self::UnmodelledType => true,
            // The dump itself says `texteq` is not a byte comparison here.
            Self::NonDeterministicCollation => true,
            Self::UnknownCollation | Self::NonBytewiseCollation | Self::JsonbStringCollation => {
                false
            }
            // A label is unique, and a NULL equals a NULL on both sides.
            Self::LabelText | Self::NestedOrder => false,
            // A literal matches only the emitted text: a spelling other than
            // the server's own (`'12:00+00'`, an unpadded `character(n)`)
            // misses a value PostgreSQL matches, and some members also write
            // one stored value two ways (`1.5` and `1.50`).
            Self::ValueAsText | Self::IntervalFields | Self::PaddedText => true,
            Self::UnnormalizedZero => true,
        }
    }
}

/// A collatable type's *default* collation — `pg_type.typcollation`, which is
/// what `pg_dump` compares a column's collation against when deciding whether
/// to write a `COLLATE` clause at all (I37), so the absence of a clause means
/// this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeCollation {
    /// `pg_type.typcollation = C` — `name`'s. A bare column of such a type is
    /// bytewise on every server.
    Bytewise,
    /// `pg_type.typcollation = default` — `text`, `varchar`, `bpchar`. A bare
    /// column of such a type is on the database's collation, which the file
    /// does not carry (I32).
    Database,
}

/// How one **nested** column compares, one node per nesting level — the
/// structural comparison `array_cmp`/`record_cmp` make, keyed on the
/// declared type exactly as a scalar's [`CompareKind`] is.
///
/// **Comparability is inherited, and so is divergence.** A nested column is
/// ordered exactly when every type beneath it is; a position whose declared
/// type this build has no order for is an [`Self::Uncomparable`] leaf, which
/// makes [`ComparisonPlan::orders`] answer `false` for the whole column and
/// names the position that did it. A position that *is* ordered but not the
/// server's way carries its own [`ComparisonDivergence`], so a `text[]`
/// column diverges for the reason its element does.
///
/// **`json` beneath a nested type is a refusal, not a divergence**:
/// `array_cmp` raises when the element type has no comparison proc, so a
/// `json[]` column has no `=` and no `<` on the server either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NestedCompare {
    /// A scalar position: the declared type as the DDL spelled it, and the
    /// comparison a column of it would have had.
    Leaf { declared: String, kind: CompareKind, divergence: Option<ComparisonDivergence> },
    /// A position whose declared type has no order here — what refuses the
    /// column, and what lets the refusal name the type that caused it.
    ///
    /// **`divergence` is about the column's `=`, not about this position's
    /// order**: the column falls back to a byte comparison of the container's
    /// whole `*_out` text, and this says what that fallback costs *here*.
    /// [`ComparisonDivergence::AsText`] is the one value it takes — `json`,
    /// where the server has no equality either. `None` makes no claim about
    /// the server's `=`: a position only *this build* declines (I22, I26, an
    /// empty enum), or one whose type the register does not model, whose
    /// server equality may not be the text's (`KD10`).
    ///
    /// `grammar` is the position's input grammar where this build reads one
    /// for a type it orders by nothing ([`text_grammar`]), and `delimiter`
    /// its type's `typdelim`, which an array of it is split at ([`typdelim`]).
    Uncomparable {
        declared: String,
        divergence: Option<ComparisonDivergence>,
        grammar: Option<TextGrammar>,
        delimiter: Option<u8>,
    },
    /// `array_cmp`: elements first, up to the shorter array's length, then
    /// element count, dimension count, dimensions and lower bounds (I45).
    /// **One node whatever the dimensionality** — an `array_out` literal
    /// carries its own shape and [`crate::nested::ArrayLiteral`] flattens it,
    /// so `integer[]` is one `Array` node at any depth.
    Array(Box<Position<NestedCompare>>),
    /// `record_cmp`: field-wise in declaration order, which is also the order
    /// `record_out` writes them in. The name is carried for the diagnostic
    /// path alone — `record_out` is positional (I23) and no comparison reads
    /// it.
    Record(Vec<(String, Position<NestedCompare>)>),
    /// `range_cmp`: `empty` below every other value, then lower bound, then
    /// upper, with a bound settling infinity before value and value before
    /// inclusivity (I46). `bound` is the subtype's own node, so a range over
    /// a composite composes like any other position.
    ///
    /// **`discrete` is a property of the range type, never of its subtype**:
    /// only `int4range`, `int8range` and `daterange` carry a canonical
    /// function among the built-ins, so it is set from the range's *name*. A
    /// user-defined range never reaches this node with it set — one declaring
    /// a `canonical` function is [`ComparisonPlan::Unanswerable`] rather than
    /// a tree (I46).
    Range { bound: Box<NestedCompare>, discrete: Option<Discrete> },
    /// `multirange_cmp`: member-wise over members the server has already
    /// sorted, coalesced and emptied out, the shorter multirange first
    /// (I46). The fields are the *member range's*, since a multirange has no
    /// comparison of its own beyond the sequence.
    Multirange { bound: Box<NestedCompare>, discrete: Option<Discrete> },
    /// `int2vector`, compared by `array_cmp` over `smallint` elements: the
    /// type names no operator of its own, and `anyarray` polymorphism
    /// resolves `<` and `=` for it (I47). [`Self::Array`]'s comparison over a
    /// fixed element node, read in `int2vectorout`'s grammar rather than
    /// `array_out`'s — which is why it is a variant of its own.
    ///
    /// It carries nothing: the element is always `smallint`, so no position
    /// here could refuse an order or announce a divergence.
    Int2Vector,
}

/// The canonical function of a built-in discrete range type —
/// `int4range_canonical`, `int8range_canonical` or `daterange_canonical` —
/// which are one rewriting differing only in where the successor leaves the
/// subtype (I46).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discrete {
    Int4,
    Int8,
    Date,
}

impl Discrete {
    /// The subtype's largest value, in the key a bound of it is read into:
    /// a bound here has no successor, and the canonical function raises
    /// `integer`, `bigint` or `date out of range` on it (I46). A leaf's key is
    /// an `i64` whatever the column's width, so the width is stated here
    /// rather than found in the key. A literal's bound past the width itself,
    /// `int4range '[1,3000000000)'`, is refused before any successor is taken,
    /// as `int4in` refuses it (I60); a field's is read as written.
    ///
    /// A date's is `5874897-12-31` as days since 1970, the day before
    /// `IS_VALID_DATE`'s exclusive `DATE_END_JULIAN`.
    pub(crate) fn largest(self) -> i64 {
        match self {
            Discrete::Int4 => i64::from(i32::MAX),
            Discrete::Int8 => i64::MAX,
            Discrete::Date => 2_145_042_905,
        }
    }
}

impl NestedCompare {
    /// The first position beneath this one with no order, as
    /// `(path, declared type, divergence)` — `None` when every position is
    /// comparable. The path is the accessor a user would write where one
    /// exists: `[]` for an element, `.name` for a field, appended as the walk
    /// descends. A range's bound has no subscript spelling, so `.bound` names
    /// it and a multirange's is `[].bound`. The divergence is the position's
    /// own, which is what tells a refusal the server shares (`json`'s
    /// [`ComparisonDivergence::AsText`]) from one only this build makes.
    pub fn uncomparable(&self) -> Option<(String, String, Option<ComparisonDivergence>)> {
        let mut found = None;
        self.walk(&mut String::new(), &mut |path, leaf| {
            if let Self::Uncomparable { declared, divergence, .. } = leaf
                && found.is_none()
            {
                found = Some((path.to_string(), declared.clone(), *divergence));
            }
        });
        found
    }

    /// Every position beneath this one with something to announce, as
    /// `(path, declared type, divergence)`, in walk order. A column carrying
    /// two of them announces both.
    ///
    /// **An [`Self::Uncomparable`] position can be one of them**, and its
    /// entry is about a different comparison from the rest: the others say
    /// the order here is not the server's, where it says the *bytewise `=`*
    /// the whole column falls back to is not.
    pub fn divergences(&self) -> Vec<(String, String, ComparisonDivergence)> {
        let mut out = Vec::new();
        self.walk(&mut String::new(), &mut |path, leaf| {
            let divergence = match leaf {
                Self::Leaf { divergence, .. } | Self::Uncomparable { divergence, .. } => {
                    *divergence
                }
                _ => None,
            };
            out.extend(divergence.map(|d| (path.to_string(), leaf.declared().to_string(), d)));
        });
        out
    }

    /// [`Self::divergences`] under [`ComparisonSemantics::DataFusion`]: what each
    /// position's own comparison differs in from the server's when DataFusion
    /// compares the emitted value with `make_comparator`, in walk order. A
    /// leaf reports what a column of its kind would
    /// ([`datafusion_position_divergences`]) plus [`ComparisonDivergence::UnnormalizedZero`]
    /// for a float, and an [`Self::Uncomparable`] position keeps what its
    /// bytewise `=` costs. The container's own order is the column's
    /// [`ComparisonDivergence::NestedOrder`], not a position's.
    pub fn datafusion_divergences(&self) -> Vec<(String, String, ComparisonDivergence)> {
        let mut out = Vec::new();
        self.walk(&mut String::new(), &mut |path, leaf| {
            let divergences = match leaf {
                Self::Leaf { kind, divergence, .. } => {
                    let mut found = datafusion_position_divergences(kind, *divergence);
                    if matches!(kind, CompareKind::Float32 | CompareKind::Float64) {
                        found.push(ComparisonDivergence::UnnormalizedZero);
                    }
                    found
                }
                Self::Uncomparable { divergence, .. } => divergence.iter().copied().collect(),
                _ => Vec::new(),
            };
            out.extend(
                divergences.into_iter().map(|d| (path.to_string(), leaf.declared().to_string(), d)),
            );
        });
        out
    }

    /// A leaf's declared type as the DDL spelled it; `int2vector` for its
    /// fixed element.
    fn declared(&self) -> &str {
        match self {
            Self::Leaf { declared, .. } | Self::Uncomparable { declared, .. } => declared,
            _ => "int2vector",
        }
    }

    /// Depth-first over every leaf — [`Self::Leaf`], [`Self::Uncomparable`]
    /// or [`Self::Int2Vector`] — handing each its path and the node.
    fn walk(&self, path: &mut String, visit: &mut impl FnMut(&str, &Self)) {
        match self {
            Self::Leaf { .. } | Self::Int2Vector | Self::Uncomparable { .. } => visit(path, self),
            Self::Array(element) => {
                let len = path.len();
                path.push_str("[]");
                element.plan.walk(path, visit);
                path.truncate(len);
            }
            Self::Record(fields) => {
                for (name, field) in fields {
                    let len = path.len();
                    path.push('.');
                    path.push_str(name);
                    field.plan.walk(path, visit);
                    path.truncate(len);
                }
            }
            Self::Range { bound, .. } => {
                let len = path.len();
                path.push_str(".bound");
                bound.walk(path, visit);
                path.truncate(len);
            }
            Self::Multirange { bound, .. } => {
                let len = path.len();
                path.push_str("[].bound");
                bound.walk(path, visit);
                path.truncate(len);
            }
        }
    }
}

/// The comparison register's answer for one declared type: how a column of it
/// compares, and whether that is the order PostgreSQL itself defines
/// (`docs/design/decisions.md`, "D40").
///
/// "This type has no order here" and "there is no way to decode a value of
/// it" are one variant, not a pairing that could come to disagree.
///
/// Carried per column in [`crate::resolve::ResolvedSchema::comparisons`] and
/// consumed by `crate::predicate`, which reads it instead of inspecting the
/// Arrow type.
///
/// **No variant refuses on PostgreSQL's terms**, so no operator refusal
/// carries a `pg-refuses` marker; a variant that would is the case
/// `scripts/pg_refuses.py`'s docstring says reopens that.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ComparisonPlan {
    /// Values decode through `kind` and compare by that order; `divergence`
    /// is `None` when the order is PostgreSQL's own.
    Compared { kind: CompareKind, divergence: Option<ComparisonDivergence> },
    /// A nested column — array, composite, range or multirange — compared
    /// structurally, one node per nesting level. **A `Nested` plan is not by
    /// itself an order**: a tree holding a [`NestedCompare::Uncomparable`]
    /// position is the register saying the column has none, which is what
    /// [`Self::orders`] answers and what lets the refusal name the position.
    Nested(NestedCompare),
    /// **No comparison at all** — every operator is refused, `=` and `!=`
    /// included, and the payload says why.
    ///
    /// The stronger of the two refusals. [`Self::Refused`] means "no
    /// *order*", and lets `=`/`!=` fall through to a bytewise comparison of
    /// the file's own canonical text; this variant is for the case where the
    /// file *states* that the server's equality is not that comparison.
    Unanswerable(UnanswerableReason),
    /// No order is defined here for this declared type.
    #[default]
    Refused,
}

/// Why the register can answer no operator at all for a column — the payload
/// of [`ComparisonPlan::Unanswerable`].
///
/// A named reason rather than a message, as [`ComparisonDivergence`] is: the
/// sentence a user reads is written beside the grammar it is about, so a new
/// producer has to say which kind of unanswerable it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnanswerableReason {
    /// The column's type is, or contains, a range type whose DDL declares a
    /// `canonical` function (I46): the server rewrites every value through
    /// arbitrary server-side code this build cannot run. Both names are
    /// carried so the refusal can say which type did it.
    RangeCanonical { range_type: String, function: String },
}

/// Which of the sets gathering stored under `stored` — one kind per
/// [`BoundsSet`], as [`ComparisonPlan::bounds_kinds`] names them — a term
/// comparing by `kind` reads: the set keyed by that kind, or a `macaddr` set
/// for a term comparing as text, its field text ordering as its octets do
/// (I40). `None` where no stored set is ordered as `kind` orders.
///
/// **The set is chosen by what gathering stored, not by the term's own plan**
/// (`docs/design/decisions.md`, "D79"): under `SchemaMode::Strings` every
/// column is [`ComparisonPlan::Refused`] and compares as text, while gathering
/// resolved it typed, so an `integer`'s set, keyed `Int`, is never read as
/// text, and a text column's `Text` set is. A text's length is not part of
/// its key, refusing a field and never moving one, so a `character
/// varying(n)` column's set serves a term comparing as any text.
pub fn bounds_set_keyed_by(
    stored: &[Option<CompareKind>; 2],
    kind: &CompareKind,
) -> Option<BoundsSet> {
    let serves = |stored: &CompareKind| match (stored, kind) {
        (CompareKind::Text { .. } | CompareKind::MacAddr { .. }, CompareKind::Text { .. })
        | (CompareKind::PaddedText { .. }, CompareKind::PaddedText { .. }) => true,
        _ => stored == kind,
    };
    [BoundsSet::Primary, BoundsSet::DataFusion]
        .into_iter()
        .zip(stored)
        .find_map(|(set, stored)| stored.as_ref().is_some_and(serves).then_some(set))
}

impl ComparisonPlan {
    /// Whether the register gives a column of this plan an order at all —
    /// the question the four ordering operators ask, and the one a caller
    /// must not answer by matching on the variant: a nested plan whose tree
    /// holds an uncomparable position is `Refused` in every way that matters.
    pub fn orders(&self) -> bool {
        match self {
            Self::Compared { .. } => true,
            Self::Nested(tree) => tree.uncomparable().is_none(),
            Self::Unanswerable(_) | Self::Refused => false,
        }
    }

    /// The kinds gathering orders a scalar column's bounds and row order by,
    /// one per [`BoundsSet`]: **one set in every semantics whose order over the
    /// file's text is exact, shared where two coincide**
    /// (`docs/design/decisions.md`, "D79").
    ///
    /// The primary set is the register's kind where its order is exact,
    /// which is PostgreSQL's, and otherwise DataFusion's ([`CompareKind::datafusion_order`]),
    /// which is always exact over the text the column emits: a divergent text
    /// column's bytewise order, `jsonb`'s and a `character(n)`'s off `C`
    /// (padded) as text, and a column with no plan bytewise, as DataFusion
    /// semantics compares it. The DataFusion set is a second one, for an exact
    /// kind whose DataFusion order is another — but `macaddr`, whose field text orders
    /// as its octets do (I40), so its one set serves both.
    pub fn bounds_kinds(&self) -> [Option<CompareKind>; 2] {
        match self {
            Self::Compared { kind, divergence: None } => {
                let datafusion = kind.datafusion_order();
                let second = (datafusion != *kind && !matches!(kind, CompareKind::MacAddr { .. }))
                    .then_some(datafusion);
                [Some(kind.clone()), second]
            }
            Self::Compared { kind, divergence: Some(_) } => [Some(kind.datafusion_order()), None],
            Self::Refused => [Some(CompareKind::Text { length: None }), None],
            Self::Nested(_) | Self::Unanswerable(_) => [None, None],
        }
    }

    /// Whether gathering keeps bounds and row order for a scalar column of
    /// this plan: wherever some semantics' order over its text is exact
    /// ([`Self::bounds_kinds`]).
    pub fn gathers_bounds(&self) -> bool {
        self.bounds_kinds()[0].is_some()
    }

    /// Which stored set of bounds and row order is ordered as `semantics`
    /// orders the column, if any — `None` where gathering keeps none in that
    /// order. PostgreSQL's reads the primary set where the register's order
    /// is exact; DataFusion's reads the second set where one is kept, and the
    /// primary set otherwise ([`Self::bounds_kinds`]): the set
    /// [`bounds_set_keyed_by`] finds for the kind a term compares by in
    /// `semantics`, of the sets gathered under this plan.
    pub fn bounds_in(&self, semantics: ComparisonSemantics) -> Option<BoundsSet> {
        bounds_set_keyed_by(&self.bounds_kinds(), &self.bounds_read_by(semantics)?)
    }

    /// The kind a term reads bounds by in `semantics` — the kind it compares
    /// by — or `None` where that semantics believes no bounds for a column of
    /// this plan: PostgreSQL's wherever the register's order is not exact, and
    /// either for a nested or unanswerable plan. Which stored set it reads is
    /// the block's to say ([`bounds_set_keyed_by`]), gathering having
    /// resolved the column under its own schema.
    pub fn bounds_read_by(&self, semantics: ComparisonSemantics) -> Option<CompareKind> {
        match (self, semantics) {
            (Self::Compared { kind, divergence: None }, ComparisonSemantics::Postgres) => {
                Some(kind.clone())
            }
            (Self::Compared { kind, .. }, ComparisonSemantics::DataFusion) => {
                Some(kind.datafusion_order())
            }
            (Self::Refused, ComparisonSemantics::DataFusion) => {
                Some(CompareKind::Text { length: None })
            }
            _ => None,
        }
    }

    /// Whether gathering stores bounds and row order for a column of this plan
    /// ordered as `semantics` orders the column ([`Self::bounds_in`]).
    pub fn bounds_ordered_in(&self, semantics: ComparisonSemantics) -> bool {
        self.bounds_in(semantics).is_some()
    }

    /// Whether the dictionary gathering stores for a column of this plan
    /// answers `semantics`' equality — `false` where gathering keeps none.
    /// It is kept where the register's `=` is exact (D79), and an entry is
    /// the field's own text, which a term answers as it answers a row holding
    /// it, whatever it compares by; `character(n)` alone stores its entries
    /// unpadded, which is its `=` and not Arrow's.
    pub fn dictionary_answers_in(&self, semantics: ComparisonSemantics) -> bool {
        match self {
            Self::Compared { kind, divergence } => {
                divergence.is_none_or(|d| !d.affects_equality())
                    && (semantics == ComparisonSemantics::Postgres
                        || !matches!(kind, CompareKind::PaddedText { .. }))
            }
            _ => false,
        }
    }

    /// The order PostgreSQL itself defines for this type.
    fn agrees(kind: CompareKind) -> Self {
        Self::Compared { kind, divergence: None }
    }

    /// Bytewise over the text the file holds, for the one type PostgreSQL
    /// orders not at all. A collatable type goes through [`collated_text`]
    /// instead and every other `Utf8View` type carries a comparison of its
    /// own, so this constant has exactly one member: `json`.
    ///
    /// It is a *stronger* answer than the server's rather than a weaker one,
    /// which is why it is not a deficiency: there is no order to disagree
    /// with. The note it produces says so.
    pub(crate) const AS_TEXT: Self = Self::Compared {
        kind: CompareKind::Text { length: None },
        divergence: Some(ComparisonDivergence::AsText),
    };

    /// PostgreSQL's own comparison but for the residue `divergence` names —
    /// the shape a type takes when its order is implemented and one part of
    /// it rests on a fact the file does not carry.
    pub(crate) fn diverging(kind: CompareKind, divergence: ComparisonDivergence) -> Self {
        Self::Compared { kind, divergence: Some(divergence) }
    }
}

/// Split a `COLLATE` reference — `pg_catalog."C"`, `public.mycoll` — into its
/// optional schema and its name, unquoted. Unquoted identifiers fold to lower
/// case, exactly as the server folds them, so an unquoted `COLLATE C` is the
/// collation `c` and not the built-in `"C"`.
fn collation_parts(reference: &str) -> Option<(Option<String>, String)> {
    let mut cur = Cursor::new(reference.trim().as_bytes());
    cur.skip_spaces();
    let first = cur.parse_ident()?;
    if cur.eat_byte(b'.') {
        return Some((Some(first), cur.parse_ident()?));
    }
    Some((None, first))
}

/// Whether a stated collation orders bytewise — i.e. is `pg_catalog."C"` or
/// `pg_catalog."POSIX"`, the two PostgreSQL defines as `memcmp` on every
/// server and under every libc.
///
/// **The schema is checked, not just the name**: a collation called `"C"` in
/// another schema is not it, and answering "agrees" for one is the one
/// direction of error this register must not make. An unqualified `"C"` is
/// `pg_catalog`'s under the default search path, where `pg_catalog` is
/// searched first — and not bytewise where `collations` declares one of that
/// name elsewhere, which a path naming `pg_catalog` after its schema would
/// put ahead of it (I8).
///
/// Deficiency register: `deficiency: KD48` — a file's `SET search_path` is
/// not read, so an unqualified name resolves as under the default path: a
/// collation a declared one could shadow answers the weaker verdict here, and
/// a type is looked up by its schema-qualified name alone, so an unqualified
/// user type resolves `Unknown` and a built-in's name keeps the built-in
/// though a declared type could shadow it (D37). `pg_dump` qualifies every
/// collation and every name outside `pg_catalog` and empties the path (I8),
/// so only a hand-written file reaches it. **(c) unowned**; promoted by such
/// a file in hand, the fix being to model the path the file sets.
fn collation_is_bytewise(reference: &str, collations: &[CollationDef]) -> bool {
    let Some((schema, name)) = collation_parts(reference) else { return false };
    if name != "C" && name != "POSIX" {
        return false;
    }
    match schema {
        Some(schema) => schema == "pg_catalog",
        None => !collations.iter().any(|declared| {
            collation_parts(&declared.name).is_some_and(|(_, declared_name)| declared_name == name)
        }),
    }
}

/// The comparison for a collatable type, given its bytewise comparison
/// `kind`, the column's own `COLLATE` clause (`None` for a column that
/// carries none) and the type's default collation.
///
/// **`kind` is the comparison in every case; only the verdict moves.** It is
/// a parameter because `character(n)` joins this rule with a comparison of
/// its own: `bpcharcmp` trims both operands' trailing blanks and *then*
/// consults the collation (I38), so the trim is orthogonal to the clause.
fn collated_text(
    kind: CompareKind,
    collation: Option<&str>,
    type_default: TypeCollation,
    collations: &[CollationDef],
) -> ComparisonPlan {
    // First, because it is the strongest thing the file can say about a
    // collation: the other three branches read a *name*, this one a statement
    // the dump wrote. The two can never both match — a dump declares nothing
    // in `pg_catalog` (I42), and an unqualified name a declared collation
    // shares is not taken for the built-in — so the order is evidence, not
    // precedence.
    if collation.is_some_and(|reference| states_non_deterministic(reference, collations)) {
        return ComparisonPlan::diverging(kind, ComparisonDivergence::NonDeterministicCollation);
    }
    let bytewise = match collation {
        Some(reference) => collation_is_bytewise(reference, collations),
        None => type_default == TypeCollation::Bytewise,
    };
    if bytewise {
        return ComparisonPlan::agrees(kind);
    }
    ComparisonPlan::diverging(
        kind,
        match collation {
            Some(_) => ComparisonDivergence::NonBytewiseCollation,
            None => ComparisonDivergence::UnknownCollation,
        },
    )
}

/// Whether `reference` — a column's `COLLATE` clause, verbatim — names a
/// collation this dump declared `deterministic = false` (I42).
///
/// **Both sides are parsed rather than compared as text**: the two spellings
/// come from different `pg_dump` code paths and either may quote an
/// identifier the other leaves bare. [`collation_parts`] folds an unquoted
/// identifier the way the server does.
///
/// **A reference with no schema matches on the name alone.** `pg_dump` writes
/// both sides schema-qualified, so the case is reachable only from a
/// hand-written file, whose search path is not read (`KD48`). Matching is
/// the announcing direction — a spurious note over correct rows, never a
/// silent wrong row set.
fn states_non_deterministic(reference: &str, collations: &[CollationDef]) -> bool {
    let Some((schema, name)) = collation_parts(reference) else { return false };
    collations.iter().any(|declared| {
        !declared.deterministic
            && collation_parts(&declared.name).is_some_and(|(declared_schema, declared_name)| {
                declared_name == name
                    && schema.as_ref().is_none_or(|s| Some(s) == declared_schema.as_ref())
            })
    })
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
fn builtin_name(
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
fn builtin_scalar(
    base: &str,
    typmod: Option<&str>,
    qualifier: Option<IntervalQualifier>,
    collation: Option<&str>,
    collations: &[CollationDef],
) -> Option<(DataType, ComparisonPlan)> {
    use CompareKind as K;
    use DataType::*;
    use arrow::datatypes::IntervalUnit::MonthDayNano;
    use arrow::datatypes::TimeUnit::Microsecond;
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
struct BuiltinRange {
    /// The bound type, spelled as DDL spells it.
    subtype: &'static str,
    /// Whether the name was the multirange half of the pair.
    multi: bool,
    /// The canonical function values of this type are rewritten through on
    /// the way in, where there is one — the half of I46 the comparison
    /// reproduces. A fact about the *range type*, not about the subtype.
    discrete: Option<Discrete>,
}

/// The definition of one of PostgreSQL's twelve built-in range/multirange
/// types. `None` for anything else.
///
/// **Hardcoded because the catalog holds it and the DDL does not** (I10):
/// `TypeKind::Range::subtype` is populated only for a user-defined range. The
/// six multirange names carry the *same* subtypes as their range counterparts
/// and a different literal form, so they are told apart here.
fn builtin_range_subtype(name: &str) -> Option<BuiltinRange> {
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
fn find_type<'a>(reference: &str, types: &'a [TypeDef]) -> Option<&'a TypeDef> {
    let key = canonical_type_name(reference)?;
    types.iter().find(|t| canonical_type_name(&t.name).is_some_and(|name| name == key))
}

/// The range whose `multirange_type_name` is `reference`, compared as
/// [`find_type`] compares — the only trace a multirange companion has (I10).
fn companion_range<'a>(reference: &str, types: &'a [TypeDef]) -> Option<&'a TypeDef> {
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
#[derive(Debug, Clone, Copy)]
struct Visits(usize);

impl Visits {
    /// The allowance for one walk over `types`.
    fn over(types: &[TypeDef]) -> Self {
        Self(types.len())
    }

    /// One definition visited: what is left, or `None` where nothing was.
    fn spend(self) -> Option<Self> {
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
fn array_element(declared: &str) -> Option<&str> {
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
/// [`resolve_array`] and [`array_comparison`] test this terminal rather than
/// the declared spelling (I22, I26; `docs/design/decisions.md`, "D41").
///
/// The loop is bounded by the type list's length — a domain chain visits each
/// `CREATE DOMAIN` at most once — which keeps a hand-edited file from
/// spinning rather than merely failing.
fn domain_terminal<'a>(name: &'a str, types: &'a [TypeDef]) -> &'a str {
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
fn is_box(terminal: &str) -> bool {
    !split_typmod(terminal).0.contains('.') && builtin_name(terminal).0 == "box"
}

/// The input grammar of a built-in this build holds as its text and orders by
/// nothing ([`ComparisonPlan::Refused`]), or of an enum whose labels are
/// exactly none, which a strict parse reads a field
/// of it by all the same: a check needs the grammar alone, where an order
/// would need a comparison register row of its own (D40).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextGrammar {
    /// `bit_in`, or `varbit_in` where `varying`, handed the column's typmod
    /// (I73): `length` is the length a `bit(n)` field must have exactly and a
    /// `bit varying(n)` field may not pass, and `None` for `"bit"` and a bare
    /// `bit varying`, which bound no length.
    Bit { varying: bool, length: Option<u32> },
    /// One of the seven geometric types' input functions (I74).
    Geometric(Geometric),
    /// An array of `box`, which `array_in` splits at `box`'s `;` (I22) and
    /// whose elements `box_in` reads: the one array a column holds that no
    /// nested plan reads, its element type being opaque here. `not_null`
    /// is its element's, a `NOT NULL` domain over `box` refusing a NULL
    /// element ([`Position`]).
    BoxArray { not_null: bool },
    /// A type whose input function refuses every value — the planner's and
    /// the extended statistics' internal types and `gtsvector` (I78), and an
    /// enum declaring no labels, which `enum_in` has none to read (I70) — so
    /// every field of it is refused.
    RefusesAll,
    /// A type whose input function refuses nothing at some supported major —
    /// `"char"`, `refcursor`, and `xid`, `xid8` and `cid`, which v16 narrowed
    /// (I79) — so a field of it is read and refused by nothing, as one is
    /// refused only where every supported major refuses it.
    RefusesNothing,
}

/// A geometric type, naming the input function a field of it is read by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Geometric {
    Point,
    Line,
    Lseg,
    Box,
    Path,
    Polygon,
    Circle,
}

/// The [`TextGrammar`] a field of a column declared `declared` is read by,
/// through any chain of domains, a domain handing its own typmod to its base
/// type's input function — `None` for a type with none here, an array among
/// them, whose elements a nested plan reads, but an array of `box`.
///
/// **`bit` alone is `bit(1)`**, the SQL word's length, and `"bit"` the catalog
/// type with no typmod, which is how `format_type` writes a `bit` column of
/// either (I8). A geometric type takes no typmod, so one written with one is
/// DDL no server holds, read as no grammar.
pub(crate) fn text_grammar(declared: &str, types: &[TypeDef]) -> Option<TextGrammar> {
    let terminal = domain_terminal(declared, types);
    let (base, _) = split_typmod(terminal);
    if let Some(element) = array_element(terminal) {
        return is_box(domain_terminal(element, types))
            .then(|| TextGrammar::BoxArray { not_null: domain_not_null(element, types) });
    }
    if base.contains('.') {
        // pg-refuses: I70 — an enum whose labels are exactly none.
        let labelless = matches!(
            find_type(base, types).map(|t| &t.kind),
            Some(TypeKind::Enum { labels, exact: true }) if labels.is_empty()
        );
        return labelless.then_some(TextGrammar::RefusesAll);
    }
    let (name, typmod, _) = builtin_name(terminal);
    // None of these takes a typmod, so one written with one is DDL no server
    // holds, read as no grammar.
    match &*name {
        // pg-refuses: I78 — an input function refusing every value.
        "pg_node_tree"
        | "pg_ndistinct"
        | "pg_dependencies"
        | "pg_mcv_list"
        | "pg_brin_bloom_summary"
        | "pg_brin_minmax_multi_summary"
        | "gtsvector" => return typmod.is_none().then_some(TextGrammar::RefusesAll),
        // `"char"` alone reaches here quoted, bare `char` being `character`.
        "char" | "refcursor" | "xid" | "xid8" | "cid" => {
            return typmod.is_none().then_some(TextGrammar::RefusesNothing);
        }
        _ => {}
    }
    let geometric = match &*name {
        "point" => Some(Geometric::Point),
        "line" => Some(Geometric::Line),
        "lseg" => Some(Geometric::Lseg),
        "box" => Some(Geometric::Box),
        "path" => Some(Geometric::Path),
        "polygon" => Some(Geometric::Polygon),
        "circle" => Some(Geometric::Circle),
        _ => None,
    };
    if let Some(geometric) = geometric {
        return typmod.is_none().then_some(TextGrammar::Geometric(geometric));
    }
    let varying = match &*name {
        "bit" => false,
        "bit varying" => true,
        _ => return None,
    };
    let length = match typmod {
        // `anybit_typmodin` takes one length of at least 1; a zero or
        // non-integer typmod is DDL no server holds, read as no grammar.
        Some(typmod) => Some(typmod.trim().parse::<u32>().ok().filter(|&n| n > 0)?),
        None if !varying && !base.starts_with('"') => Some(1),
        None => None,
    };
    Some(TextGrammar::Bit { varying, length })
}

/// The grammar a strict parse reads a field of a column compared by `plan`
/// and declared `declared` by in place of `plan`: its [`text_grammar`] where
/// `plan` orders it by nothing, and `None` elsewhere.
pub(crate) fn column_grammar(
    plan: &ComparisonPlan,
    declared: &str,
    types: &[TypeDef],
) -> Option<TextGrammar> {
    match plan {
        ComparisonPlan::Refused => text_grammar(declared, types),
        _ => None,
    }
}

/// **Why a strict parse checks nothing of a value, or of a position within
/// one**: what PostgreSQL refuses there is decided by something besides the
/// field and its column's declaration, or this build reads no input of its
/// type ([`unchecked_positions`]).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Unchecked {
    /// The column is declared nowhere in the dump, so read as no type.
    Undeclared,
    /// `xml`, which `xml_in` parses by the restoring server's libxml under
    /// its `xmloption`.
    Xml,
    /// `money`, which `cash_in` reads by the restoring server's
    /// `lc_monetary`, a setting `pg_dump` writes nowhere (`KD13`).
    Money,
    /// A user base type, read by an input function only the restoring server
    /// runs.
    BaseType,
    /// A range declaring its own `canonical` function, which only the
    /// restoring server runs (I46).
    RangeCanonical { function: String },
    /// A `reg*` type or `aclitem`, whose input function looks the name it
    /// reads up in the restoring server's catalog.
    Catalog,
    /// A type the dump does not declare, as an extension's is not, read by
    /// whatever the restoring server holds under its name.
    UndeclaredType,
    /// A built-in type this build has no reader of — `tsvector`, `tsquery`,
    /// `pg_lsn`, `jsonpath`, `tid`, `oidvector`, `pg_snapshot`,
    /// `txid_snapshot` — outside a strict parse's promise by scope
    /// (`docs/design/roadmap.md`, "A literal is guaranteed in `*_out`'s form
    /// and never read past `*_in`'s").
    NoReader,
    /// A type whose declaration this build did not read in full — a
    /// composite whose fields, or a range whose subtype, the preamble did not
    /// parse, a built-in read by a grammar here but for a typmod no server
    /// holds, or a domain whose chain never ends — so it reads the field as
    /// its text.
    Unparsed,
    /// An array whose element type is itself an array, which this build
    /// reads as its text (`KD3`).
    ArrayShape,
    /// An enum whose labels the preamble does not hold exactly, so a label
    /// it lacks is not refused (I70).
    LabelsInexact,
    /// A domain declaring a `CHECK`, an expression `domain_in` evaluates
    /// (I77).
    DomainCheck,
}

impl Unchecked {
    /// The reason in words, for a listing.
    pub fn describe(&self) -> String {
        match self {
            Self::Undeclared => "declared nowhere in the dump".to_string(),
            Self::Xml => "`xml_in` parses it by the restoring server's libxml and `xmloption`"
                .to_string(),
            Self::Money => "`cash_in` reads it by the restoring server's `lc_monetary`".to_string(),
            Self::BaseType => {
                "a base type, read by an input function only the restoring server runs".to_string()
            }
            Self::RangeCanonical { function } => {
                format!("a range whose canonical function, {function}, only the restoring server runs")
            }
            Self::Catalog => "read against the restoring server's catalog".to_string(),
            Self::UndeclaredType => {
                "a type the dump does not declare, read by the restoring server".to_string()
            }
            Self::NoReader => "a built-in type pgdt has no reader of".to_string(),
            Self::Unparsed => "a type whose declaration pgdt did not read in full".to_string(),
            Self::ArrayShape => "an array of arrays, which pgdt reads as its text".to_string(),
            Self::LabelsInexact => {
                "an enum whose labels pgdt does not hold exactly, so a label it lacks is not refused"
                    .to_string()
            }
            Self::DomainCheck => "the domain's CHECK, an expression".to_string(),
        }
    }
}

/// **Every position of a column declared `declared` and compared by `plan`
/// that a strict parse checks nothing of**, as `(path, declared type there,
/// why)`: first each a field is read at by no grammar — the column itself
/// where `plan` orders by nothing and `grammar` is `None`, or is
/// unanswerable, a position
/// of a nested `plan` holding a type it orders by nothing and has no grammar
/// for, an enum whose labels are not exact — in walk order, then each domain
/// at or beneath the column declaring a `CHECK`. A path is spelled as [`NestedCompare::uncomparable`]
/// spells one, `""` naming the column.
///
/// `grammar` is [`column_grammar`]'s, which a strict parse reads by, so what
/// this names and what the parse checks are one reading of one plan.
pub(crate) fn unchecked_positions(
    declared: &str,
    plan: &ComparisonPlan,
    grammar: Option<TextGrammar>,
    types: &[TypeDef],
) -> Vec<(String, String, Unchecked)> {
    let mut out = Vec::new();
    match plan {
        ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
            range_type,
            function,
        }) => {
            out.push((
                String::new(),
                range_type.clone(),
                Unchecked::RangeCanonical { function: function.clone() },
            ));
        }
        ComparisonPlan::Refused if grammar.is_none() => {
            out.push((String::new(), declared.trim().to_string(), unread(declared, types)));
        }
        ComparisonPlan::Compared { kind: CompareKind::Enum { exact: false, .. }, .. } => {
            out.push((String::new(), declared.trim().to_string(), Unchecked::LabelsInexact));
        }
        ComparisonPlan::Nested(nested) => nested.walk(&mut String::new(), &mut |path, leaf| {
            let why = match leaf {
                NestedCompare::Leaf { kind: CompareKind::Enum { exact: false, .. }, .. } => {
                    Unchecked::LabelsInexact
                }
                // A position holding an array is an array's element, every
                // other array being a node of its own.
                NestedCompare::Uncomparable {
                    divergence: None, grammar: None, declared, ..
                } if array_element(domain_terminal(declared, types)).is_some() => {
                    Unchecked::ArrayShape
                }
                NestedCompare::Uncomparable {
                    divergence: None, grammar: None, declared, ..
                } => unread(declared, types),
                _ => return,
            };
            out.push((path.to_string(), leaf.declared().to_string(), why));
        }),
        _ => {}
    }
    domains_beneath(declared, types, &mut String::new(), Visits::over(types), &mut out);
    out
}

/// Why a field of `declared`, read by no grammar, is checked for nothing: its
/// terminal type's input is decided by the restoring server, or this build
/// reads none of it. An array is its element's reason, but an array of arrays.
fn unread(declared: &str, types: &[TypeDef]) -> Unchecked {
    let terminal = domain_terminal(declared, types);
    if let Some(element) = array_element(terminal) {
        if array_element(domain_terminal(element, types)).is_some() {
            return Unchecked::ArrayShape;
        }
        return unread(element, types);
    }
    let (base, _) = split_typmod(terminal);
    if base.contains('.') {
        return match find_type(base, types).map(|t| &t.kind) {
            // A shell is completed only as a base type.
            Some(TypeKind::Base { .. } | TypeKind::Shell) => Unchecked::BaseType,
            Some(TypeKind::Range { canonical: Some(function), .. }) => {
                Unchecked::RangeCanonical { function: function.clone() }
            }
            Some(TypeKind::Enum { exact: false, .. }) => Unchecked::LabelsInexact,
            None if companion_range(base, types).is_none() => Unchecked::UndeclaredType,
            // A composite or range read short, its multirange companion, and
            // a domain whose chain never ends.
            _ => Unchecked::Unparsed,
        };
    }
    if text_grammar(base, types).is_some() {
        return Unchecked::Unparsed;
    }
    match &*builtin_name(terminal).0 {
        "xml" => Unchecked::Xml,
        "money" => Unchecked::Money,
        "aclitem" | "regclass" | "regcollation" | "regconfig" | "regdictionary"
        | "regnamespace" | "regoper" | "regoperator" | "regproc" | "regprocedure" | "regrole"
        | "regtype" => Unchecked::Catalog,
        "tsvector" | "tsquery" | "pg_lsn" | "jsonpath" | "tid" | "oidvector" | "pg_snapshot"
        | "txid_snapshot" => Unchecked::NoReader,
        // A name `pg_catalog` holds no type under, as a hand-written dump may
        // write a type its search path finds.
        _ => Unchecked::UndeclaredType,
    }
}

/// Each domain at or beneath `declared` declaring a `CHECK`, appended to
/// `out` with `path`, walking domains, array elements, composite fields and
/// range bounds as [`resolve_declared_type`] walks them and spending `visits`
/// as it does.
fn domains_beneath(
    declared: &str,
    types: &[TypeDef],
    path: &mut String,
    visits: Visits,
    out: &mut Vec<(String, String, Unchecked)>,
) {
    let declared = declared.trim();
    let descend = |into: &str, step: &str, path: &mut String, visits, out: &mut Vec<_>| {
        let len = path.len();
        path.push_str(step);
        domains_beneath(into, types, path, visits, out);
        path.truncate(len);
    };
    if let Some(element) = array_element(declared) {
        return descend(element, "[]", path, visits, out);
    }
    let (base, _) = split_typmod(declared);
    if !base.contains('.') {
        return;
    }
    let Some(visits) = visits.spend() else { return };
    let Some(def) = find_type(base, types) else {
        if let Some(TypeDef { kind: TypeKind::Range { subtype: Some(subtype), .. }, .. }) =
            companion_range(base, types)
        {
            descend(subtype, "[].bound", path, visits, out);
        }
        return;
    };
    match &def.kind {
        TypeKind::Domain { base_type, check, .. } => {
            if *check {
                out.push((path.clone(), def.name.clone(), Unchecked::DomainCheck));
            }
            descend(base_type, "", path, visits, out);
        }
        TypeKind::Composite { fields: Some(fields) } => {
            for field in fields {
                descend(&field.declared_type, &format!(".{}", field.name), path, visits, out);
            }
        }
        TypeKind::Range { subtype: Some(subtype), .. } => {
            descend(subtype, ".bound", path, visits, out);
        }
        _ => {}
    }
}

/// Map one declared type string — exactly as `pg_dump` wrote it, e.g. from
/// [`crate::preamble::DatabaseMetadata::tables`] — against `types`, that
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

/// The comparison for an array column, from the same walk [`resolve_array`]
/// makes and with the same two refusals: an element type that is opaque by
/// construction (I22), and one that is itself an array (I26). Both resolve the
/// *column* to `Utf8View` with its plan refused (`crate::resolve`), so a
/// column of either never reaches this plan — but the register is asked directly too, and must not
/// answer "ordered" for a column the resolver declines.
///
/// The column's own `COLLATE` clause is passed **down to the element**: an
/// array type is not collatable, and `pg_dump` writes the clause on a `text[]`
/// column to state the collation its *elements* are compared under (I37).
fn array_comparison(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let Some(element) = array_element(declared) else {
        return ComparisonPlan::Refused;
    };
    let terminal = domain_terminal(element, types);
    let opaque = is_box(terminal)
        || matches!(
            find_type(terminal, types).map(|t| &t.kind),
            Some(TypeKind::Base { .. } | TypeKind::Shell)
        );
    let child = if opaque || array_element(terminal).is_some() {
        // No divergence: a position only this build declines makes no claim
        // about the server's `=` (I22, I26). A column of either shape resolves
        // to text with its plan refused, so never reaches this node; a
        // composite's field of either shape does, and its array is split at
        // its element type's `typdelim`. A `box` element carries `box_in`'s
        // grammar.
        NestedCompare::Uncomparable {
            declared: element.to_string(),
            divergence: None,
            grammar: text_grammar(element, types).filter(|_| is_box(terminal)),
            delimiter: typdelim(element, types),
        }
    } else {
        match nested_position(element, collation, types, collations, visits) {
            Ok(child) => child,
            Err(reason) => return ComparisonPlan::Unanswerable(reason),
        }
    };
    ComparisonPlan::Nested(NestedCompare::Array(Box::new(Position::of(child, element, types))))
}

/// The comparison for a range or multirange column, from its bound type and
/// whether the range type canonicalizes.
///
/// **The bound is asked with no `COLLATE` clause**, which is the one place
/// this walk knowingly answers weaker than the file allows: a range type
/// carries its own `collation` parameter and the preamble grammar keeps only
/// `subtype`, `multirange_type_name` and `canonical` (I10), so a `text`-bounded range
/// reaches [`collated_text`]'s no-clause arm. That is the conservative
/// direction — reading the parameter would move the verdict and never the
/// answer, so it is a property here rather than a deficiency.
///
/// A range whose DDL stated no subtype at all is [`ComparisonPlan::Refused`]
/// outright rather than a tree with an unnameable position in it: there is no
/// declared type to put in the refusal.
fn range_comparison(
    subtype: Option<&str>,
    discrete: Option<Discrete>,
    multi: bool,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let Some(subtype) = subtype else { return ComparisonPlan::Refused };
    let bound = match nested_position(subtype, None, types, collations, visits) {
        Ok(bound) => Box::new(bound),
        Err(reason) => return ComparisonPlan::Unanswerable(reason),
    };
    ComparisonPlan::Nested(if multi {
        NestedCompare::Multirange { bound, discrete }
    } else {
        NestedCompare::Range { bound, discrete }
    })
}

/// The register's answer for a column whose type is, or contains, the range
/// type `range_type`, whose DDL declares the canonical function `function`.
///
/// **Knowing the function exists licenses declining the column, never
/// reproducing it** (I46): PostgreSQL rewrites every value through arbitrary
/// server-side code before storing or comparing it, so both operator families
/// are wrong rather than weak — `[1,10] = [1,11)` is true on such a server
/// and false under a bytewise comparison of the two `range_out` strings. That
/// is why the answer is [`ComparisonPlan::Unanswerable`] and not
/// [`ComparisonPlan::Refused`].
fn unanswerable_range(range_type: &str, function: &str) -> ComparisonPlan {
    ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
        range_type: range_type.to_string(),
        function: function.to_string(),
    })
}

/// One position *inside* a nested type — an array's element, a composite's
/// field — asked the same question the column was, so nesting composes and a
/// domain beneath a container bottoms out where a domain always does.
///
/// The two answers that are not a comparison collapse to
/// [`NestedCompare::Uncomparable`], carrying the divergence rather than
/// dropping it: `json` has no comparison at all inside a container, so the
/// `=` the column still falls back to is an answer PostgreSQL does not have,
/// where [`ComparisonPlan::Refused`] says only that *this build* has no order
/// for the position and makes no claim about the server's equality (`KD10`
/// one level down is announced off the column's own resolution instead).
///
/// **[`ComparisonPlan::Unanswerable`] is the one answer a position cannot
/// hold**, and it is why this returns a `Result`: "no equality either" is a
/// fact about the whole column, not about the walk, so such a position
/// short-circuits out of the tree and becomes the column's own answer.
fn nested_position(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> Result<NestedCompare, UnanswerableReason> {
    Ok(match comparison_walk(declared, collation, types, collations, visits) {
        ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. } => {
            NestedCompare::Uncomparable {
                declared: declared.to_string(),
                divergence: Some(ComparisonDivergence::AsText),
                grammar: None,
                delimiter: typdelim(declared, types),
            }
        }
        ComparisonPlan::Refused => NestedCompare::Uncomparable {
            declared: declared.to_string(),
            divergence: None,
            grammar: text_grammar(declared, types),
            delimiter: typdelim(declared, types),
        },
        ComparisonPlan::Compared { kind, divergence } => {
            NestedCompare::Leaf { declared: declared.to_string(), kind, divergence }
        }
        ComparisonPlan::Nested(inner) => inner,
        ComparisonPlan::Unanswerable(reason) => return Err(reason),
    })
}

/// **The comparison register**: how a column declared `declared` compares, and
/// whether that is PostgreSQL's own order
/// (`docs/design/decisions.md`, "D40").
///
/// It walks the declared type exactly as [`resolve_declared_type`] does —
/// array first, then a schema-qualified name through the database's own
/// `CREATE TYPE`/`DOMAIN` list, then the built-in table — so the two answers are reached through one
/// spelling of the same string and a domain compares as whatever it bottoms
/// out at.
///
/// **A nested type answers [`ComparisonPlan::Nested`]**, one node per nesting
/// level, built by the same walk: an array's element, a composite's fields
/// and a range's bound are asked this same question in turn.
///
/// **`collation` is the column's own `COLLATE` clause**, verbatim as the DDL
/// wrote it ([`crate::preamble::ColumnDef::collation`]), or `None` where the
/// column carries none — which `pg_dump` writes exactly when the column's
/// collation is its type's default (I37). It is why the register is keyed per
/// *column*: two `text` columns of one table can compare differently.
pub fn comparison_for(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
) -> ComparisonPlan {
    comparison_walk(declared, collation, types, collations, Visits::over(types))
}

/// [`comparison_for`], at one step of its walk.
fn comparison_walk(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let declared = declared.trim();
    if array_element(declared).is_some() {
        return array_comparison(declared, collation, types, collations, visits);
    }
    let (base, _) = split_typmod(declared);
    if base.contains('.') {
        return comparison_user_type(base, collation, types, collations, visits);
    }
    let (base, typmod, qualifier) = builtin_name(declared);
    if let Some((_, plan)) = builtin_scalar(&base, typmod, qualifier, collation, collations) {
        return plan;
    }
    // The twelve built-in range and multirange names, which reach no arm of
    // `builtin_scalar` and appear in no `CREATE TYPE` (I10) — the same fourth
    // step `map_builtin` takes, so the two walks agree.
    match builtin_range_subtype(&base) {
        Some(range) => range_comparison(
            Some(range.subtype),
            range.discrete,
            range.multi,
            types,
            collations,
            visits,
        ),
        None => ComparisonPlan::Refused,
    }
}

/// The user-defined half of [`comparison_for`], over the same `TypeKind` list
/// [`resolve_user_type`] reads.
///
/// **Exhaustive over `TypeKind` with no wildcard arm**, so a kind added to the
/// preamble grammar has to choose a comparison rather than inheriting one.
/// Every visit is spent from `visits`, as [`resolve_user_type`]'s is.
fn comparison_user_type(
    name: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
    visits: Visits,
) -> ComparisonPlan {
    let Some(visits) = visits.spend() else { return ComparisonPlan::Refused };
    // Absent from the list: either an unknown type or a range's multirange
    // companion, which `pg_dump` emits no `CREATE TYPE` for (I10). The
    // companion is found through the range whose DDL names it, exactly as
    // `resolve_user_type` finds it.
    let Some(def) = find_type(name, types) else {
        return match companion_range(name, types) {
            // A multirange over a range that canonicalizes is unanswerable
            // for the same reason the range is, and names the *range* type:
            // the companion has no DDL of its own to name (I10).
            Some(TypeDef { name, kind: TypeKind::Range { subtype, canonical, .. } }) => {
                match canonical {
                    Some(function) => unanswerable_range(name, function),
                    None => {
                        range_comparison(subtype.as_deref(), None, true, types, collations, visits)
                    }
                }
            }
            _ => ComparisonPlan::Refused,
        };
    };
    match &def.kind {
        // An enum with no labels resolves to no Arrow type at all, so no
        // column of it is ever asked how it compares.
        TypeKind::Enum { labels, .. } if labels.is_empty() => ComparisonPlan::Refused,
        // PostgreSQL orders an enum by `pg_enum.enumsortorder`, assigned from
        // *declaration* order (I33) — so a label's position in this list is
        // the order and its text is not.
        TypeKind::Enum { labels, exact } => ComparisonPlan::agrees(CompareKind::Enum {
            labels: labels.iter().cloned().collect(),
            exact: *exact,
        }),
        // A domain compares as what it bottoms out at, through any chain —
        // the same recursion `resolve_declared_type` makes, bounded the same
        // way. The collation walks down with it and the *column's* clause
        // wins: a domain's own `COLLATE` is its type default, which a
        // column-level clause only overrides (I37).
        TypeKind::Domain { base_type, collation: domain_collation, .. } => comparison_walk(
            base_type,
            collation.or(domain_collation.as_deref()),
            types,
            collations,
            visits,
        ),
        // Field-wise in declaration order, which is `record_cmp`'s rule and
        // the order `record_out` writes them in (I23). A field list the
        // grammar could not read is all-or-nothing: a composite parsed short
        // would compare field 3's text as field 2's type.
        TypeKind::Composite { fields } => match fields {
            Some(fields) => {
                let positions: Result<Vec<_>, _> = fields
                    .iter()
                    .map(|f| {
                        // A composite is not collatable, so the *column's*
                        // clause cannot reach a field; each attribute
                        // carries its own (I37).
                        nested_position(
                            &f.declared_type,
                            f.collation.as_deref(),
                            types,
                            collations,
                            visits,
                        )
                        .map(|plan| (f.name.clone(), Position::of(plan, &f.declared_type, types)))
                    })
                    .collect();
                match positions {
                    Ok(positions) => ComparisonPlan::Nested(NestedCompare::Record(positions)),
                    Err(reason) => ComparisonPlan::Unanswerable(reason),
                }
            }
            None => ComparisonPlan::Refused,
        },
        // Bound-wise, with no canonicalization — the whole answer only
        // because the range declares no `canonical` function. Where it
        // declares one the column is unanswerable instead (I46);
        // `fixtures/*`'s `public.myrange` and `public.textrange` declare
        // none.
        TypeKind::Range { subtype, canonical, .. } => match canonical {
            Some(function) => unanswerable_range(&def.name, function),
            None => range_comparison(subtype.as_deref(), None, false, types, collations, visits),
        },
        TypeKind::Base { .. } | TypeKind::Shell => ComparisonPlan::Refused,
    }
}

#[cfg(test)]
mod tests;
