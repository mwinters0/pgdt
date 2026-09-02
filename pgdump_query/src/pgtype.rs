//! Declared PostgreSQL type string -> Arrow `DataType`
//! (`docs/design/architecture.md`, "Type resolution").
//!
//! Pure, synchronous, no I/O — see `docs/design/layering.md`, L2. A declared
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

/// The outcome of mapping one declared type string, mirroring
/// [`crate::resolve::ColumnResolution`] but at single-type granularity.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeOutcome {
    /// The dump alone determines the value; this is the Arrow type it maps
    /// to (`Utf8View` included — e.g. `text`, `interval`, are deliberately
    /// mapped there, not merely defaulted), paired with the [`NestedPlan`]
    /// that says which literal form fills it.
    ///
    /// **The pair has one producer.** Nothing outside this module builds
    /// either half of a nested column's pairing, which is what keeps the two
    /// trees in agreement without a third structure enforcing it.
    Mapped(DataType, NestedPlan),
    /// A declared type string this build has no mapping for at all — neither
    /// a built-in, nor found in the database's `CREATE TYPE`/`DOMAIN` list,
    /// nor a composite whose declared field list survived parsing
    /// ([`TypeKind::Composite`] is all-or-nothing).
    Unknown,
    /// An array whose element type is opaque by construction — `box`, a
    /// C-level base type, or a shell type, through any chain of domains.
    ///
    /// Refused rather than mapped to `List<Utf8View>` because the array
    /// separator is the *element type's* `typdelim` (I22) and `box`'s is
    /// `;`: splitting such a literal on `,` silently invents element
    /// boundaries, and the elements it would recover are opaque text
    /// anyway. Held apart from [`Self::OpaqueBaseType`] so `pgdq info` can
    /// say which of the two happened.
    OpaqueElementType,
    /// An array whose element type is *itself* an array, through any chain of
    /// domains — `CREATE DOMAIN d AS integer[]` and a column of `d[]`, the
    /// only DDL shape there is for it (I26). `integer[][]` is **not** one: it
    /// is a spelling of `integer[]`, whose element is `integer` (I28), and it
    /// resolves like any other array.
    ///
    /// Refused for the same reason as [`Self::OpaqueElementType`]: the
    /// element's own literal grammar is not what the outer split assumes.
    /// The value is written **one brace deep** — `{"{1,2}","{3}"}`, since
    /// `array_out` force-quotes any element whose text contains `{` (I25) —
    /// so the literal's leading brace run and the column's resolved `List`
    /// depth are independent for this shape alone, and
    /// `NestedPlan::Array(Array(…))` would have to mean both *one literal,
    /// two dimensions* and *one literal whose elements are literals*. Held
    /// apart from `OpaqueElementType` because the label would lie:
    /// `integer[]` is not opaque, it is understood and declined.
    NestedArrayElement,
    /// A C-level base type or a shell/undefined type — genuinely
    /// information-free, not merely unimplemented (see the phase doc's
    /// "`CREATE TYPE`: six emitted forms").
    OpaqueBaseType,
    /// A `CREATE TYPE ... AS ENUM ()` with no labels at all.
    EmptyEnum,
}

/// Which PostgreSQL literal form fills a resolved Arrow type, at every
/// position in it.
///
/// **The Arrow type alone cannot say.** `int4range[]` and `int4multirange`
/// both resolve to `List<Struct{lower, upper, …}>`, and they are written
/// differently — `{"[1,10)","[2,3)"}` with array quoting versus `{[1,10),
/// [2,3)}` with none at all. A composite that happens to have the range
/// struct's five fields is the same collision one level down. So "which of
/// [`crate::nested`]'s codecs applies here" travels beside the `DataType`
/// rather than being inferred from it, and it is a tree because the answer
/// differs per nesting level.
///
/// A `Scalar` leaf is anything [`crate::decode`] handles (`Utf8View`
/// included), which is where every branch bottoms out.
/// `Serialize` so `pgdq info --json` can export a resolved schema's plans
/// structurally rather than inventing a second spelling for them
/// (`docs/design/architecture.md`, "CLI surface"). **Not `Deserialize`, and
/// never persisted**: the cache holds what the dump said, never what we
/// concluded (`docs/design/layering.md`, rule 5).
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub enum NestedPlan {
    /// Filled by `crate::decode`'s per-type decoders, or held as text.
    #[default]
    Scalar,
    /// `array_out` → `List<child>`. Nested `Array`s are the multi-dimensional
    /// case, and **only** that: the plan's depth is the dimensionality the
    /// column was resolved at, and a value that disagrees is a decode
    /// failure. The one declared shape whose literal would contradict that —
    /// an array whose element type is an array (I26) — is refused at
    /// resolution as [`TypeOutcome::NestedArrayElement`], so a nested `Array`
    /// can only ever come from the shape census.
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
}

/// How one column's field text becomes a value two sides of a comparison can
/// be ordered by — the decoding half of a [`ComparisonPlan`], and the only
/// thing `crate::predicate` needs in order to read a side.
///
/// It is a small closed vocabulary rather than the Arrow type because the two
/// do not correspond: `text`, bare `numeric`, `interval` and `inet` all reach
/// `Utf8View` and are four different comparisons, while `Decimal128` and
/// `Decimal256` are one.
///
/// **Not `Copy`**, because two of its variants carry the column's own facts:
/// an enum's labels, and whether a `numeric` column's typmod excludes the
/// infinities. Nothing on the per-row path clones one — the kind is cloned
/// once, into the `OrderTerm` the block's resolution builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompareKind {
    Bool,
    Int,
    /// An unsigned 32-bit integer — `oid`. Held apart from [`Self::Int`]
    /// because the two differ on a *literal* carrying a minus sign, which
    /// `oidin` wraps and this refuses: the values order identically, so
    /// sharing the arm would order correctly and accept a literal it must
    /// not.
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
    /// sides give up their trailing blanks. A dump writes every value of such
    /// a column padded to `n` and every `bpchar` comparison calls `bcTruelen`
    /// on both operands first (I38), so the padding is not part of the value
    /// and trimming it is what makes this the same comparison the server
    /// makes.
    ///
    /// Held apart from [`Self::Text`] rather than carried as a flag on it
    /// because it is a different comparison, not a different column: the
    /// register has one arm per declared type and `character` is that arm.
    /// The trim is a reverse scan for `0x20` yielding a shorter slice — no
    /// allocation and no decode — which is why it is admissible on the
    /// per-row path where a decode would not be.
    ///
    /// It is collatable exactly as [`Self::Text`] is, and for the same
    /// reason: `bpcharcmp` hands the two trimmed strings to `varstr_cmp`
    /// under the column's collation, so what the clause decides here is the
    /// verdict, never the comparison.
    PaddedText,
    /// Arbitrary-precision decimal read straight out of the text the file
    /// holds — a bare `numeric`, or one whose declared precision is past
    /// `Decimal256`'s 76 digits. Held apart from [`Self::Decimal`] because
    /// there is no scale to carry both sides to: `1.5` and `1.50` are one
    /// value written two ways, and the comparison normalizes rather than
    /// rescales.
    ///
    /// `infinities` says whether `Infinity`/`-Infinity` are values of the
    /// column. Any typmod rejects an infinity (I34), so only the bare form
    /// admits the spelling — the same shape as [`Self::UnsignedInt`], where a
    /// variant exists to refuse a *literal* the values themselves could never
    /// take.
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
    /// bits, which is why PostgreSQL's own comparison uses them.
    ///
    /// Carries the two infinities unconditionally, in `date_out`'s spellings
    /// rather than `numeric_out`'s (I34). They are v17 values, and reading
    /// them on an older file is the union rule
    /// (`docs/design/roadmap-P11-typed-predicates.md`, "Version-varying
    /// semantics"): no v13 server could have written one, so nothing is
    /// misread by a build that understands them.
    Interval,
    /// `time with time zone`, compared by the UTC-equivalent instant first
    /// and by the stored zone second, so two values are equal only when both
    /// halves are (I40) — `00:00:00+00` and `01:00:00+01` are the same
    /// instant and are *not* equal.
    TimeTz,
    /// `inet` and `cidr`, compared by `network_cmp_internal`: family, then
    /// the shorter netmask's worth of address bits, then the netmask length,
    /// then the whole address (I40). Not a byte order — a `/8` and a `/16`
    /// that agree on their first eight bits are ordered by the netmask, not
    /// by the bytes below it.
    ///
    /// `cidr` says so, because that is the only difference between the two:
    /// `cidr_in` refuses a value with a bit set below its netmask and
    /// `inet_in` accepts one, and refusing that literal is the same shape as
    /// [`Self::UnsignedInt`]'s.
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
    /// differ — the *kind* at that position first (an object outranks an
    /// array, an array outranks every scalar, a boolean outranks a number),
    /// then a container's element or pair count, then the members themselves
    /// (I41).
    ///
    /// **The one thing it cannot reproduce is a string leaf.** Every JSON
    /// string, object keys included, is ordered by `varstr_cmp` under
    /// `DEFAULT_COLLATION_OID` — the *database's* collation, which a plain
    /// dump does not record (I32) — so a `jsonb` column carries
    /// [`ComparisonDivergence::JsonbStringCollation`] for exactly the reason a
    /// bare `text` column carries [`ComparisonDivergence::UnknownCollation`],
    /// one level down. A `jsonb` column cannot state a clause of its own:
    /// `jsonb` is not a collatable type, so there is nothing for `pg_dump` to
    /// write and nothing for the register to read.
    Jsonb,
}

/// How a comparison here differs from PostgreSQL's own for the same declared
/// type. The declared type then sharpens the sentence a user reads, since
/// several declared types share one divergence for different reasons (see
/// `crate::predicate::ComparisonNote::message`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonDivergence {
    /// The column is held as text and compared bytewise where the server has
    /// no comparison at all. **`json` is its one member**, and the sentence
    /// it prints says exactly that: PostgreSQL defines no `=`, no order and
    /// no operator class for `json`, so bytewise offers *more* than the
    /// server does rather than less, and "agrees with PostgreSQL" is not a
    /// question the type can be asked.
    ///
    /// Every other text-held type now carries a comparison of its own, and
    /// every collatable one carries one of the two collation variants below.
    AsText,
    /// A collatable text column whose collation the file does not state: it
    /// carries no `COLLATE` clause and its type's default collation is the
    /// *database's*, which a plain dump never records (I32). Bytewise is
    /// PostgreSQL's answer only if that collation is `C`/`POSIX`.
    UnknownCollation,
    /// A collatable column that states a collation other than `C`/`POSIX`,
    /// whose order this build does not implement.
    ///
    /// Deficiency register: `deficiency: KD7` — this and
    /// [`Self::NonDeterministicCollation`] are the two register rows whose
    /// divergence the file gives enough information to close and this build
    /// does not, and the detail is `docs/design/architecture.md`'s "Ordering
    /// operators compare typed". The two collation variants either side of
    /// them are *not* that: [`Self::UnknownCollation`] names a fact no plain
    /// dump carries (I32), and a stated collation that is bytewise in fact but
    /// not named `C`/`POSIX` lands here with correct rows and a spurious note.
    NonBytewiseCollation,
    /// A collatable column stating a collation the *same dump* declares
    /// `deterministic = false` (I42). It is [`Self::NonBytewiseCollation`]
    /// plus one fact: under a non-deterministic collation `texteq`/`bpchareq`
    /// are not byte comparisons either, so two values that differ byte for
    /// byte can be equal to the server and this build calls them distinct.
    ///
    /// **It is the only divergence a plain dump states outright.** The
    /// collation's *order* still needs a provider version the file does not
    /// carry, but `pg_dump` writes `, deterministic = false` unconditionally
    /// (I42) — so where the other three collation variants are announcements
    /// about what the file leaves unsaid, this one repeats what it said.
    ///
    /// Deficiency register: `deficiency: KD7` — same defect, same fix (a
    /// comparison per named collation), one operator further.
    NonDeterministicCollation,
    /// A `jsonb` column, whose *structure* is compared exactly and whose
    /// string leaves and object keys are not: `compareJsonbScalarValue` orders
    /// every one of them by `varstr_cmp` under `DEFAULT_COLLATION_OID` (I41),
    /// which is the database's collation and is absent from a plain dump
    /// (I32). Held apart from [`Self::UnknownCollation`] because the column
    /// states nothing and could not — `jsonb` is not collatable, so the
    /// sentence about a missing `COLLATE` clause would be describing a clause
    /// that has no place to be written.
    JsonbStringCollation,
    /// A column whose declared type resolved, is not nested, and has no
    /// comparison in this register at all — `box`, `money`, `xml`, a
    /// user-defined base type. An ordering operator is *refused* on such a
    /// column ([`ComparisonPlan::Refused`]); `=`/`!=` are not, because the
    /// text a dump holds is canonical `*_out` form and a byte comparison over
    /// it is right for most of these types. This says it is not right for all
    /// of them.
    ///
    /// **`box` is the member that proves it**, and the committed oracle holds
    /// the proof: `box_eq` compares *areas*, so the server calls
    /// `(1,1),(0,0)` and `(3,3),(2,2)` equal and a byte comparison does not
    /// (`fixtures/<13-18>/oracle/comparisons.tsv`, `public.box_domain`).
    /// Whether any given unmodelled type is like `box` or like `money` —
    /// whose `=` is its value's and whose `*_out` is unique per value, so
    /// bytewise agrees — is exactly what this register does not know, and
    /// announcing is the conservative answer.
    ///
    /// Deficiency register: `deficiency: KD10`.
    ///
    /// *Rejected: refusing `=` here as ordering is refused.* It takes a
    /// working capability away from every type in the group to protect the
    /// geometric handful, where an announcement protects both — and for
    /// `xml`, whose `=` PostgreSQL does not define at all, filtering by exact
    /// text is a thing a user legitimately wants.
    UnmodelledType,
}

impl ComparisonDivergence {
    /// Whether this divergence reaches `=`/`!=` as well as the four ordering
    /// operators.
    ///
    /// **Determinism is what decides it for the collation variants**, and it
    /// is why three of the four answer `false`: a *deterministic* collation
    /// makes `varstr_cmp` return zero exactly when the bytes are equal, so
    /// `texteq`/`bpchareq` are byte comparisons whatever that collation
    /// otherwise orders. A collation the file does not name, one it names and
    /// this build does not implement, and one reached through a `jsonb` string
    /// leaf are therefore divergences of *order* alone — the row set an `=`
    /// returns is the server's either way, because every libc collation is
    /// deterministic and a non-deterministic one is stated outright.
    ///
    /// [`Self::NonDeterministicCollation`] is that statement, and the one
    /// collation variant that reaches `=` (I42). The other two `true` answers
    /// are not about a collation at all: the server defines no comparison
    /// ([`Self::AsText`]), or this register models none
    /// ([`Self::UnmodelledType`]).
    ///
    /// There is no `affects_ordering` beside this: every variant does, which
    /// is what makes one method enough.
    pub fn affects_equality(self) -> bool {
        match self {
            // PostgreSQL defines no `=` for `json` any more than it defines
            // an order, so bytewise equality is as much an answer the server
            // does not have as the ordering is.
            Self::AsText | Self::UnmodelledType => true,
            // The dump itself says `texteq` is not a byte comparison here.
            Self::NonDeterministicCollation => true,
            Self::UnknownCollation | Self::NonBytewiseCollation | Self::JsonbStringCollation => {
                false
            }
        }
    }
}

/// A collatable type's *default* collation — `pg_type.typcollation`, which is
/// what `pg_dump` compares a column's collation against when deciding whether
/// to write a `COLLATE` clause at all (I37). So the absence of a clause means
/// this, and the two values mean very different things.
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
/// server's way — a `text` element with no `COLLATE` clause — carries its own
/// [`ComparisonDivergence`], so a `text[]` column diverges for the reason its
/// element does, one level down.
///
/// **`json` beneath a nested type is a refusal, not a divergence.** At top
/// level [`ComparisonDivergence::AsText`] means "bytewise, where the server
/// orders not at all", which is more than the server offers rather than less.
/// Inside a container it is not available at all: `array_cmp` looks up the
/// element type's comparison proc and raises when there is none, so a
/// `json[]` column and a composite with a `json` field have no `=` and no `<`
/// on the server either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NestedCompare {
    /// A scalar position: the declared type as the DDL spelled it, and the
    /// comparison a column of it would have had.
    Leaf { declared: String, kind: CompareKind, divergence: Option<ComparisonDivergence> },
    /// A position whose declared type has no order here — what refuses the
    /// column, and what lets the refusal name the type that caused it.
    Uncomparable { declared: String },
    /// `array_cmp`: elements first, up to the shorter array's length, then
    /// element count, dimension count, dimensions and lower bounds (I45).
    /// **One node whatever the dimensionality** — an `array_out` literal
    /// carries its own shape and [`crate::nested::ArrayLiteral`] flattens it,
    /// so `integer[]` is one `Array` node whether its values are vectors or
    /// matrices.
    Array(Box<NestedCompare>),
    /// `record_cmp`: field-wise in declaration order, which is also the order
    /// `record_out` writes them in. The name is carried for the diagnostic
    /// path alone — `record_out` is positional (I23) and no comparison reads
    /// it.
    Record(Vec<(String, NestedCompare)>),
}

impl NestedCompare {
    /// The first position beneath this one with no order, as
    /// `(path, declared type)` — `None` when every position is comparable.
    /// The path is the accessor a user would write: `[]` for an element,
    /// `.name` for a field, appended as the walk descends.
    pub fn uncomparable(&self) -> Option<(String, String)> {
        let mut found = None;
        self.walk(&mut String::new(), &mut |path, declared, divergence| {
            if divergence.is_none() && found.is_none() {
                found = Some((path.to_string(), declared.to_string()));
            }
        });
        found
    }

    /// Every position beneath this one that compares but not the server's
    /// way, as `(path, declared type, divergence)`, in walk order. A column
    /// carrying two of them — a composite with a bare `text` field and a
    /// `text[]` one — announces both.
    pub fn divergences(&self) -> Vec<(String, String, ComparisonDivergence)> {
        let mut out = Vec::new();
        self.walk(&mut String::new(), &mut |path, declared, divergence| {
            if let Some(Some(divergence)) = divergence {
                out.push((path.to_string(), declared.to_string(), divergence));
            }
        });
        out
    }

    /// Depth-first over every leaf, handing each its path, its declared type
    /// and its divergence — `None` for an [`Self::Uncomparable`] one, which
    /// is how the two callers above tell "no order" from "an order that
    /// differs".
    fn walk(
        &self,
        path: &mut String,
        visit: &mut impl FnMut(&str, &str, Option<Option<ComparisonDivergence>>),
    ) {
        match self {
            Self::Leaf { declared, divergence, .. } => visit(path, declared, Some(*divergence)),
            Self::Uncomparable { declared } => visit(path, declared, None),
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
        }
    }
}

/// **The comparison register's answer for one declared type**: how a column
/// of it compares, and whether that is the order PostgreSQL itself defines.
/// Rendered as a table in `docs/design/architecture.md`, "Ordering operators
/// compare typed".
///
/// **One fact, not two.** "This type has no order here" and "there is no way
/// to decode a value of it" are the same statement, so they are one variant
/// rather than a pairing that could come to disagree.
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
    /// No order is defined here for this declared type.
    #[default]
    Refused,
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
            Self::Refused => false,
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
/// **The schema is checked, not just the name.** Nothing stops a user
/// creating a collation called `"C"` in another schema, and answering
/// "agrees" for it would be the one direction of error this register must not
/// make. Anything else — a libc locale, an ICU collation, `ucs_basic`,
/// a name this cannot parse — is not bytewise as far as this build is
/// concerned.
fn collation_is_bytewise(reference: &str) -> bool {
    let Some((schema, name)) = collation_parts(reference) else { return false };
    let known_schema = schema.as_deref().is_none_or(|s| s == "pg_catalog");
    known_schema && (name == "C" || name == "POSIX")
}

/// The comparison for a collatable type, given its bytewise comparison
/// `kind`, the column's own `COLLATE` clause (`None` for a column that
/// carries none) and the type's default collation.
///
/// **`kind` is the comparison in every case; only the verdict moves.** That
/// is the whole of what reading the clause buys: `text COLLATE "C"` and a
/// bare `name` are told they agree, where before every text column was told
/// it diverged (`docs/design/architecture.md`, "Ordering operators compare
/// typed").
///
/// `kind` exists because `character(n)` joins this rule with a comparison of
/// its own: `bpcharcmp` trims both operands' trailing blanks and *then*
/// consults the collation (I38), so the trim is orthogonal to the clause and
/// the three collation arms are the same three.
fn collated_text(
    kind: CompareKind,
    collation: Option<&str>,
    type_default: TypeCollation,
    collations: &[CollationDef],
) -> ComparisonPlan {
    // First, because it is the strongest thing the file can say about a
    // collation: the other three branches are all read off the *name*, where
    // this one is read off a statement the dump wrote. The order costs nothing
    // in practice — a non-deterministic collation is ICU-only and therefore
    // user-defined (I42), and `collation_is_bytewise` answers `true` only for
    // `pg_catalog."C"`/`"POSIX"`, which no user-defined collation can be — so
    // the two can never both match. It is put first anyway, so that reading
    // the branches top to bottom is reading them in order of evidence.
    if collation.is_some_and(|reference| states_non_deterministic(reference, collations)) {
        return ComparisonPlan::diverging(kind, ComparisonDivergence::NonDeterministicCollation);
    }
    let bytewise = match collation {
        Some(reference) => collation_is_bytewise(reference),
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
/// **Both sides are parsed rather than compared as text**, because the two
/// spellings come from different `pg_dump` code paths and need not match byte
/// for byte: a `CREATE COLLATION` names the object and a `COLLATE` clause
/// references it, and either may quote an identifier the other leaves bare.
/// [`collation_parts`] folds an unquoted identifier the way the server does,
/// so the join is on the same names the server would resolve.
///
/// **A reference with no schema matches on the name alone.** `pg_dump` writes
/// both sides schema-qualified for a user-defined collation, so the case is
/// reachable only from a hand-written file, where the search path decides and
/// the file does not carry it. Matching is the announcing direction — a
/// spurious note over correct rows, never a silent wrong row set — which is
/// the way this register errs everywhere else a name is ambiguous.
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
/// these names are for a human reading `pgdq info`, never for dispatch.
pub const RANGE_STRUCT_FIELDS: [&str; 5] =
    ["lower", "upper", "lower_inclusive", "upper_inclusive", "empty"];

/// Split `declared` into its base type name and typmod contents, if any
/// (`numeric(38,10)` -> `("numeric", Some("38,10"))`). Only `numeric` cares
/// about the typmod's *value* — every other typed mapping below is
/// `Microsecond`-precision or otherwise typmod-independent by design (see
/// "Type mapping" in the phase doc), so this split is enough to let every
/// other match ignore it entirely.
pub(crate) fn split_typmod(s: &str) -> (&str, Option<&str>) {
    match s.find('(') {
        Some(i) if s.ends_with(')') => (s[..i].trim_end(), Some(&s[i + 1..s.len() - 1])),
        _ => (s, None),
    }
}

/// `numeric(p,s)` -> `Decimal128`/`Decimal256` when `p` fits; bare `numeric`
/// or a precision beyond `Decimal256`'s 76-digit ceiling stays `Utf8View`,
/// same as arbitrary precision (I4: `NaN` is reachable through any numeric
/// column regardless, and is a decode-time concern, not a mapping one).
///
/// The `Utf8View` arms are still *ordered*, and that is the whole of what
/// closes them: [`CompareKind::Numeric`] normalizes the text the file holds —
/// sign, integer digits, fraction — and compares by value, which is
/// `cmp_var_common`'s own order and is insensitive to trailing zeros (I33).
/// The typed arms carry the column's own scale into the comparison instead,
/// so both sides of one are unscaled integers.
///
/// **Only the bare form admits an infinity.** `apply_typmod_special` rejects
/// `±Infinity` under any typmod (I34), so the `p > 76` arm's column can hold
/// a `NaN` and never an infinity — and accepting the spelling in a *filter's
/// literal* there would accept a value the server refuses.
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

/// **The built-in scalar table** — "Type mapping"'s table for everything with
/// no `.` in its declared name (I8) that is not a range. `None` means the base
/// name isn't a built-in this build recognises (e.g. `money`, never
/// specified).
///
/// **Each arm answers both questions at once**: which Arrow type the column
/// gets, and how two of its values compare. That pairing is the comparison
/// register's exhaustiveness check — a type added here without a comparison
/// does not compile — and it replaces the exhaustive `match` over `DataType`
/// the register used to be, which could only ever have been keyed on a type
/// four unrelated declared types share (`docs/design/architecture.md`,
/// "Ordering operators compare typed").
///
/// `collation` is the column's own `COLLATE` clause, verbatim, and
/// `collations` is what the dump's own `CREATE COLLATION` statements said
/// about it; only the four collatable arms read either, and [`map_builtin`] —
/// which wants the Arrow type alone — passes `None` and an empty list.
fn builtin_scalar(
    base: &str,
    typmod: Option<&str>,
    collation: Option<&str>,
    collations: &[CollationDef],
) -> Option<(DataType, ComparisonPlan)> {
    use CompareKind as K;
    use DataType::*;
    use arrow::datatypes::TimeUnit::Microsecond;
    let agrees = ComparisonPlan::agrees;
    let text = ComparisonPlan::AS_TEXT;
    Some(match base.to_ascii_lowercase().as_str() {
        "smallint" => (Int16, agrees(K::Int)),
        "integer" => (Int32, agrees(K::Int)),
        "bigint" => (Int64, agrees(K::Int)),
        // `oidout` is `snprintf("%u")`, so the file holds an unsigned 32-bit
        // integer and `UInt32` is what it says (I39). The ADBC PostgreSQL
        // driver maps `oid` to `Int32`, which misreads every OID at or above 2^31 —
        // the floor rule permits a different type, never a wider one, and a
        // narrower reading of the same bytes is not what this is.
        //
        // `UnsignedInt`, not `Int`, and the difference is only ever visible
        // on a filter's literal: `oidin` accepts a leading minus and wraps
        // (`-1` is 4294967295 on every major from 13 — I39), where this refuses the
        // literal with `Error::PredicateValueDecode`. Refusing is what keeps
        // the row honest — the wrap is an input-grammar behaviour this build
        // does not implement, and reading `-1` as −1 would be a wrong answer
        // where this is merely a weaker one. No *field* is affected: no dump
        // ever writes a signed OID.
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
        // The fourth collatable arm, and the one that needs a comparison of
        // its own: the dump writes every `character(n)` value blank-padded to
        // `n` and `bpcharcmp` calls `bcTruelen` on both sides before it
        // consults a collation at all (I38). So the padding is stripped by
        // `K::PaddedText` and what is left is exactly the `text` question —
        // an explicit `C`/`POSIX` agrees, anything else diverges, and a bare
        // column is on the database's collation (I32).
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
        // Held as text in Arrow and *ordered* all the same, each by the
        // comparison its own type defines (I40). `time with time zone` sorts
        // by the UTC instant and breaks a tie on the zone; an `interval`'s
        // months collapse to 30 days and its days to 86400 s.
        "time with time zone" => (Utf8View, agrees(K::TimeTz)),
        "interval" => (Utf8View, agrees(K::Interval)),
        // `uuid_internal_cmp` is `memcmp` over 16 bytes, and `byteacmp` is
        // `memcmp` then length — both are `[u8]`'s own order (I33).
        "uuid" => (FixedSizeBinary(16), agrees(K::Uuid)),
        "bytea" => (Binary, agrees(K::Bytea)),
        // The two JSON types part company here. PostgreSQL defines *no*
        // comparison for `json` — no `=`, no order, no operator class — so
        // bytewise offers more than the server does rather than less, and
        // `json` is the one remaining member of the register's text-held row.
        "json" => (Utf8View, text),
        // `jsonb` has a full order and this implements it, structurally
        // (I41). What it cannot implement is the string leaves: they go
        // through the database's own collation, which the file does not
        // carry (I32), so the plan agrees about the shape and announces the
        // residue.
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
        _ => return None,
    })
}

/// A canonical Arrow extension type one of our columns claims — the *name*
/// half of the mapping, which the Arrow type alone cannot carry
/// (`docs/design/architecture.md`, "Type resolution").
///
/// Two of them exist for us, because the Arrow spec defines two whose storage
/// type is already what we emit: `arrow.uuid` over `FixedSizeBinary(16)`, and
/// `arrow.json` over `Utf8View`. Neither changes a column's Arrow type or a
/// single byte of its data; both let a consumer tell a UUID from sixteen
/// arbitrary bytes, and JSON from any other string, without asking us what the
/// declared PostgreSQL type was.
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
    /// **arrow-rs's own types do the writing, and its `supports_data_type`
    /// does the checking.** Hand-writing the two metadata keys would be four
    /// lines and would get `arrow.json` subtly wrong: its metadata key must be
    /// *present and empty*, and a reader calling arrow-rs's
    /// `Field::try_canonical_extension_type` rejects the field outright when
    /// it is absent. So the spelling comes from the crate that defines it.
    ///
    /// The `expect` is by construction: [`extension_for`] answers `Some` only
    /// where it walked to the same base name [`builtin_scalar`] maps to that
    /// extension's storage type, and both walks are the one in
    /// [`resolve_declared_type`]. `the_extension_names_fit_their_storage_type`
    /// is the test that keeps the two in step.
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
/// element type, not the column's, and stamping the column's own field would
/// claim the list *is* a UUID — then a `.`-qualified user type, where only a
/// domain can bottom out at a built-in, then the built-in name itself.
///
/// **Only a top-level column's field is stamped.** A `uuid` inside a composite
/// or an array element keeps its `FixedSizeBinary(16)` with no name on it: the
/// nested `Field`s are built inside this module's type constructors, which are
/// shared by every position, and a value read through `record_out`/`array_out`
/// is reachable through the declared type anyway. Nothing here is load-bearing
/// for decoding — the metadata is a claim about the bytes, never an input to
/// producing them.
pub fn extension_for(declared: &str, types: &[TypeDef]) -> Option<CanonicalExtension> {
    let declared = declared.trim();
    if array_element(declared).is_some() {
        return None;
    }
    let (base, _) = split_typmod(declared);
    if base.contains('.') {
        // A domain is the only user-defined kind that can reach a built-in;
        // an enum, composite, range or opaque base type maps to a type no
        // canonical extension names.
        let TypeKind::Domain { base_type, .. } = &types.iter().find(|t| t.name == base)?.kind
        else {
            return None;
        };
        return extension_for(base_type, types);
    }
    match base.to_ascii_lowercase().as_str() {
        "uuid" => Some(CanonicalExtension::Uuid),
        "json" | "jsonb" => Some(CanonicalExtension::Json),
        _ => None,
    }
}

/// [`extension_for`], applied — the one call site's whole job, kept here so
/// that `apply` need not be public.
pub(crate) fn with_extension(field: Field, declared: &str, types: &[TypeDef]) -> Field {
    match extension_for(declared, types) {
        Some(extension) => extension.apply(field),
        None => field,
    }
}

/// The built-in half of "Type mapping"'s table: [`builtin_scalar`], plus
/// PostgreSQL's twelve built-in range and multirange types. Unlike a
/// user-defined range (`CREATE TYPE ... AS RANGE`), those never appear
/// schema-qualified and have no `CREATE TYPE` of their own anywhere in the
/// file, so they need bare-name recognition here or they would wrongly fall
/// through to `Unknown` (confirmed against `fixtures/*/types/default.sql`'s
/// `t_range.v_range int4range`).
fn map_builtin(base: &str, typmod: Option<&str>, types: &[TypeDef]) -> Option<TypeOutcome> {
    // No collation: an Arrow type never depends on one, and the plan half of
    // the pair is discarded here.
    if let Some((mapped, _)) = builtin_scalar(base, typmod, None, &[]) {
        return Some(TypeOutcome::Mapped(mapped, NestedPlan::Scalar));
    }
    // Built-in ranges, and their PG14+ multirange counterparts (I10): both
    // appear bare, never schema-qualified, so both need this table rather
    // than the user-defined lookup (I8).
    let (subtype, multi) = builtin_range_subtype(&base.to_ascii_lowercase())?;
    let (bound, bound_plan) = resolve_nested(subtype, types);
    Some(if multi {
        TypeOutcome::Mapped(
            list_of(range_struct(bound)),
            NestedPlan::Multirange(Box::new(bound_plan)),
        )
    } else {
        TypeOutcome::Mapped(range_struct(bound), NestedPlan::Range(Box::new(bound_plan)))
    })
}

/// The subtype of one of PostgreSQL's twelve built-in range/multirange types,
/// plus whether the name was the multirange half. `None` for anything else.
///
/// **Hardcoded because the catalog holds it and the DDL does not.**
/// `TypeKind::Range::subtype` is populated only for a user-defined range;
/// `pg_dump` writes no `CREATE TYPE` at all for a built-in one. The six
/// multirange names carry the *same* subtypes as their range counterparts
/// and a different literal form, which is why they are told apart here
/// rather than sharing one answer (I10).
fn builtin_range_subtype(name: &str) -> Option<(&'static str, bool)> {
    let (subtype, multi) = match name {
        "int4range" => ("integer", false),
        "int8range" => ("bigint", false),
        "numrange" => ("numeric", false),
        "tsrange" => ("timestamp without time zone", false),
        "tstzrange" => ("timestamp with time zone", false),
        "daterange" => ("date", false),
        "int4multirange" => ("integer", true),
        "int8multirange" => ("bigint", true),
        "nummultirange" => ("numeric", true),
        "tsmultirange" => ("timestamp without time zone", true),
        "tstzmultirange" => ("timestamp with time zone", true),
        "datemultirange" => ("date", true),
        _ => return None,
    };
    Some((subtype, multi))
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

/// Resolve a type sitting *inside* a nested one — an array's element, a
/// composite's field, a range's bound.
///
/// Every non-`Mapped` outcome becomes `Utf8View` **in that position**, which
/// is exactly what the same type would have become at top level; the
/// recursion introduces no failure mode of its own, since it bottoms out on
/// the same mapping table whose worst answer is already a string.
fn resolve_nested(declared: &str, types: &[TypeDef]) -> (DataType, NestedPlan) {
    match resolve_declared_type(declared, types) {
        TypeOutcome::Mapped(data_type, plan) => (data_type, plan),
        _ => (DataType::Utf8View, NestedPlan::Scalar),
    }
}

/// The user-defined half: look `name` up in `types` (already schema-qualified,
/// matching how [`crate::preamble::TypeDef::name`] is stored) and resolve by
/// kind. A domain recurses on its base type — legal to nest (a domain over a
/// domain), and always finite: PostgreSQL cannot create a domain over a type
/// that does not exist yet, so there is no cycle to guard against.
fn resolve_user_type(name: &str, types: &[TypeDef]) -> TypeOutcome {
    let Some(def) = types.iter().find(|t| t.name == name) else {
        // Not a type of its own — but it might be a range's auto-created
        // multirange companion, which `pg_dump` never emits a `CREATE TYPE`
        // for at all (I10). Its only trace in the file is the
        // `multirange_type_name` parameter inside the range's own DDL, so
        // that's the only place left to look — and the range it names is
        // also where the companion's bound type comes from.
        let companion_of = types.iter().find(|t| {
            matches!(&t.kind, TypeKind::Range { multirange_type_name: Some(n), .. } if n == name)
        });
        return match companion_of.map(|t| &t.kind) {
            Some(TypeKind::Range { subtype, .. }) => {
                let (bound, plan) = range_bound(subtype.as_deref(), types);
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
        TypeKind::Domain { base_type, .. } => resolve_declared_type(base_type, types),
        // The field list is all-or-nothing: `None` means the grammar could
        // not read the body, and a `Struct` built from a short list would
        // refuse every valid row (see `TypeKind::Composite`). A zero-field
        // composite is a real type and maps to a zero-field `Struct` (I23).
        TypeKind::Composite { fields: None } => TypeOutcome::Unknown,
        TypeKind::Composite { fields: Some(fields) } => {
            let mut arrow_fields = Vec::with_capacity(fields.len());
            let mut plans = Vec::with_capacity(fields.len());
            for field in fields {
                let (data_type, plan) = resolve_nested(&field.declared_type, types);
                arrow_fields.push(Field::new(&field.name, data_type, true));
                plans.push(plan);
            }
            TypeOutcome::Mapped(
                DataType::Struct(Fields::from(arrow_fields)),
                NestedPlan::Record(plans),
            )
        }
        TypeKind::Range { subtype, .. } => {
            let (bound, plan) = range_bound(subtype.as_deref(), types);
            TypeOutcome::Mapped(range_struct(bound), NestedPlan::Range(plan))
        }
        // Both genuinely information-free (see the phase doc's "`CREATE
        // TYPE`: six emitted forms") — one diagnostic bucket for both.
        TypeKind::Base | TypeKind::Shell => TypeOutcome::OpaqueBaseType,
    }
}

/// A range's bound type, from the `subtype = ...` parameter the DDL carried.
/// A range whose parameter list the grammar could not read keeps its struct
/// shape with `Utf8View` bounds — the bounds are still exactly the text the
/// file holds, which is what every other unmapped position falls back to.
fn range_bound(subtype: Option<&str>, types: &[TypeDef]) -> (DataType, Box<NestedPlan>) {
    let (data_type, plan) = match subtype {
        Some(subtype) => resolve_nested(subtype, types),
        None => (DataType::Utf8View, NestedPlan::Scalar),
    };
    (data_type, Box::new(plan))
}

/// The whole decision for an array column, from its already-normalized
/// element type: the two refusals, and otherwise the `List` and the
/// [`NestedPlan::Array`] that fills it.
///
/// **Both refusals test the terminal of one domain walk, not the declared
/// spelling** (I22, I26). A domain records neither the `typdelim` it inherited
/// nor the array-ness of its base, so `CREATE DOMAIN d AS box` makes `d[]` a
/// semicolon-separated literal named neither `box` nor `TypeKind::Base`, and
/// `CREATE DOMAIN d AS integer[]` makes `d[]` an array of arrays while being
/// spelled like an array of any other named type. One walk answers both, which
/// is why they are decided here rather than by two predicates that each walk
/// it.
///
/// **The order between the two refusals decides a label, never a type.** Both
/// answer `Utf8View`, so nothing a caller reads depends on which fires. It is
/// written opaque-first because `OpaqueElementType` is the stronger statement
/// — the delimiter is not `,`, so even the element boundaries are
/// unrecoverable, where `NestedArrayElement` says the boundaries are readable
/// and we decline to represent what is inside them.
///
/// As written, no input reaches both: the opaque test matches a bare type name
/// and the array test matches that same name with array bounds appended, so a
/// terminal of `box[]` — `CREATE DOMAIN d AS box[]`, a column of `d[]` — is
/// only ever the second, and answers `NestedArrayElement`, true but silent
/// about the delimiter. Making that case answer `OpaqueElementType` means
/// testing opaqueness recursively through the element's own array levels; it
/// is a behaviour change, and it buys a better diagnostic on a shape `pg_dump`
/// cannot write (I21) rather than a better type.
///
/// `box` is checked by name because it is a built-in with no `CREATE TYPE` of
/// its own; a user-defined base type sets its delimiter in DDL this build does
/// not read, so `TypeKind::Base`/`Shell` are refused wholesale. The
/// array-ness test reads the terminal through [`array_element`] rather than
/// looking for a trailing `[]`, because `CREATE DOMAIN d AS integer ARRAY` is
/// as legal as any other spelling (I28) and the walk stops on whatever the DDL
/// wrote. That is the second of the normalization's two call sites; the first
/// is [`resolve_declared_type`]'s entry, which every other position — a
/// composite field, a range bound, a domain's own base type — reaches through.
///
/// See [`TypeOutcome::OpaqueElementType`] and
/// [`TypeOutcome::NestedArrayElement`] for why each shape is refused rather
/// than typed.
fn resolve_array(element: &str, types: &[TypeDef]) -> TypeOutcome {
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
    let (data_type, plan) = resolve_nested(element, types);
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
/// Deficiency register: `deficiency: KD4` — the detail is
/// `docs/design/architecture.md`'s "Type resolution", and the fix is
/// `roadmap.md`'s "A real type-name tokenizer", not this function's.
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
/// is not a domain.
///
/// **[`resolve_array`] tests this terminal rather than the declared spelling**
/// (I22, I26): a domain's own DDL records neither the `typdelim` it inherited
/// nor the array-ness of its base, so the property that decides either refusal
/// is only visible at the end of the walk. The terminal is returned as the DDL
/// spelled it — normalizing the array-bounds production here would have to
/// allocate, and its one reader normalizes what it needs to.
///
/// A domain chain visits each `CREATE DOMAIN` at most once, so the type list's
/// own length bounds it. `resolve_declared_type` recurses through domains
/// unbounded on the grounds that PostgreSQL cannot create a cycle; the bound
/// here costs nothing and keeps a hand-edited file from spinning rather than
/// merely failing.
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
/// The array check runs first because a declared array type still carries
/// its element type's own qualification (`public.mood[]` contains a `.` too)
/// — I21: `pg_dump` never preserves dimensionality, so a single trailing
/// `[]` covers every array shape regardless of underlying dimensions. It runs
/// through [`array_element`], so every spelling of the array-bounds production
/// collapses to the element type plus one array level (I28) before anything
/// else looks at the string.
pub fn resolve_declared_type(declared: &str, types: &[TypeDef]) -> TypeOutcome {
    let declared = declared.trim();
    if let Some(element) = array_element(declared) {
        return resolve_array(element, types);
    }
    let (base, typmod) = split_typmod(declared);
    if base.contains('.') {
        return resolve_user_type(base, types);
    }
    map_builtin(base, typmod, types).unwrap_or(TypeOutcome::Unknown)
}

/// The comparison for an array column, from the same walk
/// [`resolve_array`] makes and with the same two refusals: an element type
/// that is opaque by construction (I22), and an element type that is itself
/// an array (I26). Both resolve the *column* to `Utf8View`, so a column of
/// either compares as text and never reaches this plan — but the register is
/// asked directly too, and answering "ordered" for a column the resolver
/// declines would be the register disagreeing with itself.
///
/// The column's own `COLLATE` clause is passed **down to the element**, which
/// is where it belongs: an array type is not collatable, and `pg_dump` writes
/// the clause on a `text[]` column to state the collation its *elements* are
/// compared under (I37).
fn array_comparison(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
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
        NestedCompare::Uncomparable { declared: element.to_string() }
    } else {
        nested_position(element, collation, types, collations)
    };
    ComparisonPlan::Nested(NestedCompare::Array(Box::new(child)))
}

/// One position *inside* a nested type — an array's element, a composite's
/// field — asked the same question the column was, so nesting composes and a
/// domain beneath a container bottoms out where a domain always does.
///
/// The two answers that are not a comparison collapse to
/// [`NestedCompare::Uncomparable`], and the second of them is the one worth
/// stating: `json` is [`ComparisonDivergence::AsText`] at top level, where
/// bytewise offers more than the server does, and has no comparison *at all*
/// inside a container, where `array_cmp` would have to find a proc that does
/// not exist.
fn nested_position(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
) -> NestedCompare {
    match comparison_for(declared, collation, types, collations) {
        ComparisonPlan::Compared { divergence: Some(ComparisonDivergence::AsText), .. }
        | ComparisonPlan::Refused => NestedCompare::Uncomparable { declared: declared.to_string() },
        ComparisonPlan::Compared { kind, divergence } => {
            NestedCompare::Leaf { declared: declared.to_string(), kind, divergence }
        }
        ComparisonPlan::Nested(inner) => inner,
    }
}

/// **The comparison register**: how a column declared `declared` compares, and
/// whether that is PostgreSQL's own order.
///
/// It walks the declared type exactly as [`resolve_declared_type`] does —
/// array first, then the built-in table, then the database's own
/// `CREATE TYPE`/`DOMAIN` list — so the two answers are reached through one
/// spelling of the same string and a domain compares as whatever it bottoms
/// out at.
///
/// **Keyed on the declared type, not on the Arrow one.** Four unrelated
/// declared types reach `Utf8View` — a text type, a bare `numeric`, a
/// text-held type such as `interval`, and `json`, which PostgreSQL does not
/// order at all — so the Arrow type cannot say which comparison a column
/// wants, and the answers that will replace them are per declared type too.
///
/// **A nested type answers [`ComparisonPlan::Nested`]**, one node per nesting
/// level, built by the same walk: an array's element and a composite's fields
/// are asked this same question in turn, so a position's comparison is
/// whatever a *column* of that type would have had and nesting composes with
/// no special case. A range or multirange is still
/// [`ComparisonPlan::Refused`] — the canonicalization its bounds need is not
/// implemented here yet.
///
/// **`collation` is the column's own `COLLATE` clause**, verbatim as the DDL
/// wrote it ([`crate::preamble::ColumnDef::collation`]), or `None` where the
/// column carries none — which `pg_dump` writes exactly when the column's
/// collation is its type's default (I37), so the absence is a fact about the
/// type rather than about the column. It is why this register is keyed per
/// *column* and not only per declared type: two `text` columns of one table
/// can compare differently.
pub fn comparison_for(
    declared: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
) -> ComparisonPlan {
    let declared = declared.trim();
    if array_element(declared).is_some() {
        return array_comparison(declared, collation, types, collations);
    }
    let (base, typmod) = split_typmod(declared);
    if base.contains('.') {
        return comparison_user_type(base, collation, types, collations);
    }
    // A built-in range name reaches neither arm of `builtin_scalar` and is
    // refused, which is the same answer the nested check above gives a
    // user-defined one.
    builtin_scalar(base, typmod, collation, collations)
        .map_or(ComparisonPlan::Refused, |(_, plan)| plan)
}

/// The user-defined half of [`comparison_for`], over the same `TypeKind` list
/// [`resolve_user_type`] reads.
///
/// **Exhaustive over `TypeKind` with no wildcard arm**, so a kind added to the
/// preamble grammar has to choose a comparison rather than inheriting one.
fn comparison_user_type(
    name: &str,
    collation: Option<&str>,
    types: &[TypeDef],
    collations: &[CollationDef],
) -> ComparisonPlan {
    // Absent from the list: either an unknown type or a range's multirange
    // companion (I10). Neither has an order here.
    let Some(def) = types.iter().find(|t| t.name == name) else {
        return ComparisonPlan::Refused;
    };
    match &def.kind {
        // An enum with no labels resolves to no Arrow type at all, so no
        // column of it is ever asked how it compares.
        TypeKind::Enum { labels } if labels.is_empty() => ComparisonPlan::Refused,
        // PostgreSQL orders an enum by `pg_enum.enumsortorder`, which is
        // assigned from *declaration* order (I33) — so the position of a
        // label in this list is the order, and the label text is not. The
        // labels are in hand here because the dump carries them verbatim,
        // which is the whole reason the register is keyed on the declared
        // type rather than on the Arrow one.
        TypeKind::Enum { labels } => {
            ComparisonPlan::agrees(CompareKind::Enum(labels.iter().cloned().collect()))
        }
        // A domain compares as what it bottoms out at, through any chain,
        // which is the same recursion `resolve_declared_type` makes and is
        // finite for the same reason: PostgreSQL cannot create a cycle.
        //
        // The collation walks down with it, and the *column's* clause wins:
        // a domain's own `COLLATE` is its type default, which `pg_dump` writes
        // a column-level clause only to override (I37).
        TypeKind::Domain { base_type, collation: domain_collation } => {
            comparison_for(base_type, collation.or(domain_collation.as_deref()), types, collations)
        }
        // Field-wise in declaration order, which is `record_cmp`'s rule and
        // also the order `record_out` writes them in, so the positional
        // comparison costs nothing here (I23). A field list the grammar could
        // not read is all-or-nothing exactly as it is for the Arrow type: a
        // composite parsed short would compare field 3's text as field 2's
        // type, so there is no partial answer to give.
        TypeKind::Composite { fields } => match fields {
            Some(fields) => ComparisonPlan::Nested(NestedCompare::Record(
                fields
                    .iter()
                    .map(|f| {
                        (
                            f.name.clone(),
                            // A composite is not collatable, so the *column's*
                            // clause cannot reach a field; each attribute
                            // carries its own (I37).
                            nested_position(
                                &f.declared_type,
                                f.collation.as_deref(),
                                types,
                                collations,
                            ),
                        )
                    })
                    .collect(),
            )),
            None => ComparisonPlan::Refused,
        },
        // A range's bounds need the subtype's own canonicalization before
        // they can be compared — `int4range '[1,10]'` is `[1,11)` — which is
        // not implemented here yet.
        TypeKind::Range { .. } => ComparisonPlan::Refused,
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
    /// point: the ADBC driver reads the *binary* wire format into `Int32`,
    /// which turns every OID at or above 2^31 negative. `oidout` writes
    /// `%u`, so the text says what it says.
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
        // leaving bare uppercase "INTEGER" (I5) -- pg_dump's own literal
        // casing there, not something we get to normalize upstream.
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
    /// catalog, never in DDL (I10), so this table is the only place they
    /// exist. The multirange half maps to a `List` of the *same* range struct
    /// — same subtype, different literal form.
    #[test]
    fn builtin_ranges_and_their_multirange_companions_map_to_their_hardcoded_subtypes() {
        let bounds = [
            ("int4range", "int4multirange", DataType::Int32),
            ("int8range", "int8multirange", DataType::Int64),
            // Bare `numeric` has no Arrow decimal representation, so a
            // `numrange`'s bounds are text — the subtype's own mapping, not a
            // special case here.
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
        // An element type with no mapping is `Utf8View` *in that position*,
        // exactly as it would be at top level — the element boundaries are
        // still recovered, which is what a whole-column string would lose.
        assert_eq!(
            resolve_declared_type("interval[]", &[]),
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

    /// I26, and the transitive half the fixture deliberately does not carry:
    /// a domain over a domain over an array produces a literal byte-identical
    /// to the single-hop case, so there is no `pg_dump` output shape left to
    /// predict and the walk is pinned here instead.
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
        // Only the outer array is refused. The domain itself is an ordinary
        // `integer[]` column at every depth of the chain, and nothing about
        // it changed: its literal is one brace deep and means one dimension.
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
    /// `pg_dump` can never write them (I21) and no generated fixture can
    /// reach this. That is `roadmap.md`'s "Where a fixture is impossible"
    /// carve-out, and the I28 citation here is its check: it puts the
    /// behaviour inside the register's re-verify ritual at each new major.
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
            // the lexer is free with whitespace. All observed on 16.15.
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
    /// malformed bound is not a bound. Every string here is a syntax error on
    /// 16.15, and the outcome that says so is the honest `Unknown` — not a
    /// refusal, which would state something false about the column the way
    /// `integer[][]` did before this normalization existed.
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

    /// The normalization's second call site. A domain's base type is spelled
    /// by whoever wrote the `CREATE DOMAIN`, so the array-ness the I26 refusal
    /// tests for can arrive in any spelling — and the domain walk stops on the
    /// raw text. `d[]` over `CREATE DOMAIN d AS integer ARRAY` is the same
    /// array-of-arrays as `d[]` over `integer[]`, and is refused the same way,
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
    /// of the refused type is one string field inside an otherwise typed
    /// `Struct` — not a refusal of the whole column.
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
                },
            ),
            ty("public.bare", TypeKind::Range { subtype: None, multirange_type_name: None }),
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
    /// The two are separate walks of the same string, so the failure to guard
    /// against is one of them moving: a `uuid` remapped away from
    /// `FixedSizeBinary(16)` would make `apply`'s `expect` a panic on a real
    /// dump. `try_with_extension_type` is arrow-rs's own
    /// `supports_data_type`, so what this asserts is the crate's rule and not
    /// a restatement of it.
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
        // A `.`-qualified name the dump never declared resolves to nothing at
        // all, so it certainly names no extension.
        assert_eq!(extension_for("public.nope", &types), None);
    }

    // -- the comparison register ------------------------------------------

    fn agrees(kind: CompareKind) -> ComparisonPlan {
        ComparisonPlan::agrees(kind)
    }

    /// The register, row for row: every declared type this build maps to a
    /// scalar, and how a column of it compares. This is the authority the
    /// Markdown table in `docs/design/architecture.md`, "Ordering operators
    /// compare typed", renders for humans.
    ///
    /// **Every arm of [`builtin_scalar`] appears here**, which is what makes
    /// the list a register rather than a sample: a type added to that table
    /// without a row here is a type nothing states the comparison of.
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
            // The four collatable arms, each asked with no `COLLATE`
            // clause — which is the shape of every column in the tree today.
            // `name`'s type default is `C`, so it agrees where the others
            // cannot; `character(n)` carries a comparison of its own (the
            // padding is trimmed off both sides, I38) and then asks the same
            // collation question the other three do.
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
            // in its width — both distinctions the plan has to carry, since
            // the declared name is gone by the time a value is read.
            ("inet", agrees(K::Network { cidr: false })),
            ("cidr", agrees(K::Network { cidr: true })),
            ("macaddr", agrees(K::MacAddr { octets: 6 })),
            ("macaddr8", agrees(K::MacAddr { octets: 8 })),
        ] {
            assert_eq!(comparison_for(declared, None, &[], &[]), expected, "{declared}");
        }
        // A declared type this build maps to nothing has no comparison
        // either — the two answers are reached through one walk of the same
        // string, so they cannot disagree about which types exist.
        assert_eq!(comparison_for("money", None, &[], &[]), ComparisonPlan::Refused);
        // A keyword is a keyword on both walks (I5).
        assert_eq!(comparison_for("INTEGER", None, &[], &[]), agrees(K::Int));
    }

    /// Seven unrelated declared types reach `Utf8View`, which is why the
    /// register cannot be keyed on the Arrow type: an enum compares by
    /// declaration order, a bare `numeric` by decimal value, an `interval` by
    /// a 128-bit span, an `inet` by family-then-prefix, a `jsonb` by a walk
    /// down two containers, `text` bytewise with the column's own collation
    /// deciding whether that is right, and `json` bytewise because the server
    /// defines no order at all.
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
            comparison_for("interval", None, &[], &[]),
            ComparisonPlan::agrees(CompareKind::Interval),
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
            // An unquoted `C` is the collation `c`, which is not the built-in
            // one — the server folds it the same way, and answering "agrees"
            // here is the one direction of error this register must not make.
            ("text", Some("C"), named()),
            // Nor is a `"C"` some other schema happens to define.
            ("text", Some("public.\"C\""), named()),
            // `character(n)` reads the clause exactly as `text` does, and
            // answers over its own comparison: `bpcharcmp` trims both sides'
            // trailing blanks and *then* consults the collation (I38), so an
            // explicit `COLLATE "C"` agrees and a bare column does not.
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
    /// It is the only one of the four that reaches `=`, and the only one read
    /// off a statement rather than off a name — which is why the same clause
    /// answers differently depending on what the dump's `CREATE COLLATION`
    /// list holds.
    #[test]
    fn a_collation_the_dump_declares_non_deterministic_diverges_under_equality_too() {
        use CompareKind as K;
        let coll = |name: &str, deterministic: bool| CollationDef {
            name: name.to_string(),
            deterministic,
        };
        let nd = ComparisonDivergence::NonDeterministicCollation;
        let named = ComparisonDivergence::NonBytewiseCollation;

        // Nothing declared: the clause is read off its name alone, exactly as
        // before this branch existed.
        assert_eq!(
            comparison_for("text", Some("public.icu_ci"), &[], &[]),
            ComparisonPlan::diverging(K::Text, named)
        );
        // Declared deterministic — which is what every `CREATE COLLATION` in
        // the committed fixtures says — moves nothing either.
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
        // Quoting is not textual: `CREATE COLLATION` and a `COLLATE` clause
        // come from different `pg_dump` paths and either may quote what the
        // other leaves bare.
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
        // announcing direction, since the file does not carry a search path.
        assert_eq!(
            comparison_for("text", Some("icu_ci"), &[], &declared),
            ComparisonPlan::diverging(K::Text, nd)
        );
        // ... but a *qualified* reference must agree on the schema.
        assert_eq!(
            comparison_for("text", Some("elsewhere.icu_ci"), &[], &declared),
            ComparisonPlan::diverging(K::Text, named)
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
        // through the same recursion — the walk bottoms out where a domain
        // always does.
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

    /// A domain over an enum carries the enum's labels, through any chain —
    /// which is what lets `pgdq info --verbose` list them beneath such a
    /// column and what lets a `--filter` term name one. **No fixture column
    /// is one**: `public.derived_domain` bottoms out at `integer` and
    /// `public.text_c` at `text`, so the recursion above is the only thing
    /// that makes the claim true and this is the only thing that checks it.
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

    /// Which nested shapes the register compares and which it still refuses,
    /// as one statement: an array and a composite are compared structurally,
    /// a range and its multirange companion are not — their bounds need the
    /// subtype's canonicalization, which is not implemented here.
    ///
    /// A base or shell type is refused for a reason of its own and is here to
    /// keep the two populations from being read as one.
    #[test]
    fn arrays_and_composites_compare_where_ranges_are_still_refused() {
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
        for declared in [
            "int4range",
            "int4multirange",
            "public.myrange",
            "public.myrange_multi",
            "public.gtype",
            "public.forward",
            "public.nosuchtype",
        ] {
            assert_eq!(
                comparison_for(declared, None, &types, &[]),
                ComparisonPlan::Refused,
                "{declared}"
            );
        }
    }

    /// Comparability is inherited: a position whose declared type has no
    /// order refuses the whole column, and the tree says which position and
    /// which type so the refusal can name them.
    ///
    /// **`json` is the case that is not obvious.** At top level it is
    /// compared bytewise and announces that PostgreSQL orders it not at all;
    /// inside a container there is nothing to compare with, because
    /// `array_cmp` looks up the element type's comparison proc and there is
    /// none.
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
        for (declared, path, at) in [
            ("json[]", "[]", "json"),
            ("public.jsonpair", ".doc", "json"),
            ("public.jsonpair[]", "[].doc", "json"),
            // I22: an opaque element type, refused for the delimiter as much
            // as for the order.
            ("box[]", "[]", "box"),
            ("public.gtype[]", "[]", "public.gtype"),
            // I26: an array whose element is itself an array, which resolves
            // the column to text as well.
            ("public.intarr[]", "[]", "public.intarr"),
        ] {
            let plan = comparison_for(declared, None, &types, &[]);
            assert!(!plan.orders(), "{declared}");
            let ComparisonPlan::Nested(tree) = plan else { panic!("{declared}: not nested") };
            assert_eq!(tree.uncomparable(), Some((path.to_string(), at.to_string())), "{declared}");
        }
    }

    /// Every diverging position, in walk order, each with the path and the
    /// declared type its sentence needs — the composite that is on the
    /// database's collation twice, once directly and once through an
    /// element.
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
}
