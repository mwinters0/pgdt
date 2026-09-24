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
use crate::preamble::{CollationDef, TypeDef, TypeKind};
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
    /// "D39").
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
    /// A `CREATE TYPE ... AS ENUM ()` with no labels at all.
    EmptyEnum,
}

/// Which PostgreSQL literal form fills a resolved Arrow type, at every
/// position in it. The Arrow type alone cannot say
/// (`docs/design/decisions.md`, "D39"); it is a tree because the answer
/// differs per nesting level.
///
/// A `Scalar` leaf is anything [`crate::decode`] handles (`Utf8View`
/// included), which is where every branch bottoms out but at `Int2Vector`
/// and a composite of no fields, childless terminals of their own. `Serialize` so
/// `pgdt info --json` can export a resolved schema's plans structurally
/// (`docs/design/decisions.md`, "D67"); **not `Deserialize`, and never
/// persisted** — the cache holds what the dump said, never what we concluded
/// (`docs/design/decisions.md`, "D68").
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub enum NestedPlan {
    /// Filled by `crate::decode`'s per-type decoders, or held as text.
    #[default]
    Scalar,
    /// `array_out` → `List<child>`. Nested `Array`s are the multi-dimensional
    /// case and **only** that: the plan's depth is the dimensionality the
    /// column was resolved at, and a value that disagrees is a decode
    /// failure. An array whose element type is an array (I26) is refused at
    /// resolution as [`TypeOutcome::NestedArrayElement`], so a nested `Array`
    /// only ever comes from the shape census.
    Array(Box<NestedPlan>),
    /// `record_out` → `Struct<…>`, one plan per declared field, in
    /// declaration order.
    Record(Vec<NestedPlan>),
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

/// How one column's field text becomes a value two sides of a comparison can
/// be ordered by — the decoding half of a [`ComparisonPlan`], and the only
/// thing `crate::predicate` needs in order to read a side.
///
/// A small closed vocabulary rather than the Arrow type, which does not
/// correspond to it (`docs/design/decisions.md`, "D40").
///
/// **Not `Copy`**: several variants carry the column's own facts — an enum's
/// labels, a typmodded `numeric`'s scale, whether one held as text admits the
/// infinities — and the labels are not `Copy`.
/// The kind is cloned once, into the `OrderTerm` the block's resolution
/// builds, never on the per-row path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareKind {
    Bool,
    Int,
    /// An unsigned 32-bit integer — `oid`. Held apart from [`Self::Int`]
    /// because the two differ on a *literal* carrying a minus sign, which
    /// `oidin` wraps and this refuses; the values themselves order
    /// identically.
    UnsignedInt,
    Float32,
    Float64,
    Decimal(i8),
    Date,
    Time,
    Timestamp {
        with_tz: bool,
    },
    Uuid,
    Bytea,
    Text,
    /// `character(n)`: bytewise over the text the file holds, after **both**
    /// sides give up their trailing blanks. A dump writes every value padded
    /// to `n` and `bpcharcmp` calls `bcTruelen` on both operands before
    /// consulting a collation (I38), so the padding is not part of the value
    /// and the clause decides the verdict, never the comparison.
    PaddedText,
    /// Arbitrary-precision decimal read straight out of the text the file
    /// holds — a bare `numeric`, or one whose declared precision is past
    /// `Decimal256`'s 76 digits. There is no scale to carry both sides to, so
    /// the comparison normalizes rather than rescales: `1.5` and `1.50` are
    /// one value written two ways.
    ///
    /// `infinities` says whether `Infinity`/`-Infinity` are values of the
    /// column. Any typmod rejects an infinity (I34), so only the bare form
    /// admits the spelling.
    Numeric {
        infinities: bool,
    },
    /// An enum, compared by each label's position in the type's own
    /// declaration order (I33) rather than by its text. The labels are
    /// `TypeKind::Enum`'s, verbatim and in that order, which is what
    /// `CREATE TYPE … AS ENUM (…)` writes and what a `--binary-upgrade`
    /// dump's `ALTER TYPE … ADD VALUE` run is folded back into (I6).
    Enum(Arc<[String]>),
    /// `interval`, compared by `interval_cmp_value`'s span: months collapse
    /// to 30 days and days to 86400 seconds, so `1 mon`, `30 days` and
    /// `720:00:00` are one value written three ways (I40). The span needs 128
    /// bits.
    ///
    /// Carries the two infinities unconditionally, in `date_out`'s spellings
    /// rather than `numeric_out`'s (I34). They are v17 values, read on an
    /// older file under the union rule (I35).
    Interval,
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
    /// [`Self::arrow_order`].
    IntervalFields,
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
    Arrow,
}

impl CompareKind {
    /// The kind whose order over this kind's field text is
    /// [`ComparisonSemantics::Arrow`]'s order over the value its column emits.
    /// A kind whose key *is* the emitted value is its own. A float is its own
    /// too: with `-0` made `0`, IEEE `totalOrder` over what a dump holds — one
    /// `NaN`, which it writes as `NaN` — is `float8_cmp`'s. **Every kind
    /// emitted as text becomes [`Self::Text`]**, an enum (emitted
    /// `Dictionary`, compared by label text) and `character(n)` (emitted
    /// padded) among them, and `macaddr` too, though the file's text orders
    /// as its octets do (`docs/design/decisions.md`, "D40"). `interval`
    /// becomes its field-wise variant.
    ///
    /// A special value the emitted type cannot hold (`KD8`) keeps its rank in
    /// the key, which no Arrow value contradicts.
    pub fn arrow_order(&self) -> CompareKind {
        match self {
            Self::Interval => Self::IntervalFields,
            Self::Enum(_)
            | Self::Numeric { .. }
            | Self::TimeTz
            | Self::Network { .. }
            | Self::MacAddr { .. }
            | Self::Jsonb
            | Self::PaddedText => Self::Text,
            kind => kind.clone(),
        }
    }

    /// How [`Self::arrow_order`]'s comparison differs from this kind's —
    /// `None` exactly where it leaves the kind alone. A float inside a nested
    /// column differs besides ([`NestedCompare::arrow_divergences`]).
    pub fn arrow_divergence(&self) -> Option<ComparisonDivergence> {
        match self {
            Self::Enum(_) => Some(ComparisonDivergence::LabelText),
            Self::Numeric { .. }
            | Self::TimeTz
            | Self::Network { .. }
            | Self::MacAddr { .. }
            | Self::Jsonb => Some(ComparisonDivergence::ValueAsText),
            Self::Interval => Some(ComparisonDivergence::IntervalFields),
            Self::PaddedText => Some(ComparisonDivergence::PaddedText),
            _ => None,
        }
    }
}

/// What a position of `kind` diverges in under [`ComparisonSemantics::Arrow`],
/// given the register's own `divergence` for it: [`CompareKind::arrow_divergence`],
/// then the register's, but for `jsonb`'s string collation, the whole value
/// being compared as text there.
pub(crate) fn arrow_position_divergences(
    kind: &CompareKind,
    divergence: Option<ComparisonDivergence>,
) -> Vec<ComparisonDivergence> {
    kind.arrow_divergence()
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
    /// *more* than the server does rather than less. Every other text-held
    /// type carries a comparison of its own, and a collatable one either
    /// agrees or carries one of the three collation variants below.
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
    /// A column whose declared type resolved, is not nested, and has no
    /// comparison in this register at all — `box`, `money`, `xml`, a
    /// user-defined base type. An ordering operator is *refused* on such a
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
    /// An enum under [`ComparisonSemantics::Arrow`]: DataFusion compares the
    /// emitted `Dictionary` by its label text, where PostgreSQL orders labels
    /// by declaration (I33). Equality agrees, a label being unique.
    ///
    /// This and the five variants after it are Arrow semantics' own:
    /// produced by [`CompareKind::arrow_divergence`],
    /// [`NestedCompare::arrow_divergences`] and
    /// `crate::predicate::column_divergences`, never by the register — as
    /// [`CompareKind::IntervalFields`] is produced by
    /// [`CompareKind::arrow_order`] alone.
    LabelText,
    /// A type emitted as `Utf8View` whose PostgreSQL comparison is by value —
    /// a bare `numeric` or one past 76 digits, `timetz`, `inet`/`cidr`, `macaddr`/`macaddr8`,
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
    NestedArrowOrder,
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
            Self::LabelText | Self::NestedArrowOrder => false,
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
    Uncomparable { declared: String, divergence: Option<ComparisonDivergence> },
    /// `array_cmp`: elements first, up to the shorter array's length, then
    /// element count, dimension count, dimensions and lower bounds (I45).
    /// **One node whatever the dimensionality** — an `array_out` literal
    /// carries its own shape and [`crate::nested::ArrayLiteral`] flattens it,
    /// so `integer[]` is one `Array` node at any depth.
    Array(Box<NestedCompare>),
    /// `record_cmp`: field-wise in declaration order, which is also the order
    /// `record_out` writes them in. The name is carried for the diagnostic
    /// path alone — `record_out` is positional (I23) and no comparison reads
    /// it.
    Record(Vec<(String, NestedCompare)>),
    /// `range_cmp`: `empty` below every other value, then lower bound, then
    /// upper, with a bound settling infinity before value and value before
    /// inclusivity (I46). `bound` is the subtype's own node, so a range over
    /// a composite composes like any other position.
    ///
    /// **`discrete` is a property of the range type, never of its subtype**:
    /// only `int4range`, `int8range` and `daterange` carry a canonical
    /// function among the built-ins, so the flag is set from the range's
    /// *name*. A user-defined range never reaches this node with it set — one
    /// declaring a `canonical` function is [`ComparisonPlan::Unanswerable`]
    /// rather than a tree (I46).
    Range { bound: Box<NestedCompare>, discrete: bool },
    /// `multirange_cmp`: member-wise over members the server has already
    /// sorted, coalesced and emptied out, the shorter multirange first
    /// (I46). The fields are the *member range's*, since a multirange has no
    /// comparison of its own beyond the sequence.
    Multirange { bound: Box<NestedCompare>, discrete: bool },
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

impl NestedCompare {
    /// The first position beneath this one with no order, as
    /// `(path, declared type)` — `None` when every position is comparable.
    /// The path is the accessor a user would write where one exists: `[]` for
    /// an element, `.name` for a field, appended as the walk descends. A
    /// range's bound has no subscript spelling, so `.bound` names it and a
    /// multirange's is `[].bound`.
    pub fn uncomparable(&self) -> Option<(String, String)> {
        let mut found = None;
        self.walk(&mut String::new(), &mut |path, leaf| {
            if matches!(leaf, Self::Uncomparable { .. }) && found.is_none() {
                found = Some((path.to_string(), leaf.declared().to_string()));
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

    /// [`Self::divergences`] under [`ComparisonSemantics::Arrow`]: what each
    /// position's own comparison differs in from the server's when DataFusion
    /// compares the emitted value with `make_comparator`, in walk order. A
    /// leaf reports what a column of its kind would
    /// ([`arrow_position_divergences`]) plus [`ComparisonDivergence::UnnormalizedZero`]
    /// for a float, and an [`Self::Uncomparable`] position keeps what its
    /// bytewise `=` costs. The container's own order is the column's
    /// [`ComparisonDivergence::NestedArrowOrder`], not a position's.
    pub fn arrow_divergences(&self) -> Vec<(String, String, ComparisonDivergence)> {
        let mut out = Vec::new();
        self.walk(&mut String::new(), &mut |path, leaf| {
            let divergences = match leaf {
                Self::Leaf { kind, divergence, .. } => {
                    let mut found = arrow_position_divergences(kind, *divergence);
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
                element.walk(path, visit);
                path.truncate(len);
            }
            Self::Record(fields) => {
                for (name, field) in fields {
                    let len = path.len();
                    path.push('.');
                    path.push_str(name);
                    field.walk(path, visit);
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
/// text, and a text column's `Text` set is.
pub fn bounds_set_keyed_by(
    stored: &[Option<CompareKind>; 2],
    kind: &CompareKind,
) -> Option<BoundsSet> {
    let serves = |stored: &CompareKind| {
        stored == kind
            || (*kind == CompareKind::Text && matches!(stored, CompareKind::MacAddr { .. }))
    };
    [BoundsSet::Primary, BoundsSet::Arrow]
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
    /// which is PostgreSQL's, and otherwise Arrow's ([`CompareKind::arrow_order`]),
    /// which is always exact over the text the column emits: a divergent text
    /// column's bytewise order, `jsonb`'s and a `character(n)`'s off `C`
    /// (padded) as text, and a column with no plan bytewise, as Arrow
    /// semantics compares it. The Arrow set is a second one, for an exact kind
    /// whose Arrow order is another — but `macaddr`, whose field text orders
    /// as its octets do (I40), so its one set serves both.
    pub fn bounds_kinds(&self) -> [Option<CompareKind>; 2] {
        match self {
            Self::Compared { kind, divergence: None } => {
                let arrow = kind.arrow_order();
                let second = (arrow != *kind && !matches!(kind, CompareKind::MacAddr { .. }))
                    .then_some(arrow);
                [Some(kind.clone()), second]
            }
            Self::Compared { kind, divergence: Some(_) } => [Some(kind.arrow_order()), None],
            Self::Refused => [Some(CompareKind::Text), None],
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
    /// is exact; Arrow's reads the second set where one is kept, and the
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
            (Self::Compared { kind, .. }, ComparisonSemantics::Arrow) => Some(kind.arrow_order()),
            (Self::Refused, ComparisonSemantics::Arrow) => Some(CompareKind::Text),
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
                        || *kind != CompareKind::PaddedText)
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
    pub(crate) const AS_TEXT: Self =
        Self::Compared { kind: CompareKind::Text, divergence: Some(ComparisonDivergence::AsText) };

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
/// name elsewhere, which a path the file sets could put ahead of it.
///
/// Deficiency register: `deficiency: KD48` — a file's `SET search_path` is
/// not read, so an unqualified name resolves as under the default path: a
/// collation a declared one could shadow answers the weaker verdict here, and
/// a type is looked up by its schema-qualified name alone, so an unqualified
/// user type resolves `Unknown` and a built-in's name keeps the built-in
/// though a declared type could shadow it. `pg_dump` qualifies every name
/// outside `pg_catalog` and empties the path (I8), so only a hand-written file
/// reaches it. **(c) unowned**; promoted by such a file in hand, the fix being
/// to model the path the file sets.
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
/// by [`builtin_name`]. Only `numeric` and `float` read the typmod's *value*;
/// every other mapping below is `Microsecond`-precision or otherwise
/// typmod-independent.
pub(crate) fn split_typmod(s: &str) -> (&str, Option<&str>) {
    match s.find('(') {
        Some(i) if s.ends_with(')') => (s[..i].trim_end(), Some(&s[i + 1..s.len() - 1])),
        _ => (s, None),
    }
}

/// A built-in's declared spelling — no `.` in it (I8) — as the one name
/// [`builtin_scalar`] and [`builtin_range_subtype`] match on, and its typmod,
/// read the way PostgreSQL's grammar reads it (`docs/design/roadmap.md`, "The
/// input contract is valid PostgreSQL").
///
/// - **A typmod may sit mid-name**: `format_type` writes
///   `timestamp(3) with time zone`, `time(2) without time zone` and
///   `interval day to second(3)`, so the one parenthesized group is cut out
///   wherever it falls.
/// - **`interval`'s field qualifiers** restrict what a value may hold, never
///   how it is written, so `interval year to month` is `interval`.
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
fn builtin_name(declared: &str) -> (std::borrow::Cow<'static, str>, Option<&str>) {
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
        return (name.into(), typmod);
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
            return ((if real { "real" } else { "double precision" }).into(), None);
        }
        "char" | "nchar" | "national char" | "national character" => "character",
        "char varying"
        | "nchar varying"
        | "national char varying"
        | "national character varying" => "character varying",
        "timestamp" => "timestamp without time zone",
        "time" => "time without time zone",
        w if w.strip_prefix("interval ").is_some_and(|fields| {
            fields.split(' ').all(|f| {
                matches!(f, "year" | "month" | "day" | "hour" | "minute" | "second" | "to")
            })
        }) =>
        {
            "interval"
        }
        w => return (catalog_name(w).map_or_else(|| words.clone(), str::to_owned).into(), typmod),
    };
    (keyword.into(), typmod)
}

/// Each catalog type name whose SQL spelling is another, beside that
/// spelling. The right-hand names are the grammar's alone: `pg_catalog` holds
/// no type called `integer`.
const CATALOG_NAMES: [(&str, &str); 12] = [
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
];

/// A catalog type name whose SQL spelling is another, as that spelling —
/// `None` where the two agree or the name is not a built-in's.
fn catalog_name(name: &str) -> Option<&'static str> {
    CATALOG_NAMES.iter().find(|&&(catalog, _)| catalog == name).map(|&(_, sql)| sql)
}

/// `numeric(p,s)` -> `Decimal128`/`Decimal256` when `p` fits; bare `numeric`
/// or a precision beyond `Decimal256`'s 76-digit ceiling stays `Utf8View`,
/// same as arbitrary precision (I4: `NaN` is reachable through any numeric
/// column regardless, and is a decode-time concern, not a mapping one).
///
/// The `Utf8View` arms are still *ordered*: [`CompareKind::Numeric`]
/// normalizes the text the file holds and compares by value, which is
/// `cmp_var_common`'s order and insensitive to trailing zeros (I33). The
/// typed arms carry the column's own scale instead, so both sides of one are
/// unscaled integers.
///
/// **Only the bare form admits an infinity**: `apply_typmod_special` rejects
/// `±Infinity` under any typmod (I34), so accepting the spelling in a
/// *filter's literal* on the `p > 76` arm would accept a value the server
/// refuses.
fn map_numeric(typmod: Option<&str>) -> (DataType, ComparisonPlan) {
    let arbitrary = |infinities| ComparisonPlan::agrees(CompareKind::Numeric { infinities });
    let Some(typmod) = typmod else { return (DataType::Utf8View, arbitrary(true)) };
    let mut parts = typmod.split(',').map(str::trim);
    let precision: Option<u8> = parts.next().and_then(|p| p.parse().ok());
    let scale: i8 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let decimal = ComparisonPlan::agrees(CompareKind::Decimal(scale));
    match precision {
        Some(p) if p <= 38 => (DataType::Decimal128(p, scale), decimal),
        Some(p) if p <= 76 => (DataType::Decimal256(p, scale), decimal),
        _ => (DataType::Utf8View, arbitrary(false)),
    }
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
/// which wants the Arrow type alone — passes `None` and an empty list.
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
        "smallint" => (Int16, agrees(K::Int)),
        "integer" => (Int32, agrees(K::Int)),
        "bigint" => (Int64, agrees(K::Int)),
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
        "text" | "character varying" => {
            (Utf8View, collated_text(K::Text, collation, TypeCollation::Database, collations))
        }
        "name" => {
            (Utf8View, collated_text(K::Text, collation, TypeCollation::Bytewise, collations))
        }
        // The fourth collatable arm, with a comparison of its own: the dump
        // writes every `character(n)` value blank-padded to `n` and
        // `bpcharcmp` calls `bcTruelen` on both sides before consulting a
        // collation (I38). `K::PaddedText` strips the padding; what is left
        // is the `text` question.
        "character" => {
            (Utf8View, collated_text(K::PaddedText, collation, TypeCollation::Database, collations))
        }
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
        // The *comparison* is `interval_cmp_value`'s fused span (I40).
        "interval" => (Interval(MonthDayNano), agrees(K::Interval)),
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
        let TypeKind::Domain { base_type, .. } = &types.iter().find(|t| t.name == base)?.kind
        else {
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
    // No collation: an Arrow type never depends on one, and the comparison
    // half of the pair is discarded here.
    if let Some((mapped, _)) = builtin_scalar(base, typmod, None, &[]) {
        // The literal form is this walk's to say: a `smallint[]` column and
        // an `int2vector` one are both `List<Int16>` and are written in
        // different grammars (`docs/design/decisions.md`, "D39"). Exactly one
        // built-in is a container.
        let plan = match base {
            "int2vector" => NestedPlan::Int2Vector,
            _ => NestedPlan::Scalar,
        };
        // The pairing is checked rather than trusted: a built-in mapped to a
        // container type without a plan named above would fill it with a
        // scalar builder and decode every value as text.
        debug_assert_eq!(
            matches!(mapped, DataType::List(_) | DataType::Struct(_)),
            plan != NestedPlan::Scalar,
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
    /// Whether values of this type are rewritten into canonical form on the
    /// way in — `int4range_canonical` and its two siblings, the half of I46
    /// the comparison reproduces. A fact about the *range type*, not about
    /// the subtype.
    discrete: bool,
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
        "int4range" => ("integer", false, true),
        "int8range" => ("bigint", false, true),
        "numrange" => ("numeric", false, false),
        "tsrange" => ("timestamp without time zone", false, false),
        "tstzrange" => ("timestamp with time zone", false, false),
        "daterange" => ("date", false, true),
        "int4multirange" => ("integer", true, true),
        "int8multirange" => ("bigint", true, true),
        "nummultirange" => ("numeric", true, false),
        "tsmultirange" => ("timestamp without time zone", true, false),
        "tstzmultirange" => ("timestamp with time zone", true, false),
        "datemultirange" => ("date", true, true),
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

/// The user-defined half: look `name` up in `types` (already schema-qualified,
/// matching how [`crate::preamble::TypeDef::name`] is stored) and resolve by
/// kind. A domain recurses on its base type — legal to nest (a domain over a
/// domain) — and every visit is spent from `visits`, which bounds a cycle.
fn resolve_user_type(name: &str, types: &[TypeDef], visits: Visits) -> TypeOutcome {
    let Some(visits) = visits.spend() else { return TypeOutcome::Unknown };
    let Some(def) = types.iter().find(|t| t.name == name) else {
        // Not a type of its own — but it might be a range's auto-created
        // multirange companion, which `pg_dump` emits no `CREATE TYPE` for
        // (I10). Its only trace is the `multirange_type_name` parameter
        // inside the range's own DDL, which is also where the companion's
        // bound type comes from.
        let companion_of = types.iter().find(|t| {
            matches!(&t.kind, TypeKind::Range { multirange_type_name: Some(n), .. } if n == name)
        });
        return match companion_of.map(|t| &t.kind) {
            Some(TypeKind::Range { subtype, .. }) => {
                let (bound, plan) = range_bound(subtype.as_deref(), types, visits);
                TypeOutcome::Mapped(list_of(range_struct(bound)), NestedPlan::Multirange(plan))
            }
            _ => TypeOutcome::Unknown,
        };
    };
    match &def.kind {
        TypeKind::Enum { labels } if labels.is_empty() => TypeOutcome::EmptyEnum,
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
                plans.push(plan);
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
        TypeKind::Base | TypeKind::Shell => TypeOutcome::OpaqueBaseType,
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
/// its own; a user-defined base type sets its delimiter in DDL this build does
/// not read, so `TypeKind::Base`/`Shell` are refused wholesale. The
/// array-ness test reads the terminal through [`array_element`] rather than
/// looking for a trailing `[]`, because `CREATE DOMAIN d AS integer ARRAY` is
/// as legal as any other spelling (I28).
///
/// See [`TypeOutcome::OpaqueElementType`] and
/// [`TypeOutcome::NestedArrayElement`] for why each shape is refused.
fn resolve_array(element: &str, types: &[TypeDef], visits: Visits) -> TypeOutcome {
    let terminal = domain_terminal(element, types);
    let opaque = terminal.eq_ignore_ascii_case("box")
        || matches!(
            types.iter().find(|t| t.name == terminal).map(|t| &t.kind),
            Some(TypeKind::Base | TypeKind::Shell)
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
    TypeOutcome::Mapped(list_of(data_type), NestedPlan::Array(Box::new(plan)))
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
/// outside the quotes and answers the name with its quotes intact. The quotes
/// are what keep the production unambiguous, and stripping a suffix without
/// checking for a closing quote first is exactly what would break that. What
/// such a name *does* cost is a weaker type, never a wrong one: `TypeDef.name`
/// holds it dequoted while the declaration keeps its quotes, so the lookup
/// misses and the column resolves `Unknown`.
///
/// Deficiency register: `deficiency: KD4` — a type name that needs quoting
/// therefore resolves `Unknown` (I29). No spelling is misread and every value
/// still decodes as the text the file holds, so what is lost is strength, not
/// correctness, and it is unreachable from a dump whose type names are
/// ordinary identifiers. **(c) unowned.** Closing it means a real type-name
/// tokenizer, which is strictly additive and not this function's.
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
/// is not a domain. The terminal is returned as the DDL spelled it; each of
/// its two readers normalizes what it needs to.
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
        match types.iter().find(|t| t.name == name).map(|t| &t.kind) {
            Some(TypeKind::Domain { base_type, .. }) => name = base_type.trim(),
            _ => break,
        }
    }
    name
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
    let (base, typmod) = builtin_name(declared);
    map_builtin(&base, typmod, types, visits).unwrap_or(TypeOutcome::Unknown)
}

/// The comparison for an array column, from the same walk [`resolve_array`]
/// makes and with the same two refusals: an element type that is opaque by
/// construction (I22), and one that is itself an array (I26). Both resolve the
/// *column* to `Utf8View`, so a column of either compares as text and never
/// reaches this plan — but the register is asked directly too, and must not
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
    let opaque = terminal.eq_ignore_ascii_case("box")
        || matches!(
            types.iter().find(|t| t.name == terminal).map(|t| &t.kind),
            Some(TypeKind::Base | TypeKind::Shell)
        );
    let child = if opaque || array_element(terminal).is_some() {
        // No divergence, because nothing reads one: both shapes resolve the
        // *column* to text (I22, I26), so `crate::predicate::resolve_term`
        // takes the plan away before this tree is reached.
        NestedCompare::Uncomparable { declared: element.to_string(), divergence: None }
    } else {
        match nested_position(element, collation, types, collations, visits) {
            Ok(child) => child,
            Err(reason) => return ComparisonPlan::Unanswerable(reason),
        }
    };
    ComparisonPlan::Nested(NestedCompare::Array(Box::new(child)))
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
    discrete: bool,
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
            }
        }
        ComparisonPlan::Refused => {
            NestedCompare::Uncomparable { declared: declared.to_string(), divergence: None }
        }
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
    let (base, typmod) = builtin_name(declared);
    if let Some((_, plan)) = builtin_scalar(&base, typmod, collation, collations) {
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
    let Some(def) = types.iter().find(|t| t.name == name) else {
        let companion_of = types.iter().find(|t| {
            matches!(&t.kind, TypeKind::Range { multirange_type_name: Some(n), .. } if n == name)
        });
        return match companion_of {
            // A multirange over a range that canonicalizes is unanswerable
            // for the same reason the range is, and names the *range* type:
            // the companion has no DDL of its own to name (I10).
            Some(TypeDef { name, kind: TypeKind::Range { subtype, canonical, .. } }) => {
                match canonical {
                    Some(function) => unanswerable_range(name, function),
                    None => {
                        range_comparison(subtype.as_deref(), false, true, types, collations, visits)
                    }
                }
            }
            _ => ComparisonPlan::Refused,
        };
    };
    match &def.kind {
        // An enum with no labels resolves to no Arrow type at all, so no
        // column of it is ever asked how it compares.
        TypeKind::Enum { labels } if labels.is_empty() => ComparisonPlan::Refused,
        // PostgreSQL orders an enum by `pg_enum.enumsortorder`, assigned from
        // *declaration* order (I33) — so a label's position in this list is
        // the order and its text is not.
        TypeKind::Enum { labels } => {
            ComparisonPlan::agrees(CompareKind::Enum(labels.iter().cloned().collect()))
        }
        // A domain compares as what it bottoms out at, through any chain —
        // the same recursion `resolve_declared_type` makes, bounded the same
        // way. The collation walks down with it and the *column's* clause
        // wins: a domain's own `COLLATE` is its type default, which a
        // column-level clause only overrides (I37).
        TypeKind::Domain { base_type, collation: domain_collation } => comparison_walk(
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
                        .map(|position| (f.name.clone(), position))
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
            None => range_comparison(subtype.as_deref(), false, false, types, collations, visits),
        },
        TypeKind::Base | TypeKind::Shell => ComparisonPlan::Refused,
    }
}

#[cfg(test)]
mod tests {
    use arrow::datatypes::TimeUnit;

    use super::*;
    use crate::preamble::ColumnDef;

    fn ty(name: &str, kind: TypeKind) -> TypeDef {
        TypeDef { name: name.to_string(), kind }
    }

    #[test]
    fn maps_simple_builtins() {
        assert_eq!(
            resolve_declared_type("integer", &[]),
            TypeOutcome::Mapped(DataType::Int32, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("boolean", &[]),
            TypeOutcome::Mapped(DataType::Boolean, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("bytea", &[]),
            TypeOutcome::Mapped(DataType::Binary, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("uuid", &[]),
            TypeOutcome::Mapped(DataType::FixedSizeBinary(16), NestedPlan::Scalar)
        );
    }

    /// `oid` is PostgreSQL's one unsigned integer type, and the width is the
    /// point: `oidout` writes `%u`, where the ADBC driver's `Int32` turns
    /// every OID at or above 2^31 negative.
    #[test]
    fn oid_is_unsigned() {
        assert_eq!(
            resolve_declared_type("oid", &[]),
            TypeOutcome::Mapped(DataType::UInt32, NestedPlan::Scalar)
        );
    }

    #[test]
    fn maps_case_insensitively_for_the_binary_upgrade_dummy_column_shape() {
        // `INTEGER /* dummy */` -> preamble.rs already strips the comment,
        // leaving bare uppercase "INTEGER" (I5).
        assert_eq!(
            resolve_declared_type("INTEGER", &[]),
            TypeOutcome::Mapped(DataType::Int32, NestedPlan::Scalar)
        );
    }

    #[test]
    fn text_like_types_map_to_utf8view_deliberately() {
        for declared in ["text", "character varying(16)", "character(10)", "name"] {
            assert_eq!(
                resolve_declared_type(declared, &[]),
                TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
            );
        }
    }

    #[test]
    fn timestamps_are_always_microsecond_regardless_of_typmod() {
        assert_eq!(
            resolve_declared_type("timestamp without time zone", &[]),
            TypeOutcome::Mapped(
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None),
                NestedPlan::Scalar
            )
        );
        assert_eq!(
            resolve_declared_type("timestamp with time zone", &[]),
            TypeOutcome::Mapped(
                DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, Some("UTC".into())),
                NestedPlan::Scalar
            )
        );
        assert_eq!(
            resolve_declared_type("time with time zone", &[]),
            TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
        );
        // The typmod `format_type` writes mid-name, before the zone.
        for (declared, bare) in [
            ("timestamp(3) with time zone", "timestamp with time zone"),
            ("timestamp(0) without time zone", "timestamp without time zone"),
            ("time(2) with time zone", "time with time zone"),
            ("time(6) without time zone", "time without time zone"),
        ] {
            assert_eq!(resolve_declared_type(declared, &[]), resolve_declared_type(bare, &[]));
        }
    }

    /// **A declared type is read as PostgreSQL's grammar reads it**: every
    /// spelling on the left names the type on the right, a catalog name, a
    /// grammar keyword or an `interval` field qualifier alike.
    #[test]
    fn every_spelling_of_a_built_in_is_that_built_in() {
        for (declared, canonical) in [
            ("int2", "smallint"),
            ("int4", "integer"),
            ("INT", "integer"),
            ("int8", "bigint"),
            ("float4", "real"),
            ("float(24)", "real"),
            ("float8", "double precision"),
            ("float", "double precision"),
            ("float(25)", "double precision"),
            ("bool", "boolean"),
            ("bpchar", "character"),
            ("bpchar(5)", "character(5)"),
            ("char(5)", "character(5)"),
            ("national character(5)", "character(5)"),
            ("varchar(16)", "character varying(16)"),
            ("char  varying", "character varying"),
            ("decimal(10,2)", "numeric(10,2)"),
            ("dec", "numeric"),
            ("timestamp", "timestamp without time zone"),
            ("timestamptz", "timestamp with time zone"),
            ("TIMESTAMP(3)", "timestamp without time zone"),
            ("time", "time without time zone"),
            ("timetz", "time with time zone"),
            ("interval year to month", "interval"),
            ("interval day to second(3)", "interval"),
            ("interval second", "interval"),
            ("\"int4\"", "integer"),
            ("\"timestamptz\"", "timestamp with time zone"),
        ] {
            let got = resolve_declared_type(declared, &[]);
            assert_ne!(got, TypeOutcome::Unknown, "`{declared}`");
            assert_eq!(got, resolve_declared_type(canonical, &[]), "`{declared}`");
            assert_eq!(
                comparison_for(declared, None, &[], &[]),
                comparison_for(canonical, None, &[], &[]),
                "`{declared}`"
            );
        }
    }

    /// A quoted name is a catalog name, case and all: `"char"` is the one-byte
    /// internal type, which no arm maps, `"INT4"` names nothing, and a
    /// spelling only the grammar knows names a type `pg_catalog` does not
    /// hold, so `"integer"` is not `integer`. A catalog name whose SQL
    /// spelling is itself is still the built-in quoted.
    #[test]
    fn a_quoted_name_is_only_a_catalog_name() {
        for declared in [
            "\"char\"",
            "\"INT4\"",
            "\"bit\"",
            "interval fortnight",
            "\"integer\"",
            "\"double precision\"",
            "\"character varying\"(10)",
            "\"boolean\"",
            "\"timestamp with time zone\"",
        ] {
            assert_eq!(resolve_declared_type(declared, &[]), TypeOutcome::Unknown, "`{declared}`");
            assert_eq!(comparison_for(declared, None, &[], &[]), ComparisonPlan::Refused);
            assert_eq!(extension_for(declared, &[]), None, "`{declared}`");
        }
        for (declared, bare) in [("\"text\"", "text"), ("\"numeric\"(10,2)", "numeric(10,2)")] {
            assert_eq!(resolve_declared_type(declared, &[]), resolve_declared_type(bare, &[]));
        }
    }

    /// `interval` is the one type whose Arrow mapping is a *struct* of
    /// PostgreSQL's own three fields rather than a reading of them: months,
    /// days and a time part, independent, exactly as `Interval` stores them.
    #[test]
    fn interval_maps_to_arrows_three_field_calendar_interval() {
        assert_eq!(
            resolve_declared_type("interval", &[]),
            TypeOutcome::Mapped(
                DataType::Interval(arrow::datatypes::IntervalUnit::MonthDayNano),
                NestedPlan::Scalar
            )
        );
    }

    #[test]
    fn numeric_picks_decimal_width_by_precision() {
        assert_eq!(
            resolve_declared_type("numeric(38,10)", &[]),
            TypeOutcome::Mapped(DataType::Decimal128(38, 10), NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric(39,0)", &[]),
            TypeOutcome::Mapped(DataType::Decimal256(39, 0), NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric", &[]),
            TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric(2,-2)", &[]),
            TypeOutcome::Mapped(DataType::Decimal128(2, -2), NestedPlan::Scalar)
        );
        assert_eq!(
            resolve_declared_type("numeric(77,0)", &[]),
            TypeOutcome::Mapped(DataType::Utf8View, NestedPlan::Scalar)
        );
    }

    #[test]
    fn unrecognized_builtin_is_unknown() {
        assert_eq!(resolve_declared_type("money", &[]), TypeOutcome::Unknown);
    }

    /// The twelve built-in range/multirange names carry their subtypes in the
    /// catalog, never in DDL (I10). The multirange half maps to a `List` of
    /// the *same* range struct — same subtype, different literal form.
    #[test]
    fn builtin_ranges_and_their_multirange_companions_map_to_their_hardcoded_subtypes() {
        let bounds = [
            ("int4range", "int4multirange", DataType::Int32),
            ("int8range", "int8multirange", DataType::Int64),
            // Bare `numeric` has no Arrow decimal representation, so a
            // `numrange`'s bounds are text — the subtype's own mapping.
            ("numrange", "nummultirange", DataType::Utf8View),
            ("tsrange", "tsmultirange", DataType::Timestamp(TimeUnit::Microsecond, None)),
            (
                "tstzrange",
                "tstzmultirange",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            ),
            ("daterange", "datemultirange", DataType::Date32),
        ];
        for (range, multirange, bound) in bounds {
            assert_eq!(
                resolve_declared_type(range, &[]),
                TypeOutcome::Mapped(
                    range_struct(bound.clone()),
                    NestedPlan::Range(Box::new(NestedPlan::Scalar))
                ),
                "{range}"
            );
            assert_eq!(
                resolve_declared_type(multirange, &[]),
                TypeOutcome::Mapped(
                    list_of(range_struct(bound)),
                    NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
                ),
                "{multirange}"
            );
        }
    }

    /// The collision `NestedPlan` exists for, asserted where it is otherwise
    /// invisible: one Arrow type, two literal forms
    /// (`{"[1,10)","[2,3)"}` versus `{[1,10),[2,3)}`).
    #[test]
    fn an_array_of_ranges_and_a_multirange_agree_on_the_type_and_not_on_the_plan() {
        let TypeOutcome::Mapped(array_type, array_plan) = resolve_declared_type("int4range[]", &[])
        else {
            panic!("int4range[] maps")
        };
        let TypeOutcome::Mapped(multi_type, multi_plan) =
            resolve_declared_type("int4multirange", &[])
        else {
            panic!("int4multirange maps")
        };
        assert_eq!(array_type, multi_type);
        assert_ne!(array_plan, multi_plan);
    }

    #[test]
    fn an_array_maps_to_a_list_of_its_element_type() {
        assert_eq!(
            resolve_declared_type("integer[]", &[]),
            TypeOutcome::Mapped(
                list_of(DataType::Int32),
                NestedPlan::Array(Box::new(NestedPlan::Scalar))
            )
        );
        let types = [ty("public.mood", TypeKind::Enum { labels: vec!["sad".into()] })];
        assert_eq!(
            resolve_declared_type("public.mood[]", &types),
            TypeOutcome::Mapped(
                list_of(DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))),
                NestedPlan::Array(Box::new(NestedPlan::Scalar))
            )
        );
        // A type Arrow has no representation for is `Utf8View` *in that
        // position*, exactly as it would be at top level.
        assert_eq!(
            resolve_declared_type("inet[]", &[]),
            TypeOutcome::Mapped(
                list_of(DataType::Utf8View),
                NestedPlan::Array(Box::new(NestedPlan::Scalar))
            )
        );
    }

    /// The delimiter trap (I22): an array's separator is its *element type's*
    /// `typdelim`, and `box`'s is `;`. The refusal has to run on the terminal
    /// of the domain walk, since a domain's own DDL records nothing about it.
    #[test]
    fn an_array_over_an_opaque_element_type_is_refused_through_any_chain_of_domains() {
        let types = [
            ty("public.mybase", TypeKind::Base),
            ty("public.shellonly", TypeKind::Shell),
            ty("public.box_domain", TypeKind::domain("box")),
            ty("public.box_domain2", TypeKind::domain("public.box_domain")),
        ];
        for declared in [
            "box[]",
            "public.mybase[]",
            "public.shellonly[]",
            "public.box_domain[]",
            "public.box_domain2[]",
        ] {
            assert_eq!(
                resolve_declared_type(declared, &types),
                TypeOutcome::OpaqueElementType,
                "{declared}"
            );
        }
        // The scalar cases are unchanged: only the *array* is refused for its
        // element, and `box` itself is simply a type this build never mapped.
        assert_eq!(resolve_declared_type("box", &types), TypeOutcome::Unknown);
        assert_eq!(
            resolve_declared_type("public.box_domain", &types),
            TypeOutcome::Unknown,
            "a domain resolves through to its base, which this build has no mapping for"
        );
    }

    /// I26, and the transitive half the fixture does not carry: a domain over
    /// a domain over an array produces a literal byte-identical to the
    /// single-hop case, so the walk is pinned here instead.
    #[test]
    fn an_array_over_an_array_typed_element_is_refused_through_any_chain_of_domains() {
        let types = [
            ty("public.intarr", TypeKind::domain("integer[]")),
            ty("public.intarr2", TypeKind::domain("public.intarr")),
            ty("public.intarr3", TypeKind::domain("public.intarr2")),
        ];
        for declared in ["public.intarr[]", "public.intarr2[]", "public.intarr3[]"] {
            assert_eq!(
                resolve_declared_type(declared, &types),
                TypeOutcome::NestedArrayElement,
                "{declared}"
            );
        }
        // Only the outer array is refused: the domain itself is an ordinary
        // `integer[]` column at every depth of the chain.
        for declared in ["public.intarr", "public.intarr2", "public.intarr3"] {
            assert_eq!(
                resolve_declared_type(declared, &types),
                TypeOutcome::Mapped(
                    list_of(DataType::Int32),
                    NestedPlan::Array(Box::new(NestedPlan::Scalar))
                ),
                "{declared}"
            );
        }
    }

    /// I28: PostgreSQL accepts six spellings for an array-typed column and
    /// every one is the same type — the parser keeps "array of the element"
    /// and discards the bracket count and the bounds — so all six resolve
    /// exactly as `integer[]` does, one array level deep.
    ///
    /// Five of the six survive no round trip through `format_type`, so
    /// `pg_dump` can never write them (I21) and no generated fixture reaches
    /// this: `roadmap.md`'s "Where a fixture is impossible" carve-out.
    #[test]
    fn every_array_declaration_spelling_is_one_array_of_the_element_type() {
        let expected = TypeOutcome::Mapped(
            list_of(DataType::Int32),
            NestedPlan::Array(Box::new(NestedPlan::Scalar)),
        );
        for declared in [
            // The six of I28's table.
            "integer[]",
            "integer[3]",
            "integer[][]",
            "integer[3][4]",
            "integer ARRAY",
            "integer ARRAY[4]",
            // The bracket run is unbounded in the DDL even though `MAXDIM`
            // is 6; `ARRAY` is a keyword, so its case carries nothing; and
            // the lexer is free with whitespace.
            "integer[][][][][][][][][]",
            "INTEGER array[4]",
            "integer  Array",
            "integer [ 3 ] [ ]",
        ] {
            assert_eq!(resolve_declared_type(declared, &[]), expected, "{declared}");
        }
    }

    /// The other direction of the same rule: a declaration PostgreSQL rejects
    /// gets no array type invented for it. `ARRAY` takes at most one bound and
    /// only in the `[n]` form, what precedes it is a bare type name, and a
    /// malformed bound is not a bound. Every string here is a syntax error,
    /// and `Unknown` says so where a refusal would state something false.
    #[test]
    fn a_declaration_postgresql_would_reject_is_not_read_as_an_array() {
        for declared in [
            "integer ARRAY[4][5]",
            "integer ARRAY[]",
            "integer ARRAY ARRAY",
            "integer[] ARRAY",
            "integer[abc]",
            "integer[-1]",
            "integer[",
            "integerARRAY",
            "ARRAY",
        ] {
            assert_eq!(resolve_declared_type(declared, &[]), TypeOutcome::Unknown, "{declared}");
        }
    }

    /// A domain's base type is spelled by whoever wrote the `CREATE DOMAIN`,
    /// so the array-ness the I26 refusal tests for can arrive in any spelling
    /// and the domain walk stops on the raw text. `d[]` over `CREATE DOMAIN d
    /// AS integer ARRAY` is refused exactly as `d[]` over `integer[]` is,
    /// while `d` itself is an ordinary `integer[]` column.
    #[test]
    fn a_domain_over_an_array_is_recognized_in_every_spelling() {
        for base in ["integer[]", "integer[3]", "integer[][]", "integer ARRAY", "integer ARRAY[4]"]
        {
            let types = [ty("public.d", TypeKind::domain(base))];
            assert_eq!(
                resolve_declared_type("public.d[]", &types),
                TypeOutcome::NestedArrayElement,
                "{base}"
            );
            assert_eq!(
                resolve_declared_type("public.d", &types),
                TypeOutcome::Mapped(
                    list_of(DataType::Int32),
                    NestedPlan::Array(Box::new(NestedPlan::Scalar))
                ),
                "{base}"
            );
        }
    }

    /// The refusal composes into a composite for free: `resolve_nested` maps
    /// every non-`Mapped` outcome to `Utf8View` *in that position*, so a field
    /// of the refused type is one string field, not a refused column.
    #[test]
    fn a_composite_field_of_the_refused_array_type_is_a_string_field_only() {
        let types = [
            ty("public.intarr", TypeKind::domain("integer[]")),
            ty(
                "public.arr_holder",
                TypeKind::Composite {
                    fields: Some(vec![
                        ColumnDef::new("label", "text"),
                        ColumnDef::new("arr", "public.intarr[]"),
                    ]),
                },
            ),
        ];
        assert_eq!(
            resolve_declared_type("public.arr_holder", &types),
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(vec![
                    Field::new("label", DataType::Utf8View, true),
                    Field::new("arr", DataType::Utf8View, true),
                ])),
                NestedPlan::Record(vec![NestedPlan::Scalar, NestedPlan::Scalar])
            )
        );
    }

    #[test]
    fn a_composite_maps_to_a_struct_of_its_declared_fields() {
        let types = [
            ty(
                "public.point2d",
                TypeKind::Composite {
                    fields: Some(vec![ColumnDef::new("x", "integer"), ColumnDef::new("y", "text")]),
                },
            ),
            ty("public.empty_comp", TypeKind::Composite { fields: Some(vec![]) }),
            ty("public.broken", TypeKind::Composite { fields: None }),
        ];
        assert_eq!(
            resolve_declared_type("public.point2d", &types),
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(vec![
                    Field::new("x", DataType::Int32, true),
                    Field::new("y", DataType::Utf8View, true),
                ])),
                NestedPlan::Record(vec![NestedPlan::Scalar, NestedPlan::Scalar])
            )
        );
        // A zero-field composite is a real type with a real value, `()`
        // (I23) — a zero-field `Struct` is well-formed Arrow.
        assert_eq!(
            resolve_declared_type("public.empty_comp", &types),
            TypeOutcome::Mapped(DataType::Struct(Fields::empty()), NestedPlan::Record(Vec::new()))
        );
        // A body the grammar could not read is *not* a short field list:
        // `record_out` is positional, so a `Struct` built from one would
        // refuse every valid row.
        assert_eq!(resolve_declared_type("public.broken", &types), TypeOutcome::Unknown);
    }

    /// Nesting composes through the same function, so a composite field's own
    /// array and an array of composites are the same recursion in two orders.
    #[test]
    fn nesting_composes_in_both_orders() {
        let types = [
            ty(
                "public.point2d",
                TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
            ),
            ty(
                "public.tagged",
                TypeKind::Composite { fields: Some(vec![ColumnDef::new("tags", "text[]")]) },
            ),
        ];
        let point = DataType::Struct(Fields::from(vec![Field::new("x", DataType::Int32, true)]));
        assert_eq!(
            resolve_declared_type("public.point2d[]", &types),
            TypeOutcome::Mapped(
                list_of(point),
                NestedPlan::Array(Box::new(NestedPlan::Record(vec![NestedPlan::Scalar])))
            )
        );
        assert_eq!(
            resolve_declared_type("public.tagged", &types),
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(vec![Field::new(
                    "tags",
                    list_of(DataType::Utf8View),
                    true
                )])),
                NestedPlan::Record(vec![NestedPlan::Array(Box::new(NestedPlan::Scalar))])
            )
        );
    }

    /// A user-defined range's subtype comes from its own DDL, and a range
    /// whose parameter list the grammar could not read keeps the struct with
    /// text bounds rather than losing the shape.
    #[test]
    fn a_user_range_maps_through_its_declared_subtype() {
        let types = [
            ty(
                "public.myrange",
                TypeKind::Range {
                    subtype: Some("double precision".to_string()),
                    multirange_type_name: None,
                    canonical: None,
                },
            ),
            ty(
                "public.bare",
                TypeKind::Range { subtype: None, multirange_type_name: None, canonical: None },
            ),
        ];
        assert_eq!(
            resolve_declared_type("public.myrange", &types),
            TypeOutcome::Mapped(
                range_struct(DataType::Float64),
                NestedPlan::Range(Box::new(NestedPlan::Scalar))
            )
        );
        assert_eq!(
            resolve_declared_type("public.bare", &types),
            TypeOutcome::Mapped(
                range_struct(DataType::Utf8View),
                NestedPlan::Range(Box::new(NestedPlan::Scalar))
            )
        );
    }

    /// I10: a user range's auto-created multirange companion has no
    /// `CREATE TYPE` of its own anywhere in the dump — its only trace is the
    /// `multirange_type_name` parameter inside the range's own DDL, so a
    /// column declared with that companion name must still resolve, not
    /// fall through to `Unknown`.
    #[test]
    fn a_ranges_multirange_companion_resolves_even_with_no_type_def_of_its_own() {
        let types = [ty(
            "public.myrange",
            TypeKind::Range {
                subtype: Some("double precision".to_string()),
                multirange_type_name: Some("public.myrange_multi".to_string()),
                canonical: None,
            },
        )];
        assert_eq!(
            resolve_declared_type("public.myrange_multi", &types),
            TypeOutcome::Mapped(
                list_of(range_struct(DataType::Float64)),
                NestedPlan::Multirange(Box::new(NestedPlan::Scalar))
            ),
            "the companion's bound type comes from the range that names it"
        );
        // A name that merely resembles a companion but isn't named by any
        // range's `multirange_type_name` stays `Unknown`.
        assert_eq!(resolve_declared_type("public.not_a_companion", &types), TypeOutcome::Unknown);
    }

    #[test]
    fn base_and_shell_types_are_opaque() {
        let types = [ty("public.gtype", TypeKind::Base), ty("public.forward", TypeKind::Shell)];
        assert_eq!(resolve_declared_type("public.gtype", &types), TypeOutcome::OpaqueBaseType);
        assert_eq!(resolve_declared_type("public.forward", &types), TypeOutcome::OpaqueBaseType);
    }

    #[test]
    fn a_user_type_absent_from_the_list_is_unknown() {
        assert_eq!(resolve_declared_type("public.nope", &[]), TypeOutcome::Unknown);
    }

    // -- the canonical extension names -------------------------------------

    /// The table the extension names are keyed on, and the storage type each
    /// requires. Kept beside the tests so both directions read it.
    const EXTENSIONS: [(&str, CanonicalExtension, DataType); 3] = [
        ("uuid", CanonicalExtension::Uuid, DataType::FixedSizeBinary(16)),
        ("json", CanonicalExtension::Json, DataType::Utf8View),
        ("jsonb", CanonicalExtension::Json, DataType::Utf8View),
    ];

    /// **The join that keeps `extension_for` and `builtin_scalar` in step.**
    /// They are separate walks of the same string: a `uuid` remapped away
    /// from `FixedSizeBinary(16)` would make `apply`'s `expect` a panic on a
    /// real dump. `try_with_extension_type` is arrow-rs's own
    /// `supports_data_type`, so this asserts the crate's rule.
    #[test]
    fn the_extension_names_fit_their_storage_type() {
        for (declared, extension, storage) in EXTENSIONS {
            assert_eq!(extension_for(declared, &[]), Some(extension), "{declared}");
            assert_eq!(
                builtin_scalar(declared, None, None, &[]).map(|(dt, _)| dt),
                Some(storage.clone()),
                "{declared}"
            );
            let field = extension.apply(Field::new("v", storage, true));
            assert!(
                field.try_canonical_extension_type().is_ok(),
                "{declared}: arrow-rs must read back what we wrote"
            );
        }
    }

    /// The names themselves, in the metadata a consumer reads. `arrow.json`
    /// carries an empty metadata value and `arrow.uuid` carries none, which
    /// is the difference hand-writing the two keys would get wrong.
    #[test]
    fn the_extension_metadata_is_the_canonical_spelling() {
        let uuid =
            with_extension(Field::new("v", DataType::FixedSizeBinary(16), true), "uuid", &[]);
        assert_eq!(
            uuid.metadata().get("ARROW:extension:name").map(String::as_str),
            Some("arrow.uuid")
        );
        assert_eq!(uuid.metadata().get("ARROW:extension:metadata"), None);

        let json = with_extension(Field::new("v", DataType::Utf8View, true), "jsonb", &[]);
        assert_eq!(
            json.metadata().get("ARROW:extension:name").map(String::as_str),
            Some("arrow.json")
        );
        assert_eq!(json.metadata().get("ARROW:extension:metadata").map(String::as_str), Some(""));
    }

    /// A domain bottoms out at the extension its base type names, through any
    /// chain — the same recursion `resolve_declared_type` makes, so the two
    /// cannot disagree about what a domain column holds.
    #[test]
    fn a_domain_over_uuid_carries_the_name() {
        let types = [
            ty("public.d_uuid", TypeKind::Domain { base_type: "uuid".into(), collation: None }),
            ty(
                "public.d_deep",
                TypeKind::Domain { base_type: "public.d_uuid".into(), collation: None },
            ),
        ];
        assert_eq!(extension_for("public.d_deep", &types), Some(CanonicalExtension::Uuid));
    }

    /// Everything that carries no name, and the array is the one worth
    /// stating: `uuid[]` is a `List<FixedSizeBinary(16)>`, and `arrow.uuid`
    /// on the column's own field would claim the *list* is a UUID.
    #[test]
    fn nothing_else_carries_a_name() {
        let types = [
            ty("public.mood", TypeKind::Enum { labels: vec!["sad".into()] }),
            ty("public.d_int", TypeKind::Domain { base_type: "integer".into(), collation: None }),
        ];
        for declared in ["uuid[]", "json[]", "text", "integer", "public.mood", "public.d_int"] {
            assert_eq!(extension_for(declared, &types), None, "{declared}");
        }
        // A `.`-qualified name the dump never declared names no extension.
        assert_eq!(extension_for("public.nope", &types), None);
    }

    // -- the comparison register ------------------------------------------

    fn agrees(kind: CompareKind) -> ComparisonPlan {
        ComparisonPlan::agrees(kind)
    }

    /// The register, row for row: every declared type this build maps to a
    /// scalar, and how a column of it compares
    /// (`docs/design/decisions.md`, "D40").
    ///
    /// **Every arm of [`builtin_scalar`] appears here** but `int2vector`'s,
    /// which maps to a list and compares as a nested column (D58): a type
    /// added to that table without a row here is one nothing states the
    /// comparison of.
    #[test]
    fn the_register_answers_every_builtin_scalar() {
        use CompareKind as K;
        let text = || ComparisonPlan::AS_TEXT;
        let unknown_collation =
            || ComparisonPlan::diverging(CompareKind::Text, ComparisonDivergence::UnknownCollation);
        let unknown_padded = || {
            ComparisonPlan::diverging(
                CompareKind::PaddedText,
                ComparisonDivergence::UnknownCollation,
            )
        };
        for (declared, expected) in [
            ("smallint", agrees(K::Int)),
            ("integer", agrees(K::Int)),
            ("bigint", agrees(K::Int)),
            ("oid", agrees(K::UnsignedInt)),
            ("boolean", agrees(K::Bool)),
            ("real", agrees(K::Float32)),
            ("double precision", agrees(K::Float64)),
            ("numeric(10,2)", agrees(K::Decimal(2))),
            ("numeric(50,0)", agrees(K::Decimal(0))),
            // The two `numeric` shapes with no Arrow decimal behind them, and
            // the one place the register distinguishes them: a typmod rejects
            // an infinity, so only the bare form admits the spelling (I34).
            ("numeric", agrees(K::Numeric { infinities: true })),
            ("numeric(77,0)", agrees(K::Numeric { infinities: false })),
            // The four collatable arms, each asked with no `COLLATE` clause.
            // `name`'s type default is `C`, so it agrees where the others
            // cannot; `character(n)` carries a comparison of its own (the
            // padding is trimmed off both sides, I38).
            ("text", unknown_collation()),
            ("character varying(10)", unknown_collation()),
            ("character(10)", unknown_padded()),
            ("name", agrees(K::Text)),
            ("date", agrees(K::Date)),
            ("timestamp without time zone", agrees(K::Timestamp { with_tz: false })),
            ("timestamp with time zone", agrees(K::Timestamp { with_tz: true })),
            ("time without time zone", agrees(K::Time)),
            ("time with time zone", agrees(K::TimeTz)),
            ("interval", agrees(K::Interval)),
            ("uuid", agrees(K::Uuid)),
            ("bytea", agrees(K::Bytea)),
            // The one the register still holds as text, because the server
            // defines no order for it to be wrong about.
            ("json", text()),
            // `jsonb` is ordered structurally and diverges only where a
            // string leaf decides, which is the database's collation (I41).
            (
                "jsonb",
                ComparisonPlan::diverging(K::Jsonb, ComparisonDivergence::JsonbStringCollation),
            ),
            // `cidr` differs from `inet` only in refusing a literal with a
            // bit set below its netmask, and `macaddr8` from `macaddr` only
            // in its width — both carried, the declared name being gone.
            ("inet", agrees(K::Network { cidr: false })),
            ("cidr", agrees(K::Network { cidr: true })),
            ("macaddr", agrees(K::MacAddr { octets: 6 })),
            ("macaddr8", agrees(K::MacAddr { octets: 8 })),
        ] {
            assert_eq!(comparison_for(declared, None, &[], &[]), expected, "{declared}");
        }
        // A declared type this build maps to nothing has no comparison
        // either — one walk of the same string answers both.
        assert_eq!(comparison_for("money", None, &[], &[]), ComparisonPlan::Refused);
        // A keyword is a keyword on both walks (I5).
        assert_eq!(comparison_for("INTEGER", None, &[], &[]), agrees(K::Int));
    }

    /// Six unrelated declared types reach the same text columns — five as
    /// `Utf8View`, an enum as a `Dictionary` of its labels — which is why the
    /// register cannot be keyed on the Arrow type: an enum compares by
    /// declaration order, a bare `numeric` by decimal value, an `inet` by
    /// family-then-prefix, a `jsonb` by a walk down two containers, `text`
    /// bytewise under its collation, and `json` bytewise with no order at all.
    #[test]
    fn the_register_tells_apart_types_that_share_one_arrow_type() {
        let labels = ["sad".to_string(), "ok".to_string()];
        let types = [ty("public.mood", TypeKind::Enum { labels: labels.to_vec() })];
        assert_eq!(
            comparison_for("public.mood", None, &types, &[]),
            ComparisonPlan::agrees(CompareKind::Enum(labels.iter().cloned().collect())),
        );
        assert_eq!(
            comparison_for("numeric", None, &[], &[]),
            ComparisonPlan::agrees(CompareKind::Numeric { infinities: true }),
        );
        assert_eq!(
            comparison_for("inet", None, &[], &[]),
            ComparisonPlan::agrees(CompareKind::Network { cidr: false }),
        );
        assert_eq!(comparison_for("json", None, &[], &[]), ComparisonPlan::AS_TEXT);
        // `jsonb` shares the Arrow type and neither the comparison nor the
        // verdict: it is a container walk that diverges only at a string.
        assert_eq!(
            comparison_for("jsonb", None, &[], &[]),
            ComparisonPlan::diverging(
                CompareKind::Jsonb,
                ComparisonDivergence::JsonbStringCollation
            ),
        );
        // `text` is the last, and it does not share `AS_TEXT` with them:
        // reading the collation is what tells them apart.
        assert_eq!(
            comparison_for("text", None, &[], &[]),
            ComparisonPlan::diverging(CompareKind::Text, ComparisonDivergence::UnknownCollation),
        );
        // An enum with no labels resolves to no Arrow type at all, so no
        // column of it is ever asked how it compares.
        let empty = [ty("public.empty", TypeKind::Enum { labels: vec![] })];
        assert_eq!(comparison_for("public.empty", None, &empty, &[]), ComparisonPlan::Refused);
    }

    /// **A scalar column is bounded in every semantics whose order over its
    /// text is exact, one set where two coincide**: a divergent plan's one set
    /// is Arrow's and read there alone, a column with no plan is bounded
    /// bytewise for Arrow, and an exact kind Arrow orders otherwise keeps a
    /// second set in Arrow's order — but `macaddr`, whose one set is read in
    /// both (`docs/design/decisions.md`, "D79").
    #[test]
    fn a_column_is_bounded_in_each_semantics_whose_order_is_exact() {
        use BoundsSet::{Arrow as A, Primary as P};
        use ComparisonDivergence::*;
        use ComparisonSemantics::{Arrow, Postgres};
        let text = || Some(CompareKind::Text);
        let read = |plan: &ComparisonPlan| {
            (plan.bounds_kinds(), plan.bounds_in(Postgres), plan.bounds_in(Arrow))
        };
        assert_eq!(
            read(&ComparisonPlan::agrees(CompareKind::Text)),
            ([text(), None], Some(P), Some(P))
        );
        for divergence in
            [AsText, UnknownCollation, NonBytewiseCollation, NonDeterministicCollation]
        {
            let plan = ComparisonPlan::diverging(CompareKind::Text, divergence);
            assert_eq!(read(&plan), ([text(), None], None, Some(P)), "{divergence:?}");
        }
        for (kind, divergence) in [
            (CompareKind::PaddedText, UnknownCollation),
            (CompareKind::PaddedText, NonBytewiseCollation),
            (CompareKind::Jsonb, JsonbStringCollation),
        ] {
            let plan = ComparisonPlan::diverging(kind.clone(), divergence);
            assert_eq!(read(&plan), ([text(), None], None, Some(P)), "{kind:?}");
        }
        assert_eq!(read(&ComparisonPlan::Refused), ([text(), None], None, Some(P)));
        let labels: Arc<[String]> = Arc::from(vec!["b".to_string(), "a".to_string()]);
        for kind in [
            CompareKind::PaddedText,
            CompareKind::Enum(labels),
            CompareKind::Numeric { infinities: true },
            CompareKind::TimeTz,
            CompareKind::Network { cidr: false },
            CompareKind::Jsonb,
        ] {
            let plan = ComparisonPlan::agrees(kind.clone());
            assert_eq!(read(&plan), ([Some(kind.clone()), text()], Some(P), Some(A)), "{kind:?}");
        }
        assert_eq!(
            read(&ComparisonPlan::agrees(CompareKind::Interval)),
            ([Some(CompareKind::Interval), Some(CompareKind::IntervalFields)], Some(P), Some(A))
        );
        for kind in [CompareKind::MacAddr { octets: 6 }, CompareKind::Int, CompareKind::Bytea] {
            let plan = ComparisonPlan::agrees(kind.clone());
            assert_eq!(read(&plan), ([Some(kind.clone()), None], Some(P), Some(P)), "{kind:?}");
        }
        let nested = ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
            range_type: "r".into(),
            function: "f".into(),
        });
        assert_eq!(read(&nested), ([None, None], None, None));
    }

    /// **A term reads the set gathering stored under the kind it compares
    /// by**, not the one its own plan would have stored: a column compared as
    /// text for want of a plan — every column under `SchemaMode::Strings` —
    /// reads a set keyed `Text` or `macaddr`'s, whose text orders as its
    /// octets (I40), and never one keyed by a value order
    /// (`docs/design/decisions.md`, "D79").
    #[test]
    fn a_term_reads_the_set_keyed_by_the_kind_it_compares_by() {
        use BoundsSet::{Arrow as A, Primary as P};
        let text = CompareKind::Text;
        let refused = ComparisonPlan::Refused.bounds_read_by(ComparisonSemantics::Arrow);
        assert_eq!(refused.as_ref(), Some(&text));
        assert_eq!(ComparisonPlan::Refused.bounds_read_by(ComparisonSemantics::Postgres), None);
        let labels: Arc<[String]> = Arc::from(vec!["b".to_string()]);
        for (declared, stored, want) in [
            ("text", [Some(text.clone()), None], Some(P)),
            ("an enum", [Some(CompareKind::Enum(labels)), Some(text.clone())], Some(A)),
            ("macaddr", [Some(CompareKind::MacAddr { octets: 6 }), None], Some(P)),
            ("integer", [Some(CompareKind::Int), None], None),
            ("interval", [Some(CompareKind::Interval), Some(CompareKind::IntervalFields)], None),
            ("a nested column", [None, None], None),
        ] {
            assert_eq!(bounds_set_keyed_by(&stored, &text), want, "{declared}");
        }
        let int = [Some(CompareKind::Int), None];
        assert_eq!(bounds_set_keyed_by(&int, &CompareKind::Int), Some(P));
        let mac = [Some(CompareKind::MacAddr { octets: 6 }), None];
        assert_eq!(bounds_set_keyed_by(&mac, &CompareKind::MacAddr { octets: 8 }), None);
    }

    /// The collation rule, as a table: what a text column's `COLLATE` clause
    /// (or its absence, against the type's own default) does to the register's
    /// verdict. **The comparison never moves — only the verdict does.**
    #[test]
    fn a_text_column_is_judged_by_the_collation_it_states() {
        let agrees_text = || ComparisonPlan::agrees(CompareKind::Text);
        let unknown =
            || ComparisonPlan::diverging(CompareKind::Text, ComparisonDivergence::UnknownCollation);
        let named = || {
            ComparisonPlan::diverging(CompareKind::Text, ComparisonDivergence::NonBytewiseCollation)
        };
        for (declared, collation, expected) in [
            // No clause: the type's own default decides, and only `name`'s is
            // `C` (I37).
            ("text", None, unknown()),
            ("character varying(10)", None, unknown()),
            ("name", None, agrees_text()),
            // An explicit clause overrides it in both directions.
            ("text", Some("pg_catalog.\"C\""), agrees_text()),
            ("text", Some("pg_catalog.\"POSIX\""), agrees_text()),
            ("text", Some("pg_catalog.\"en_US.utf8\""), named()),
            ("text", Some("public.mycoll"), named()),
            ("name", Some("pg_catalog.\"en_US.utf8\""), named()),
            // Unqualified, as a hand-written dump might spell it.
            ("text", Some("\"C\""), agrees_text()),
            // An unquoted `C` is the collation `c`, not the built-in one; the
            // server folds it the same way.
            ("text", Some("C"), named()),
            // Nor is a `"C"` some other schema happens to define.
            ("text", Some("public.\"C\""), named()),
            // `character(n)` reads the clause exactly as `text` does, over
            // its own trimmed comparison (I38).
            (
                "character(10)",
                Some("pg_catalog.\"C\""),
                ComparisonPlan::agrees(CompareKind::PaddedText),
            ),
            (
                "character(10)",
                Some("pg_catalog.\"en_US.utf8\""),
                ComparisonPlan::diverging(
                    CompareKind::PaddedText,
                    ComparisonDivergence::NonBytewiseCollation,
                ),
            ),
            (
                "character(10)",
                None,
                ComparisonPlan::diverging(
                    CompareKind::PaddedText,
                    ComparisonDivergence::UnknownCollation,
                ),
            ),
            // A non-collatable type ignores a clause it cannot carry.
            ("integer", Some("pg_catalog.\"C\""), ComparisonPlan::agrees(CompareKind::Int)),
            ("interval", Some("pg_catalog.\"C\""), ComparisonPlan::agrees(CompareKind::Interval)),
            // Including one that is still held as text: `json` is not
            // collatable either, so a clause on it moves nothing.
            ("json", Some("pg_catalog.\"C\""), ComparisonPlan::AS_TEXT),
        ] {
            assert_eq!(
                comparison_for(declared, collation, &[], &[]),
                expected,
                "{declared} {collation:?}"
            );
        }
    }

    /// The fourth collation branch: a column stating a collation the *same
    /// dump* declared `deterministic = false` (I42).
    ///
    /// The only one of the four that reaches `=`, and the only one read off a
    /// statement rather than a name — so the same clause answers differently
    /// depending on what the dump's `CREATE COLLATION` list holds.
    #[test]
    fn a_collation_the_dump_declares_non_deterministic_diverges_under_equality_too() {
        use CompareKind as K;
        let coll = |name: &str, deterministic: bool| CollationDef {
            name: name.to_string(),
            deterministic,
        };
        let nd = ComparisonDivergence::NonDeterministicCollation;
        let named = ComparisonDivergence::NonBytewiseCollation;

        // Nothing declared: the clause is read off its name alone.
        assert_eq!(
            comparison_for("text", Some("public.icu_ci"), &[], &[]),
            ComparisonPlan::diverging(K::Text, named)
        );
        // Declared deterministic moves nothing either.
        assert_eq!(
            comparison_for("text", Some("public.icu_ci"), &[], &[coll("public.icu_ci", true)]),
            ComparisonPlan::diverging(K::Text, named)
        );
        // Declared non-deterministic: same comparison, stronger verdict.
        let declared = [coll("public.other", true), coll("public.icu_ci", false)];
        assert_eq!(
            comparison_for("text", Some("public.icu_ci"), &[], &declared),
            ComparisonPlan::diverging(K::Text, nd)
        );
        // `character(n)` and `character varying` reach the same branch, and
        // `character(n)` keeps its own trim (I38).
        assert_eq!(
            comparison_for("character(10)", Some("public.icu_ci"), &[], &declared),
            ComparisonPlan::diverging(K::PaddedText, nd)
        );
        assert_eq!(
            comparison_for("character varying(10)", Some("public.icu_ci"), &[], &declared),
            ComparisonPlan::diverging(K::Text, nd)
        );
        // A domain's own clause is the column's default, so it reaches the
        // branch the same way a column-level clause does.
        let types = [ty(
            "public.dom_nd",
            TypeKind::Domain {
                base_type: "text".to_string(),
                collation: Some("public.icu_ci".to_string()),
            },
        )];
        assert_eq!(
            comparison_for("public.dom_nd", None, &types, &declared),
            ComparisonPlan::diverging(K::Text, nd)
        );
        // A column of a *different* collation is untouched by the entry.
        assert_eq!(
            comparison_for("text", Some("public.other"), &[], &declared),
            ComparisonPlan::diverging(K::Text, named)
        );
        // A non-collatable type carries no clause and so never reaches it.
        assert_eq!(
            comparison_for("integer", Some("public.icu_ci"), &[], &declared),
            ComparisonPlan::agrees(K::Int)
        );
        // Quoting is not textual: the two spellings come from different
        // `pg_dump` paths and either may quote what the other leaves bare.
        assert_eq!(
            comparison_for(
                "text",
                Some("public.\"icu_ci\""),
                &[],
                &[coll("\"public\".icu_ci", false)]
            ),
            ComparisonPlan::diverging(K::Text, nd)
        );
        // An unqualified reference matches on the name alone — the
        // announcing direction, since a file's search path is not read.
        assert_eq!(
            comparison_for("text", Some("icu_ci"), &[], &declared),
            ComparisonPlan::diverging(K::Text, nd)
        );
        // ... but a *qualified* reference must agree on the schema.
        assert_eq!(
            comparison_for("text", Some("elsewhere.icu_ci"), &[], &declared),
            ComparisonPlan::diverging(K::Text, named)
        );
        // An unqualified `"C"` is the built-in unless the dump declares one of
        // that name, which a search path it sets could put first (`KD48`):
        // then it is the weaker verdict, and never both branches at once.
        let c = ComparisonPlan::agrees(K::Text);
        assert_eq!(comparison_for("text", Some("\"C\""), &[], &declared), c);
        assert_eq!(
            comparison_for("text", Some("pg_catalog.\"C\""), &[], &[coll("public.\"C\"", true)]),
            c
        );
        assert_eq!(
            comparison_for("text", Some("\"C\""), &[], &[coll("public.\"C\"", true)]),
            ComparisonPlan::diverging(K::Text, named)
        );
        assert_eq!(
            comparison_for("text", Some("\"POSIX\""), &[], &[coll("public.\"POSIX\"", false)]),
            ComparisonPlan::diverging(K::Text, nd)
        );
    }

    /// The operator dimension, stated once: this is the only collation
    /// divergence `=`/`!=` ever sees.
    #[test]
    fn only_the_non_deterministic_collation_reaches_equality() {
        use ComparisonDivergence as D;
        for divergence in [D::AsText, D::UnmodelledType, D::NonDeterministicCollation] {
            assert!(divergence.affects_equality(), "{divergence:?}");
        }
        for divergence in [D::UnknownCollation, D::NonBytewiseCollation, D::JsonbStringCollation] {
            assert!(!divergence.affects_equality(), "{divergence:?}");
        }
    }

    /// A domain's own `COLLATE` is its type default, so a column of it with
    /// no clause inherits it — and a column-level clause overrides it, which
    /// is exactly the pair `pg_dump` writes a clause to express (I37).
    #[test]
    fn a_domain_s_collation_is_the_column_s_default_and_the_column_may_override() {
        let types = [
            ty(
                "public.dom_c",
                TypeKind::Domain {
                    base_type: "text".to_string(),
                    collation: Some("pg_catalog.\"C\"".to_string()),
                },
            ),
            ty("public.dom_plain", TypeKind::domain("text")),
            ty("public.dom_over_c", TypeKind::domain("public.dom_c")),
        ];
        let agrees_text = || ComparisonPlan::agrees(CompareKind::Text);
        let unknown =
            || ComparisonPlan::diverging(CompareKind::Text, ComparisonDivergence::UnknownCollation);
        let named = || {
            ComparisonPlan::diverging(CompareKind::Text, ComparisonDivergence::NonBytewiseCollation)
        };

        assert_eq!(comparison_for("public.dom_c", None, &types, &[]), agrees_text());
        assert_eq!(comparison_for("public.dom_plain", None, &types, &[]), unknown());
        // Through a chain, like every other domain answer.
        assert_eq!(comparison_for("public.dom_over_c", None, &types, &[]), agrees_text());
        // The column's clause wins over the domain's, both ways round.
        assert_eq!(
            comparison_for("public.dom_c", Some("pg_catalog.\"en_US.utf8\""), &types, &[]),
            named()
        );
        assert_eq!(
            comparison_for("public.dom_plain", Some("pg_catalog.\"C\""), &types, &[]),
            agrees_text()
        );
    }

    /// A domain compares as whatever it bottoms out at, through any chain —
    /// the same recursion `resolve_declared_type` makes, so a domain over
    /// `integer` orders numerically rather than as text.
    #[test]
    fn a_domain_compares_as_the_type_it_bottoms_out_at() {
        let types = [
            ty("public.d1", TypeKind::domain("integer")),
            ty("public.d2", TypeKind::domain("public.d1")),
            ty("public.dtext", TypeKind::domain("text")),
            ty("public.darr", TypeKind::domain("integer[]")),
            ty("public.dmoney", TypeKind::domain("money")),
        ];
        assert_eq!(comparison_for("public.d1", None, &types, &[]), agrees(CompareKind::Int));
        assert_eq!(comparison_for("public.d2", None, &types, &[]), agrees(CompareKind::Int));
        assert_eq!(
            comparison_for("public.dtext", None, &types, &[]),
            ComparisonPlan::diverging(CompareKind::Text, ComparisonDivergence::UnknownCollation),
        );
        // A domain over a nested type is that type's nested comparison,
        // through the same recursion.
        assert_eq!(
            comparison_for("public.darr", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Array(Box::new(NestedCompare::Leaf {
                declared: "integer".to_string(),
                kind: CompareKind::Int,
                divergence: None,
            }))),
        );
        // A domain over something with no order here has none either.
        assert_eq!(comparison_for("public.dmoney", None, &types, &[]), ComparisonPlan::Refused);
    }

    /// A walk visits each definition at most once along a path, so the list's
    /// length bounds it (I24): a chain using every entry still resolves, and a
    /// cycle — which only a hand-edited file or an unfollowed rename makes —
    /// answers the weaker `Unknown`, `Refused` and no extension rather than
    /// exhausting the stack.
    #[test]
    fn a_type_walk_is_bounded_by_the_definitions_the_file_declares() {
        let chain: Vec<TypeDef> = (0..64)
            .map(|i| {
                let base = if i == 0 { "uuid".to_string() } else { format!("public.d{}", i - 1) };
                ty(&format!("public.d{i}"), TypeKind::domain(&base))
            })
            .collect();
        assert_eq!(
            resolve_declared_type("public.d63", &chain),
            TypeOutcome::Mapped(DataType::FixedSizeBinary(16), NestedPlan::Scalar)
        );
        assert_eq!(comparison_for("public.d63", None, &chain, &[]), agrees(CompareKind::Uuid));
        assert_eq!(extension_for("public.d63", &chain), Some(CanonicalExtension::Uuid));

        let cycle = [
            ty("public.a", TypeKind::domain("public.b")),
            ty("public.b", TypeKind::domain("public.a")),
        ];
        assert_eq!(resolve_declared_type("public.a", &cycle), TypeOutcome::Unknown);
        // An array over one keeps its level, its element held as text.
        assert_eq!(
            resolve_declared_type("public.b[]", &cycle),
            TypeOutcome::Mapped(
                list_of(DataType::Utf8View),
                NestedPlan::Array(Box::new(NestedPlan::Scalar))
            )
        );
        assert_eq!(comparison_for("public.a", None, &cycle, &[]), ComparisonPlan::Refused);
        assert_eq!(extension_for("public.a", &cycle), None);

        // A composite holding itself terminates too, its innermost position
        // falling back to text as any unresolved one does.
        let holder = [ty(
            "public.holder",
            TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "public.holder[]")]) },
        )];
        assert!(matches!(
            resolve_declared_type("public.holder", &holder),
            TypeOutcome::Mapped(DataType::Struct(_), NestedPlan::Record(_))
        ));
        assert!(matches!(
            comparison_for("public.holder", None, &holder, &[]),
            ComparisonPlan::Nested(NestedCompare::Record(_))
        ));
    }

    /// A domain over an enum carries the enum's labels, through any chain —
    /// which is what lets `pgdt info --detail` list them beneath such a
    /// column and a `--filter` term name one. **No fixture column is one**,
    /// so this is the only check of it.
    ///
    /// A domain over an *empty* enum is refused like the enum itself: it
    /// resolves to no Arrow type, so no column of it is ever asked.
    #[test]
    fn a_domain_over_an_enum_compares_by_that_enum_s_declaration_order() {
        let labels = ["sad".to_string(), "ok".to_string(), "happy".to_string()];
        let types = [
            ty("public.mood", TypeKind::Enum { labels: labels.to_vec() }),
            ty("public.empty_enum", TypeKind::Enum { labels: Vec::new() }),
            ty("public.moodish", TypeKind::domain("public.mood")),
            ty("public.moodisher", TypeKind::domain("public.moodish")),
            ty("public.nothingish", TypeKind::domain("public.empty_enum")),
        ];
        let by_declaration = agrees(CompareKind::Enum(labels.iter().cloned().collect()));
        assert_eq!(comparison_for("public.moodish", None, &types, &[]), by_declaration);
        assert_eq!(comparison_for("public.moodisher", None, &types, &[]), by_declaration);
        assert_eq!(comparison_for("public.nothingish", None, &types, &[]), ComparisonPlan::Refused);
    }

    /// Every nested shape the register compares, as one statement: an array,
    /// a composite, a range and a range's multirange companion, each built by
    /// the same walk so that nesting composes with no special case. A base or
    /// shell type is refused for a reason of its own and is here to keep the
    /// two populations from being read as one.
    #[test]
    fn every_container_kind_compares_structurally() {
        let types = [
            ty(
                "public.point2d",
                TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
            ),
            ty(
                "public.myrange",
                TypeKind::Range {
                    subtype: Some("integer".to_string()),
                    multirange_type_name: Some("public.myrange_multi".to_string()),
                    canonical: None,
                },
            ),
            ty("public.gtype", TypeKind::Base),
            ty("public.forward", TypeKind::Shell),
        ];
        let int_leaf = || NestedCompare::Leaf {
            declared: "integer".to_string(),
            kind: CompareKind::Int,
            divergence: None,
        };
        let array_of_int = || ComparisonPlan::Nested(NestedCompare::Array(Box::new(int_leaf())));
        for declared in ["integer[]", "integer ARRAY"] {
            assert_eq!(comparison_for(declared, None, &types, &[]), array_of_int(), "{declared}");
        }
        // The element carries the column's own clause, because an array type
        // is not collatable and the clause is about its elements (I37).
        assert_eq!(
            comparison_for("text[]", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Array(Box::new(NestedCompare::Leaf {
                declared: "text".to_string(),
                kind: CompareKind::Text,
                divergence: Some(ComparisonDivergence::UnknownCollation),
            }))),
        );
        assert_eq!(
            comparison_for("text[]", Some("pg_catalog.\"C\""), &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Array(Box::new(NestedCompare::Leaf {
                declared: "text".to_string(),
                kind: CompareKind::Text,
                divergence: None,
            }))),
        );
        let point2d =
            || ComparisonPlan::Nested(NestedCompare::Record(vec![("x".to_string(), int_leaf())]));
        assert_eq!(comparison_for("public.point2d", None, &types, &[]), point2d());
        // Nesting composes with no special case: an array of composites is
        // the composite's own answer one level down.
        let ComparisonPlan::Nested(record) = point2d() else { unreachable!() };
        assert_eq!(
            comparison_for("public.point2d[]", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Array(Box::new(record))),
        );
        // A user-defined range and the companion multirange `pg_dump` writes
        // no `CREATE TYPE` for (I10) reach the same bound through two
        // different lookups, and neither canonicalizes.
        assert_eq!(
            comparison_for("public.myrange", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Range {
                bound: Box::new(int_leaf()),
                discrete: false,
            }),
        );
        assert_eq!(
            comparison_for("public.myrange_multi", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Multirange {
                bound: Box::new(int_leaf()),
                discrete: false,
            }),
        );
        // A built-in range is named nowhere in the file (I10), so its bound
        // and its canonicalization come off the hardcoded table — `discrete`
        // is a fact about the range type, so two ranges over `integer` differ.
        assert_eq!(
            comparison_for("int4range", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Range {
                bound: Box::new(int_leaf()),
                discrete: true,
            }),
        );
        assert_eq!(
            comparison_for("int4multirange", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Multirange {
                bound: Box::new(int_leaf()),
                discrete: true,
            }),
        );
        assert!(matches!(
            comparison_for("numrange", None, &types, &[]),
            ComparisonPlan::Nested(NestedCompare::Range { discrete: false, .. })
        ));
        // A domain over a range is the range's own answer, through the same
        // recursion every domain takes.
        let rangedom =
            [types[1].clone(), ty("public.rangedom", TypeKind::domain("public.myrange"))];
        assert_eq!(
            comparison_for("public.rangedom", None, &rangedom, &[]),
            comparison_for("public.myrange", None, &types, &[]),
        );
        // A range whose DDL stated no subtype has no bound type to name a
        // refusal after, so the column is refused outright.
        let subtypeless = [ty(
            "public.opaquerange",
            TypeKind::Range { subtype: None, multirange_type_name: None, canonical: None },
        )];
        assert_eq!(
            comparison_for("public.opaquerange", None, &subtypeless, &[]),
            ComparisonPlan::Refused,
        );
        for declared in ["public.gtype", "public.forward", "public.nosuchtype"] {
            assert_eq!(
                comparison_for(declared, None, &types, &[]),
                ComparisonPlan::Refused,
                "{declared}"
            );
        }
    }

    /// A range type declaring a `canonical` function is the one answer that
    /// is neither a comparison nor a bytewise fallback: PostgreSQL rewrites
    /// every value through arbitrary server-side code before storing or
    /// comparing one (I46), so both operator families would be *wrong* rather
    /// than weak.
    ///
    /// **Unanswerability propagates where comparability is inherited**: such a
    /// range short-circuits out of every walk that reaches it — an array of
    /// it, a composite holding one, its multirange companion, a domain over
    /// it — and each answers for the column instead.
    #[test]
    fn a_range_declaring_a_canonical_function_answers_no_operator() {
        let types = [
            ty(
                "public.canonrange",
                TypeKind::Range {
                    subtype: Some("integer".to_string()),
                    multirange_type_name: Some("public.canonrange_multi".to_string()),
                    canonical: Some("public.canonrange_canonical".to_string()),
                },
            ),
            ty(
                "public.holder",
                TypeKind::Composite {
                    fields: Some(vec![
                        ColumnDef::new("id", "integer"),
                        ColumnDef::new("span", "public.canonrange"),
                    ]),
                },
            ),
            ty("public.canondom", TypeKind::domain("public.canonrange")),
        ];
        // The refusal names the *range* type and its function, whichever
        // route reached it: the companion has no DDL of its own to name (I10)
        // and a position inside a container is not where the parameter sits.
        let unanswerable = || {
            ComparisonPlan::Unanswerable(UnanswerableReason::RangeCanonical {
                range_type: "public.canonrange".to_string(),
                function: "public.canonrange_canonical".to_string(),
            })
        };
        for declared in [
            "public.canonrange",
            "public.canonrange_multi",
            "public.canonrange[]",
            "public.holder",
            "public.holder[]",
            "public.canondom",
            "public.canondom[]",
        ] {
            assert_eq!(comparison_for(declared, None, &types, &[]), unanswerable(), "{declared}");
        }
        // Not an order, which is what the four ordering operators ask, and
        // not a `Refused` either, which is what lets `=` be refused with it.
        assert!(!unanswerable().orders());
        assert_ne!(unanswerable(), ComparisonPlan::Refused);
        // The same range without the parameter is an ordinary bound-wise
        // comparison, so the answer turns on the DDL and on nothing else.
        let TypeKind::Range { subtype, multirange_type_name, .. } = types[0].kind.clone() else {
            unreachable!()
        };
        let plain = [
            ty(
                "public.canonrange",
                TypeKind::Range { subtype, multirange_type_name, canonical: None },
            ),
            types[1].clone(),
            types[2].clone(),
        ];
        assert!(comparison_for("public.canonrange", None, &plain, &[]).orders());
        assert!(comparison_for("public.holder[]", None, &plain, &[]).orders());
    }

    /// Comparability is inherited: a position whose declared type has no
    /// order refuses the whole column, and the tree says which position and
    /// which type so the refusal can name them.
    ///
    /// **`json` is the case that is not obvious**: bytewise at top level, and
    /// inside a container nothing at all, because `array_cmp` looks up the
    /// element type's comparison proc and there is none.
    #[test]
    fn a_position_with_no_order_refuses_the_whole_column() {
        let types = [
            ty(
                "public.jsonpair",
                TypeKind::Composite {
                    fields: Some(vec![
                        ColumnDef::new("ok", "integer"),
                        ColumnDef::new("doc", "json"),
                    ]),
                },
            ),
            ty("public.gtype", TypeKind::Base),
        ];
        for (declared, path, at, announces) in [
            // `json` takes the server's `=` away with its order, so the
            // column's bytewise fallback is an answer it does not have.
            ("json[]", "[]", "json", true),
            ("public.jsonpair", ".doc", "json", true),
            ("public.jsonpair[]", "[].doc", "json", true),
            // I22: an opaque element type, refused for the delimiter as much
            // as for the order.
            ("box[]", "[]", "box", false),
            ("public.gtype[]", "[]", "public.gtype", false),
            // An element this walk cannot find, `types` declaring no
            // `public.intarr`: refused as an unknown type. The fixture's own
            // `public.intarr`, an array element that is itself an array
            // (I26), is the oracle walk's (`predicate.rs`'s `REFUSED`).
            ("public.intarr[]", "[]", "public.intarr", false),
        ] {
            let plan = comparison_for(declared, None, &types, &[]);
            assert!(!plan.orders(), "{declared}");
            let ComparisonPlan::Nested(tree) = plan else { panic!("{declared}: not nested") };
            assert_eq!(tree.uncomparable(), Some((path.to_string(), at.to_string())), "{declared}");
            // The three refused for a reason of *this build's* announce
            // nothing: PostgreSQL orders an array of arrays through
            // `array_ops` and compares `box[]` element-wise.
            let expected: Vec<_> = announces
                .then(|| (path.to_string(), at.to_string(), ComparisonDivergence::AsText))
                .into_iter()
                .collect();
            assert_eq!(tree.divergences(), expected, "{declared}");
        }
    }

    /// Every diverging position, in walk order, each with the path and the
    /// declared type its sentence needs — a composite on the database's
    /// collation twice, once directly and once through an element.
    #[test]
    fn a_nested_column_announces_every_diverging_position() {
        let types = [ty(
            "public.tagged",
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("label", "text"),
                    ColumnDef::new("n", "integer"),
                    ColumnDef::new("tags", "text[]"),
                ]),
            },
        )];
        let ComparisonPlan::Nested(tree) = comparison_for("public.tagged", None, &types, &[])
        else {
            panic!("a composite compares structurally")
        };
        assert_eq!(
            tree.divergences(),
            [
                (".label".to_string(), "text".to_string(), ComparisonDivergence::UnknownCollation),
                (".tags[]".to_string(), "text".to_string(), ComparisonDivergence::UnknownCollation),
            ],
        );
        // A field's own clause is what the register reads, not the column's:
        // a composite is not collatable, so there is no column clause to
        // inherit.
        let types = [ty(
            "public.pair",
            TypeKind::Composite {
                fields: Some(vec![
                    ColumnDef::new("plain", "text"),
                    ColumnDef {
                        name: "c".to_string(),
                        declared_type: "text".to_string(),
                        collation: Some("pg_catalog.\"C\"".to_string()),
                    },
                ]),
            },
        )];
        let ComparisonPlan::Nested(tree) = comparison_for("public.pair", None, &types, &[]) else {
            panic!("a composite compares structurally")
        };
        assert_eq!(
            tree.divergences(),
            [(".plain".to_string(), "text".to_string(), ComparisonDivergence::UnknownCollation)],
        );
    }

    /// `int2vector` is the one built-in whose Arrow type is a container and
    /// whose declaration is not spelled like one, so its three answers are
    /// pinned together: the type, the literal form, and the comparison.
    #[test]
    fn int2vector_is_a_list_with_a_literal_form_and_an_order_of_its_own() {
        let TypeOutcome::Mapped(data_type, plan) = resolve_declared_type("int2vector", &[]) else {
            panic!("int2vector maps")
        };
        assert_eq!(data_type, list_of(DataType::Int16));
        assert_eq!(plan, NestedPlan::Int2Vector);
        // The same Arrow type through the array spelling, and a different
        // literal form — the reason the plan travels beside the type.
        let TypeOutcome::Mapped(array_type, array_plan) = resolve_declared_type("smallint[]", &[])
        else {
            panic!("smallint[] maps")
        };
        assert_eq!(array_type, data_type);
        assert_eq!(array_plan, NestedPlan::Array(Box::new(NestedPlan::Scalar)));

        let comparison = comparison_for("int2vector", None, &[], &[]);
        assert_eq!(comparison, ComparisonPlan::Nested(NestedCompare::Int2Vector));
        assert!(comparison.orders());
        let ComparisonPlan::Nested(tree) = &comparison else { panic!("nested") };
        assert_eq!(tree.uncomparable(), None);
        assert_eq!(tree.divergences(), []);

        // An array *of* them composes like any other element type: the outer
        // literal is `array_out`'s and each element is a vector.
        let ComparisonPlan::Nested(outer) = comparison_for("int2vector[]", None, &[], &[]) else {
            panic!("int2vector[] compares structurally")
        };
        assert_eq!(outer, NestedCompare::Array(Box::new(NestedCompare::Int2Vector)));
    }
}
