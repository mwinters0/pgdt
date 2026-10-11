//! **The comparison register**: how a column of a declared type compares,
//! and whether that is the order PostgreSQL itself defines
//! (`docs/design/decisions.md`, "D40") — the plans, the declared collation,
//! and where DataFusion's comparison of the value a column emits diverges. A
//! value's place in that order is [`crate::key`]'s.
//!
//! Pure, synchronous, no I/O (`docs/design/decisions.md`, "D74"). The
//! register walks a declared type as [`crate::pgtype`] maps it, through the
//! one built-in table that answers both (`pgtype::builtin_scalar`) and the
//! database's own `CREATE TYPE`/`DOMAIN` list, so the two answers are reached
//! through one spelling of the same string. A position it orders by nothing
//! carries the input grammar a strict parse reads it by
//! ([`crate::grammar`]), the namespace declaring its types.

mod walk;

use std::sync::Arc;

use pgdt_core::internal::Cursor;
use pgdt_model::{BoundsSet, CollationDef};

use self::divergence::datafusion_position_divergences;
pub use self::walk::comparison_for;
use crate::declared::{IntervalQualifier, NumericTypmod, Position};
use crate::grammar::TextGrammar;

/// How one column's field text becomes a value two sides of a comparison can
/// be ordered by — the decoding half of a [`ComparisonPlan`], and the only
/// thing `crate::key` needs in order to read a side.
///
/// A small closed vocabulary rather than the Arrow type, which does not
/// correspond to it (`docs/design/decisions.md`, "D40").
///
/// **Not `Copy`**: several variants carry the column's own facts — an enum's
/// labels, a typmodded `numeric`'s precision and scale, whether one held as
/// text admits the infinities — and the labels are not `Copy`.
/// The kind is cloned into a term as the term's literal is resolved
/// (`pgdt_query::predicate`'s `Comparison::Decoded`), once per term.
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
    /// blanks ([`char_typmod_refuses`](crate::value::text::char_typmod_refuses)); `None` for every
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
    /// ([`interval_reads`](crate::value::datetime::grammar::interval_reads));
    /// `None` for a column with none, and for a literal's reading, which no
    /// typmod reaches. It refuses a field and never moves a key.
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

/// What a position diverges in from PostgreSQL's comparison when DataFusion
/// compares it: not the register's API but what a crate above calls, so
/// `internal` re-exports it.
pub(crate) mod divergence {
    use super::{CompareKind, ComparisonDivergence};

    /// What a position of `kind` diverges in under
    /// [`ComparisonSemantics::DataFusion`](super::ComparisonSemantics::DataFusion),
    /// given the register's own `divergence` for it:
    /// [`CompareKind::datafusion_divergence`], then the register's, but for
    /// `jsonb`'s string collation, the whole value being compared as text there.
    pub fn datafusion_position_divergences(
        kind: &CompareKind,
        divergence: Option<ComparisonDivergence>,
    ) -> Vec<ComparisonDivergence> {
        kind.datafusion_divergence()
            .into_iter()
            .chain(divergence.filter(|d| *d != ComparisonDivergence::JsonbStringCollation))
            .collect()
    }
}

/// How a comparison here differs from PostgreSQL's own for the same declared
/// type. The declared type then sharpens the sentence a user reads, since
/// several declared types share one divergence for different reasons (see
/// `pgdt_query::ComparisonNote::message`).
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
    /// database's collation, in its `CREATE DATABASE`, which pgdt-dump's `ddl.rs`
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
    /// `pgdt_query::column_divergences`, never by the register — as
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
pub(crate) enum TypeCollation {
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
    /// `grammar` is the position's input grammar where this build reads one for
    /// a type it orders by nothing
    /// ([`text_grammar`](crate::grammar::input::text_grammar)), and `delimiter` its
    /// type's `typdelim`, which an array of it is split at
    /// ([`typdelim`](crate::declared::typdelim)).
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

/// What a crate above calls on [`Discrete`] that is not the register's API,
/// so `internal` re-exports it.
pub(crate) mod discrete {
    use super::Discrete;

    /// What a crate above calls on [`Discrete`] that is not its API: public
    /// only because a crate boundary forced it, and reached through
    /// `internal`.
    pub trait DiscreteExt {
        fn largest(self) -> i64;
    }

    impl DiscreteExt for Discrete {
        /// The subtype's largest value, in the key a bound of it is read
        /// into: a bound here has no successor, and the canonical function
        /// raises `integer`, `bigint` or `date out of range` on it (I46). A
        /// leaf's key is an `i64` whatever the column's width, so the width
        /// is stated here rather than found in the key. A literal's bound past
        /// the width itself, `int4range '[1,3000000000)'`, is refused before
        /// any successor is taken, as `int4in` refuses it (I60); a field's is
        /// read as written.
        ///
        /// A date's is `5874897-12-31` as days since 1970, the day before
        /// `IS_VALID_DATE`'s exclusive `DATE_END_JULIAN`.
        fn largest(self) -> i64 {
            match self {
                Discrete::Int4 => i64::from(i32::MAX),
                Discrete::Int8 => i64::MAX,
                Discrete::Date => 2_145_042_905,
            }
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
    pub(crate) fn declared(&self) -> &str {
        match self {
            Self::Leaf { declared, .. } | Self::Uncomparable { declared, .. } => declared,
            _ => "int2vector",
        }
    }

    /// Depth-first over every leaf — [`Self::Leaf`], [`Self::Uncomparable`]
    /// or [`Self::Int2Vector`] — handing each its path and the node.
    pub(crate) fn walk(&self, path: &mut String, visit: &mut impl FnMut(&str, &Self)) {
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
/// consumed by `pgdt_query::predicate`, which reads it instead of inspecting the
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
    pub(crate) fn agrees(kind: CompareKind) -> Self {
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
pub(crate) fn collated_text(
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

#[cfg(test)]
mod tests;
