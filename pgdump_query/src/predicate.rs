//! Post-parse row filtering (`docs/design/architecture.md`, "Predicates").

use std::cmp::Ordering;

use arrow::datatypes::i256;

use crate::copy::{RawRow, RowSplit};
use crate::decode;
use crate::nested;
use crate::pgtype::{
    CompareKind, ComparisonDivergence, ComparisonPlan, NestedCompare, NestedPlan,
    UnanswerableReason,
};
use crate::resolve::{ColumnResolution, ResolvedSchema};
use crate::{Error, Result};

/// Comparison operator for [`Predicate`].
///
/// **Every operator but the two NULL tests compares typed**, through the
/// column's own [`ComparisonPlan`] — the four ordering operators by decoding
/// both sides and comparing the values, `Eq`/`Ne` by the cheapest of three
/// canonicalizations that gives the server's answer for that column
/// ([`equality_comparison`]). Where the register has no plan for a column —
/// it did not resolve, it is nested, or this build orders its type not at all
/// — `Eq`/`Ne` fall back to the string comparison every column made before,
/// which is right for the reason it always was: every value in a dump is
/// already in canonical `*_out` form. The ordering operators are *refused*
/// there instead, because an order over a composite or an array literal is
/// not a thing this layer can define.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOp {
    Eq,
    Ne,
    /// `col IS NULL` — matches only a NULL field (`\N`). Rounds out the NULL
    /// semantics `Eq`/`Ne` deliberately can't express (see [`Predicate::value`]).
    IsNull,
    /// `col IS NOT NULL` — matches every non-NULL field.
    IsNotNull,
    Lt,
    Le,
    Gt,
    Ge,
    /// `col IS DISTINCT FROM <value>` — [`Self::Ne`] with NULL counted as a
    /// value rather than as unknown, so a NULL field answers [`Truth::True`]
    /// where `!=` answers [`Truth::Unknown`].
    ///
    /// **The one operator three-valued logic makes necessary rather than
    /// redundant**: `NOT UNKNOWN` is `UNKNOWN`, so `Not(Term(a = 1))` drops a
    /// row whose `a` is NULL and nothing else can express "different,
    /// counting NULL as a value".
    IsDistinctFrom,
    /// `col IS NOT DISTINCT FROM <value>` — [`Self::Eq`] with the same NULL
    /// rule, answering [`Truth::False`] on a NULL field.
    IsNotDistinctFrom,
}

impl PredicateOp {
    /// Whether this operator compares by the column's own order rather than
    /// as text — the four that need a `Mapped` column with a
    /// [`NestedPlan::Scalar`] plan.
    pub fn is_ordering(self) -> bool {
        matches!(self, Self::Lt | Self::Le | Self::Gt | Self::Ge)
    }

    /// How an error message names this operator — the same spelling
    /// `pgdq query --filter` accepts.
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::IsNull => "IS NULL",
            Self::IsNotNull => "IS NOT NULL",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::IsDistinctFrom => "IS DISTINCT FROM",
            Self::IsNotDistinctFrom => "IS NOT DISTINCT FROM",
        }
    }
}

/// A single-column post-parse filter: `column <op> value`, and the **leaf**
/// of an [`Expr`] — a query carries one expression and keeps a row only if
/// its root evaluates [`Truth::True`] (`QueryOptions::filter`). `column` is
/// matched against the queried table's column names (the `COPY` header list,
/// or the `column1`, `column2`, ... placeholders used when the header has
/// none). `value` is `None` for `IsNull`/`IsNotNull`, which need no
/// comparison value; it is always `Some` for every other operator.
///
/// **`value` is read with the column's own decoder, whatever the operator**,
/// once when the block's schema resolves rather than per row — so a literal
/// that is not a value of the column's type is `Error::PredicateValueDecode`
/// before any row is read, and both sides of a `numeric(p,s)` comparison
/// carry that column's scale. What differs between the operators is what is
/// kept: an ordering operator keeps the decoded key and decodes the field to
/// match, while `Eq`/`Ne` usually keep the literal *rendered back* into the
/// `*_out` spelling the file holds and compare bytes
/// ([`equality_comparison`]).
///
/// A NULL field is [`Truth::Unknown`] under every comparing operator — not
/// `Eq`, not `Ne`, and not an ordering operator — because SQL's own
/// three-valued logic says so, and a row survives only where the root is
/// `True`. Four operators are two-valued on a NULL field by definition, and
/// they exist because unknown swallows everything else:
/// `IsNull`/`IsNotNull` ask about the NULL directly
/// (`docs/status/history/2026-08-22.md`), and
/// `IsDistinctFrom`/`IsNotDistinctFrom` count it as a value
/// (`docs/design/architecture.md`, "Predicates").
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: Option<String>,
}

/// SQL's three-valued truth domain, which is what a filter evaluates in.
///
/// **A row survives only if the expression's root is [`Truth::True`]**, so
/// `Unknown` and `False` are indistinguishable at the top — which is why
/// collapsing unknown to "excluded" was sound while a filter was a bare
/// conjunction, and stops being sound the moment [`Expr::Not`] can sit above
/// a term: `NOT UNKNOWN` is `UNKNOWN`, not `TRUE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Truth {
    True,
    False,
    Unknown,
}

impl Truth {
    /// `True` for `True` and nothing else — the question the row path asks.
    pub fn is_true(self) -> bool {
        matches!(self, Self::True)
    }

    /// `True`/`False`, for the comparisons that cannot be unknown once the
    /// field is known not to be NULL.
    fn of(b: bool) -> Self {
        if b { Self::True } else { Self::False }
    }

    /// SQL's `NOT`: `Unknown` is its own negation.
    fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
}

/// A row filter as a boolean expression tree over single-column
/// [`Predicate`] terms — what `QueryOptions::filter` carries
/// (`docs/design/architecture.md`, "Predicates").
///
/// `And` and `Or` are **n-ary**, because the shape a repeated `--filter`
/// builds is n-ary by construction and binary nesting would make the
/// ordinary case a right-leaning chain every reader has to flatten
/// mentally. The default — and the filter every query had before this — is
/// the empty conjunction, [`Expr::all`] over nothing, which every row
/// satisfies; so "no filter" is a degenerate tree rather than a case of its
/// own, and nothing on the row path branches on whether a filter exists.
///
/// **Nothing here is parsed.** `Expr` is a struct an embedder fills in field
/// by field, exactly as [`Predicate`] is; the `--where` grammar that builds
/// one from text lives in the CLI
/// (`docs/design/architecture.md`, "A filter term is parsed for two
/// audiences").
#[derive(Debug, Clone)]
pub enum Expr {
    Term(Predicate),
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Not(Box<Expr>),
}

impl Expr {
    /// The conjunction of `terms` — the shape a repeated `--filter` builds,
    /// and the shape every filter had before expressions existed. Over an
    /// empty iterator it is the filter that keeps every row.
    pub fn all(terms: impl IntoIterator<Item = Predicate>) -> Self {
        Self::And(terms.into_iter().map(Self::Term).collect())
    }
}

impl Default for Expr {
    fn default() -> Self {
        Self::all([])
    }
}

/// One term of one query whose comparison does not answer what PostgreSQL's
/// own operator for that column would — reported per stream by
/// `crate::stream::TableStream::comparison_notes`.
///
/// **Per term rather than per column, because a divergence is
/// operator-conditional**: three of the four
/// [`ComparisonDivergence`] variants are divergences of *order* alone
/// ([`ComparisonDivergence::affects_equality`]), so a `text` column with no
/// `COLLATE` clause earns a note under `<` and none under `=`. A query
/// filtering that column with both operators therefore carries one note, not
/// two, and neither zero.
///
/// **Neither a `Diagnostic` nor a [`crate::resolve::ColumnNote`]**, and
/// deliberately: `DumpIndex.diagnostics` is the L1 file-level channel and
/// `ResolvedSchema.notes` is the L2 per-column one, while this is per-column
/// *and* conditional on a predicate — L4. Writing it into either would
/// invert the layering (`docs/design/layering.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonNote {
    pub column: String,
    /// The position *inside* the column the divergence is about, as the
    /// accessor a user would write — `[]` for an array's elements, `.label`
    /// for a composite's field, appended as the nesting descends. `None` is
    /// the column itself, which is every scalar column and is what the
    /// sentence reads as when there is no nesting to name.
    ///
    /// **A nested column can carry more than one**, which is why a term
    /// yields a list of notes rather than one: `public.tagged` is
    /// `(label text, tags text[])` and both positions are on the database's
    /// own collation, one directly and one through an element.
    pub path: Option<String>,
    /// The declared PostgreSQL type **at that position**, as the DDL spelled
    /// it — the element's or the field's for a nested note, and the column's
    /// own where `path` is `None`. It is what sharpens the sentence, so it
    /// has to name the type the divergence is actually about.
    pub declared_type: String,
    pub divergence: ComparisonDivergence,
}

impl ComparisonNote {
    /// One sentence naming the column and what its comparison is not.
    ///
    /// Each sentence names the column and its declared type, and the
    /// declared type is what sharpens it: the collatable text types say what
    /// the *column* stated, where `AsText` says what the type means. Both
    /// exist because several declared types reach one Arrow type for
    /// different reasons.
    pub fn message(&self) -> String {
        let column = format!("{}{}", self.column, self.path.as_deref().unwrap_or(""));
        let column = &column;
        let declared = &self.declared_type;
        let bytewise = |why: &str| format!("`{column}` ({declared}) is compared bytewise: {why}");
        match self.divergence {
            // Not "PostgreSQL orders this differently": it does not order it
            // at all. The sentence has to say which way the difference runs,
            // because a user who reads "diverges" and assumes the server has
            // a better answer will go looking for one that does not exist.
            ComparisonDivergence::AsText => bytewise(
                "PostgreSQL defines no comparison for this type at all — no equality, no \
                 ordering, no operator class — so this comparison is one the server does not \
                 have",
            ),
            ComparisonDivergence::UnknownCollation => bytewise(
                "the column declares no COLLATE clause, so its collation is the database's, which \
                 a plain dump does not record — this matches the server only if that collation is \
                 C or POSIX",
            ),
            ComparisonDivergence::NonBytewiseCollation => bytewise(
                "the column declares a collation other than C/POSIX, and PostgreSQL orders it by \
                 that collation",
            ),
            // The only sentence here that reports what the dump *said* rather
            // than what it left out, which is why it names equality outright:
            // under a non-deterministic collation two values that differ byte
            // for byte can be equal to the server.
            ComparisonDivergence::NonDeterministicCollation => bytewise(
                "the column declares a collation this dump declares non-deterministic, so \
                 PostgreSQL neither orders nor compares it byte for byte — two values spelled \
                 differently can be equal to the server",
            ),
            ComparisonDivergence::UnmodelledType => bytewise(
                "this build models no comparison for the column's declared type — the four \
                 ordering operators are refused on it, and PostgreSQL's own equality for such a \
                 type need not be a byte comparison (`box` compares areas)",
            ),
            // Not a `bytewise` sentence: the structure *is* compared the way
            // PostgreSQL compares it, and only a string leaf is left.
            ComparisonDivergence::JsonbStringCollation => format!(
                "`{column}` ({declared}) is compared structurally, but every string value and \
                 object key inside it is ordered by the database's collation, which a plain dump \
                 does not record — this matches the server only if that collation is C or POSIX",
            ),
        }
    }
}

/// One side of a bare-`numeric` comparison: the sign, and the digits either
/// side of the point with the *insignificant* ones removed — leading zeros
/// from the integer part, trailing zeros from the fraction.
///
/// **Normalizing is what makes this PostgreSQL's own order.** `cmp_numerics`
/// compares by value and never by display scale (I33), so `1.5` and `1.50`
/// are one value that a bare `numeric` column writes two ways; and a fixed
/// scale, which is what [`CompareKind::Decimal`] carries both sides to, does
/// not exist here to rescale against.
///
/// Digit *strings* rather than a big integer, because the column is
/// arbitrary-precision by definition: the file may hold a thousand digits,
/// which is past every fixed-width type including `i256`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NumericKey {
    /// False for a zero of either written sign — PostgreSQL has one zero, and
    /// a filter literal is free to spell it `-0`.
    negative: bool,
    /// Integer digits, leading zeros stripped; empty for a magnitude below 1.
    int: String,
    /// Fraction digits, trailing zeros stripped; empty for an integer.
    frac: String,
}

impl NumericKey {
    /// `[-]digits[.digits]`, the only shape `numeric_out` writes: no
    /// exponent, no sign but `-`, and at least one digit somewhere — the same
    /// *lexical* grammar [`decode::decimal_unscaled_digits`] accepts for a
    /// typmod'd column, so a literal one `numeric` comparison rejects as
    /// malformed the other does too. What the two differ on is the typmod: a
    /// literal finer than the column's scale is refused there and has nothing
    /// to be refused against here.
    fn parse(text: &str) -> Option<Self> {
        let (negative, rest) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let (int, frac) = rest.split_once('.').unwrap_or((rest, ""));
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self::from_parts(negative, int, frac))
    }

    /// Drop the insignificant digits from an already-split sign/integer part/
    /// fraction, which is the whole of the normalization. The one shared
    /// entry point, because the two callers reach a split differently: a
    /// `numeric` field is read straight out of the text, while a `jsonb`
    /// number's point has to be moved by its exponent first.
    fn from_parts(negative: bool, int: &str, frac: &str) -> Self {
        let int = int.trim_start_matches('0');
        let frac = frac.trim_end_matches('0');
        Self {
            negative: negative && !(int.is_empty() && frac.is_empty()),
            int: int.to_string(),
            frac: frac.to_string(),
        }
    }

    /// Sign first, then magnitude: how many integer digits, then those digits,
    /// then the fraction. Each stage is a plain byte comparison over ASCII
    /// digits, which is why the normalization above has to have happened —
    /// with trailing zeros stripped, a fraction that is a prefix of another is
    /// the smaller of the two, so `"5"` beats `"45"` and loses to `"55"`.
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.negative, other.negative) {
            (false, true) => return Ordering::Greater,
            (true, false) => return Ordering::Less,
            _ => {}
        }
        let magnitude = self
            .int
            .len()
            .cmp(&other.int.len())
            .then_with(|| self.int.as_bytes().cmp(other.int.as_bytes()))
            .then_with(|| self.frac.as_bytes().cmp(other.frac.as_bytes()));
        if self.negative { magnitude.reverse() } else { magnitude }
    }
}

/// One side of an `inet`/`cidr` comparison: the family, the netmask length,
/// and the address left-aligned in sixteen bytes.
///
/// **Not a byte key, and it cannot be made into one.** `network_cmp_internal`
/// compares the *shorter* netmask's worth of address bits first, so how many
/// bits are significant depends on the value it is being compared against —
/// `10.1.0.0/8` sorts below `10.0.0.0/16` because their first eight bits
/// agree and `8 < 16`, where a plain address-then-netmask key would put it
/// above (I40). So the pair is compared, not two independently sortable keys.
///
/// `v6` is the family, as a bool because there are two and PostgreSQL's own
/// `PGSQL_AF_INET6` is `PGSQL_AF_INET + 1` — an IPv4 address sorts below
/// every IPv6 one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkKey {
    v6: bool,
    bits: u8,
    /// The address, left-aligned: an IPv4 address occupies the first four
    /// bytes and the rest are zero, which is exactly what `bitncmp` reads
    /// since it never looks past `maxbits`.
    addr: [u8; 16],
}

impl NetworkKey {
    /// `ip_maxbits` — the family's address width.
    fn maxbits(&self) -> u8 {
        if self.v6 { 128 } else { 32 }
    }

    /// `network_cmp_internal` (I40): family, then the shorter netmask's
    /// worth of address bits, then the netmask length, then the whole
    /// address.
    fn cmp(&self, other: &Self) -> Ordering {
        if self.v6 != other.v6 {
            return self.v6.cmp(&other.v6);
        }
        let shared = self.bits.min(other.bits);
        bitncmp(&self.addr, &other.addr, shared)
            .then_with(|| self.bits.cmp(&other.bits))
            .then_with(|| bitncmp(&self.addr, &other.addr, self.maxbits()))
    }
}

/// PostgreSQL's `bitncmp`: the first `n` bits of two addresses, most
/// significant first. Whole bytes by `memcmp`, then the remaining bits of the
/// straddling byte under a high-bit mask — which is the same answer as
/// `bitncmp`'s bit-at-a-time loop, since that loop stops at the first
/// differing bit and a masked byte comparison finds exactly that bit.
fn bitncmp(left: &[u8; 16], right: &[u8; 16], n: u8) -> Ordering {
    let whole = usize::from(n / 8);
    let full = left[..whole].cmp(&right[..whole]);
    let spare = n % 8;
    if full != Ordering::Equal || spare == 0 {
        return full;
    }
    let mask = 0xffu8 << (8 - spare);
    (left[whole] & mask).cmp(&(right[whole] & mask))
}

/// One `jsonb` value, in the shape PostgreSQL stores and compares one in.
///
/// **The variants are `JsonbValue`'s own type codes, in their order**, because
/// those codes *are* the fallback order `compareJsonbContainers` uses whenever
/// two positions hold different kinds: `jbvNull` 0x0, `jbvString` 0x1,
/// `jbvNumeric` 0x2, `jbvBool` 0x3, `jbvArray` 0x10, `jbvObject` 0x11 (I41).
/// So an object outranks an array, an array outranks every scalar, and a
/// boolean outranks a number — none of which is JSON's own idea of an order,
/// and none of which a bytewise comparison of the text produces.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Jsonb {
    Null,
    String(String),
    /// A JSON number is stored as a `numeric` and compared by `numeric_cmp`,
    /// which is exactly [`NumericKey`] — so `1`, `1.0` and `1e0` are one
    /// value, as they are for a bare `numeric` column.
    Number(NumericKey),
    Bool(bool),
    /// `raw_scalar` marks the **pseudo-array a top-level scalar is stored
    /// in**, and it is not cosmetic: `compareJsonbContainers` tests it before
    /// the element count and lets the count *overwrite* the answer, so a
    /// scalar sorts below a one- or many-element array and **above an empty
    /// one** (I41). Only [`jsonb_key`] ever sets it; a nested array is a real
    /// array at every depth.
    Array {
        raw_scalar: bool,
        items: Vec<Jsonb>,
    },
    /// Pairs in **storage** order — key length first, then bytes — which is
    /// the order the walk visits them in and is *not* the order the keys are
    /// compared in ([`Jsonb::cmp`]).
    Object(Vec<(String, Jsonb)>),
}

impl Jsonb {
    /// `JsonbValue.type`, which is the type-defined order itself (I41).
    fn rank(&self) -> u8 {
        match self {
            Self::Null => 0x0,
            Self::String(_) => 0x1,
            Self::Number(_) => 0x2,
            Self::Bool(_) => 0x3,
            Self::Array { .. } => 0x10,
            Self::Object(_) => 0x11,
        }
    }

    /// `compareJsonbContainers`, as a recursion rather than as a lockstep walk
    /// over two token streams.
    ///
    /// The two agree because both stop at the first position where the values
    /// differ, and up to that position the streams are identical: a kind
    /// mismatch anywhere is the type-defined order, and a container's size is
    /// settled at its opening token, before any member of it is looked at.
    ///
    /// **Two details are PostgreSQL's and would not be guessed.** An object is
    /// ordered by its pair *count* before its first key, so `{"z":1}` sorts
    /// below `{"a":1,"b":2}`; and the keys are compared by `varstr_cmp` while
    /// being *stored* by length-then-bytes, so the walk visits `{"z":1,"aa":2}`
    /// as `z` then `aa` and compares those keys in that order (I41).
    ///
    /// **A string leaf is where this stops being PostgreSQL's answer.**
    /// `compareJsonbScalarValue` passes `DEFAULT_COLLATION_OID` to
    /// `varstr_cmp`, so every string value and every object key is ordered by
    /// the database's collation — which a plain dump does not record (I32).
    /// Bytewise is what this build has, and
    /// `ComparisonDivergence::JsonbStringCollation` is the column's announcement
    /// of it.
    fn cmp(&self, other: &Self) -> Ordering {
        if self.rank() != other.rank() {
            return self.rank().cmp(&other.rank());
        }
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::String(a), Self::String(b)) => a.as_bytes().cmp(b.as_bytes()),
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (
                Self::Array { raw_scalar: raw_a, items: a },
                Self::Array { raw_scalar: raw_b, items: b },
            ) => {
                // Written in the source's own order, overwrite included: the
                // flag is decided first and the length then replaces the
                // answer rather than refining it.
                let mut container = Ordering::Equal;
                if raw_a != raw_b {
                    container = if *raw_a { Ordering::Less } else { Ordering::Greater };
                }
                if a.len() != b.len() {
                    container = a.len().cmp(&b.len());
                }
                container.then_with(|| first_difference(a.iter().zip(b).map(|(x, y)| x.cmp(y))))
            }
            (Self::Object(a), Self::Object(b)) => a.len().cmp(&b.len()).then_with(|| {
                first_difference(a.iter().zip(b).map(|((ka, va), (kb, vb))| {
                    ka.as_bytes().cmp(kb.as_bytes()).then_with(|| va.cmp(vb))
                }))
            }),
            _ => unreachable!("two `Jsonb` values of one rank are of one variant"),
        }
    }
}

/// The first non-`Equal` answer, or `Equal` when there is none — the
/// member-wise half of [`Jsonb::cmp`], where the walk stops at the first
/// position the two containers differ at.
fn first_difference(mut answers: impl Iterator<Item = Ordering>) -> Ordering {
    answers.find(|answer| answer.is_ne()).unwrap_or(Ordering::Equal)
}

/// How deep [`parse_jsonb`] will descend before refusing. PostgreSQL's own
/// parser recurses too and is bounded by `check_stack_depth()`, so it has a
/// limit of its own; ours is a fixed number because a Rust stack overflow
/// aborts the process where a refusal is an error a user can read. Nothing a
/// `jsonb_out` field of a real dump holds comes near it.
const JSONB_MAX_DEPTH: usize = 1000;

/// The furthest a `jsonb` number's exponent may move the decimal point. It
/// bounds the digit string this builds, and it is ours rather than
/// PostgreSQL's — `numeric` reaches further. No *field* is affected: `jsonb`
/// prints its numbers through `numeric_out`, which never writes an exponent,
/// so only a literal can reach this at all, and refusing one is the weaker
/// answer rather than the wrong one.
const JSONB_MAX_EXPONENT: i64 = 100_000;

/// A recursive-descent reader over one JSON document, implementing what
/// `jsonb_in` accepts and nothing wider (I41): RFC 8259 with PostgreSQL's two
/// extra refusals — `\u0000`, which cannot be part of a `text` value, and a
/// lone surrogate half.
struct JsonCursor<'a> {
    bytes: &'a [u8],
    at: usize,
    depth: usize,
}

impl<'a> JsonCursor<'a> {
    /// The four bytes RFC 8259 calls whitespace, which is also what the
    /// server's lexer skips — not `char::is_whitespace`, and not
    /// `array_isspace` either.
    fn skip_ws(&mut self) {
        while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.bytes.get(self.at) == Some(&byte);
        self.at += usize::from(found);
        found
    }

    fn keyword(&mut self, word: &str) -> Option<()> {
        let end = self.at + word.len();
        (self.bytes.get(self.at..end)? == word.as_bytes()).then(|| self.at = end)
    }

    /// A run of one or more ASCII digits, as the number grammar's three
    /// positions all want.
    fn digits(&mut self) -> Option<&'a str> {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        if self.at == start {
            return None;
        }
        // ASCII digits, so this can only be valid UTF-8.
        std::str::from_utf8(&self.bytes[start..self.at]).ok()
    }

    /// One value, and the only place the depth is counted.
    fn value(&mut self) -> Option<Jsonb> {
        self.depth += 1;
        if self.depth > JSONB_MAX_DEPTH {
            return None;
        }
        let value = match self.bytes.get(self.at)? {
            b'{' => {
                self.at += 1;
                self.object()?
            }
            b'[' => {
                self.at += 1;
                self.array()?
            }
            b'"' => Jsonb::String(self.string()?),
            b't' => {
                self.keyword("true")?;
                Jsonb::Bool(true)
            }
            b'f' => {
                self.keyword("false")?;
                Jsonb::Bool(false)
            }
            b'n' => {
                self.keyword("null")?;
                Jsonb::Null
            }
            // Anything else is a number or nothing: a leading `+`, a bare
            // `.5` and `NaN` all fail inside, exactly as the server's lexer
            // fails them.
            _ => Jsonb::Number(self.number()?),
        };
        self.depth -= 1;
        Some(value)
    }

    /// The body of an array, its `[` already eaten.
    fn array(&mut self) -> Option<Jsonb> {
        let mut items = Vec::new();
        self.skip_ws();
        if !self.eat(b']') {
            loop {
                self.skip_ws();
                items.push(self.value()?);
                self.skip_ws();
                if self.eat(b',') {
                    continue;
                }
                if self.eat(b']') {
                    break;
                }
                return None;
            }
        }
        Some(Jsonb::Array { raw_scalar: false, items })
    }

    /// The body of an object, its `{` already eaten. The pairs come out in
    /// storage order with duplicate keys resolved, which is the state a
    /// stored `jsonb` is always in.
    fn object(&mut self) -> Option<Jsonb> {
        let mut pairs: Vec<(String, Jsonb)> = Vec::new();
        self.skip_ws();
        if !self.eat(b'}') {
            loop {
                self.skip_ws();
                let key = self.string()?;
                self.skip_ws();
                if !self.eat(b':') {
                    return None;
                }
                self.skip_ws();
                pairs.push((key, self.value()?));
                self.skip_ws();
                if self.eat(b',') {
                    continue;
                }
                if self.eat(b'}') {
                    break;
                }
                return None;
            }
        }
        Some(Jsonb::Object(storage_order(pairs)))
    }

    /// A JSON string, its escapes resolved. An unescaped byte below `0x20` is
    /// refused, which is what the server refuses and what `escape_json` never
    /// writes — it renders every one of them as `\b`/`\f`/`\n`/`\r`/`\t` or a
    /// `\u00xx` (I41).
    fn string(&mut self) -> Option<String> {
        if !self.eat(b'"') {
            return None;
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let byte = *self.bytes.get(self.at)?;
            self.at += 1;
            match byte {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let escape = *self.bytes.get(self.at)?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut buffer = [0u8; 4];
                            out.extend_from_slice(
                                self.unicode_escape()?.encode_utf8(&mut buffer).as_bytes(),
                            );
                        }
                        _ => return None,
                    }
                }
                0x00..=0x1f => return None,
                // Every other byte is copied verbatim, and each is either
                // ASCII or part of a multi-byte sequence copied whole — the
                // input is a `&str`, so the result is valid UTF-8 by
                // construction and `from_utf8` above never fails.
                _ => out.push(byte),
            }
        }
    }

    /// `\uXXXX`, its `\u` already eaten. Surrogates must come as a matched
    /// high-then-low pair, and `\u0000` is refused outright: `jsonb` stores
    /// strings as `text`, which cannot hold a NUL (I41).
    fn unicode_escape(&mut self) -> Option<char> {
        let first = self.hex4()?;
        if first == 0 {
            return None;
        }
        if (0xd800..0xdc00).contains(&first) {
            if !(self.eat(b'\\') && self.eat(b'u')) {
                return None;
            }
            let second = self.hex4()?;
            if !(0xdc00..0xe000).contains(&second) {
                return None;
            }
            return char::from_u32(0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00));
        }
        // A low surrogate with no high half in front of it.
        char::from_u32(first).filter(|_| !(0xdc00..0xe000).contains(&first))
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.bytes.get(self.at..self.at.checked_add(4)?)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        self.at += 4;
        u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
    }

    /// A JSON number: an optional `-`, then `0` or a digit run that does not
    /// start with `0`, then an optional `.` with at least one digit, then an
    /// optional `e`/`E` exponent with an optional sign. `01`, `+1`, `.5`,
    /// `1.` and `NaN` are refused, as the server refuses them (I41).
    ///
    /// The result is the [`NumericKey`] the stored `numeric` would compare
    /// by, so the exponent is applied by moving the decimal point rather than
    /// kept: `1e2`, `100` and `100.00` are one value, which is what
    /// `numeric_cmp` says of them.
    fn number(&mut self) -> Option<NumericKey> {
        let negative = self.eat(b'-');
        let int = self.digits()?;
        if int.len() > 1 && int.starts_with('0') {
            return None;
        }
        let frac = if self.eat(b'.') { self.digits()? } else { "" };
        let mut exponent: i64 = 0;
        if matches!(self.bytes.get(self.at), Some(b'e' | b'E')) {
            self.at += 1;
            let signed = if self.eat(b'-') {
                true
            } else {
                self.eat(b'+');
                false
            };
            let digits = self.digits()?;
            // Parsed as `i64` and then bounded, so a run of digits too long
            // for the integer is refused rather than wrapping.
            exponent = digits.parse::<i64>().ok()?;
            if signed {
                exponent = -exponent;
            }
        }
        // Where the point lands, counted in digits from the left of
        // `int ++ frac`. Both directions need padding, and both are bounded.
        let point = i64::try_from(int.len()).ok()? + exponent;
        if !(-JSONB_MAX_EXPONENT..=JSONB_MAX_EXPONENT).contains(&point) {
            return None;
        }
        let digits = format!("{int}{frac}");
        let width = i64::try_from(digits.len()).ok()?;
        Some(match point {
            _ if point <= 0 => {
                NumericKey::from_parts(negative, "", &format!("{}{digits}", zeros(-point)))
            }
            _ if point >= width => {
                NumericKey::from_parts(negative, &format!("{digits}{}", zeros(point - width)), "")
            }
            _ => {
                let split = usize::try_from(point).ok()?;
                NumericKey::from_parts(negative, &digits[..split], &digits[split..])
            }
        })
    }
}

/// `n` zeros, for the padding a moved decimal point needs on either side.
fn zeros(n: i64) -> String {
    "0".repeat(usize::try_from(n).unwrap_or(0))
}

/// `uniqueifyJsonbObject`: sort the pairs into storage order and drop every
/// duplicate key but the **last** one written.
///
/// Storage order is `lengthCompareJsonbString` — key length first, then
/// `memcmp` — which is why `{"z":1,"aa":2}` is stored, printed and walked in
/// that order rather than alphabetically. The duplicate rule comes out of
/// `lengthCompareJsonbPair` breaking a tie on *descending* insertion order
/// while the uniqueify pass keeps the first of each run, which is what the
/// `reverse` here reproduces against a stable sort.
fn storage_order(mut pairs: Vec<(String, Jsonb)>) -> Vec<(String, Jsonb)> {
    pairs.reverse();
    pairs.sort_by(|(a, _), (b, _)| {
        a.len().cmp(&b.len()).then_with(|| a.as_bytes().cmp(b.as_bytes()))
    });
    pairs.dedup_by(|(a, _), (b, _)| a == b);
    pairs
}

/// One `jsonb` document, parsed. `None` for text the server's own parser would
/// refuse — which for a *field* is `Error::FieldDecode` and for a filter's
/// literal `Error::PredicateValueDecode`, the same two faults every other
/// comparison raises.
fn parse_jsonb(text: &str) -> Option<Jsonb> {
    let mut cursor = JsonCursor { bytes: text.as_bytes(), at: 0, depth: 0 };
    cursor.skip_ws();
    let value = cursor.value()?;
    cursor.skip_ws();
    (cursor.at == cursor.bytes.len()).then_some(value)
}

/// A `jsonb` comparison key: the document, with a **top-level scalar wrapped
/// in the one-element pseudo-array PostgreSQL stores it in** (I41).
///
/// The wrapper is not bookkeeping. `compareJsonbContainers` reads the
/// `rawScalar` flag and then lets the element count overwrite what it
/// concluded, so `1 < [1]`, `1 < [1,2]` and `1 > []` — the last of which no
/// "a scalar sorts below every array" rule produces, and which falls out here
/// only because the wrapping is modelled rather than special-cased.
fn jsonb_key(text: &str) -> Option<OrderKey> {
    Some(OrderKey::Jsonb(match parse_jsonb(text)? {
        container @ (Jsonb::Array { .. } | Jsonb::Object(_)) => container,
        scalar => Jsonb::Array { raw_scalar: true, items: vec![scalar] },
    }))
}

/// One side of an ordering comparison, decoded from text per the column's
/// [`CompareKind`]. Both sides of any one comparison come from the same kind,
/// so a *finite* variant mismatch is unreachable by construction.
///
/// **Three variants are not values of the column's Arrow type at all**, and
/// that is the point: `infinity`, `-infinity` and `numeric`'s `NaN` are legal
/// values of their declared PostgreSQL types with a total order (I34), while
/// `Date32` has no infinity and `Decimal128` no NaN. Deciding an order needs
/// strictly less than materializing a value, so they are carried as their
/// *position* — below every finite value, above every finite value, or above
/// `infinity` — rather than as a number that would have to be indistinguishable
/// from a real one. A `real`/`double precision` special is **not** here: IEEE
/// has all three, so the column's own decoder yields them inside `Float` and
/// [`pg_float_cmp`] already orders them PostgreSQL's way.
#[derive(Debug, Clone, PartialEq)]
enum OrderKey {
    /// PostgreSQL's `-infinity`, below every finite value of its type.
    NegativeInfinity,
    Bool(bool),
    Int(i64),
    Float(f64),
    Decimal(i256),
    /// A bare `numeric`, whose digits do not fit any fixed-width integer.
    Numeric(NumericKey),
    /// An `interval`'s span in microseconds, which is 128 bits wide because
    /// PostgreSQL's own `interval_cmp_value` is (I40).
    Interval(i128),
    /// A `time with time zone`: the UTC-equivalent instant, then the stored
    /// zone as PostgreSQL stores it — seconds *west* of GMT, the negation of
    /// the sign the value displays.
    TimeTz {
        utc: i64,
        zone: i64,
    },
    Network(NetworkKey),
    /// A `jsonb` document, with a top-level scalar already wrapped in the
    /// pseudo-array PostgreSQL stores it in — see [`jsonb_key`].
    Jsonb(Jsonb),
    Bytes(Vec<u8>),
    Text(String),
    /// PostgreSQL's `infinity`, above every finite value of its type.
    PositiveInfinity,
    /// `numeric`'s `NaN`, which orders above `infinity` and equals itself
    /// (I34). Reached only through [`CompareKind::Decimal`].
    NotANumber,
}

/// The rank of a finite value — the middle of the four [`OrderKey::rank`]
/// classes, and the only one whose members are compared by value.
const FINITE: u8 = 1;

impl OrderKey {
    /// Where this key sits in PostgreSQL's total order relative to the finite
    /// values of its own type. Ranks are compared before values are, which is
    /// what lets a special value be carried as a position instead of as a
    /// sentinel that a finite value could collide with — `Date32`'s would be
    /// free at both ends, but a `Timestamp`'s would not: `i64::MAX` micros
    /// since 1970 is a date PostgreSQL itself accepts.
    fn rank(&self) -> u8 {
        match self {
            Self::NegativeInfinity => 0,
            Self::Bool(_)
            | Self::Int(_)
            | Self::Float(_)
            | Self::Decimal(_)
            | Self::Numeric(_)
            | Self::Interval(_)
            | Self::TimeTz { .. }
            | Self::Network(_)
            | Self::Jsonb(_)
            | Self::Bytes(_)
            | Self::Text(_) => FINITE,
            Self::PositiveInfinity => 2,
            Self::NotANumber => 3,
        }
    }
}

/// PostgreSQL's special values, for the kinds whose columns can hold one and
/// in the exact spelling that type's own `*_out` writes (I34): `date`,
/// `timestamp`, `timestamptz` and `interval` write `infinity`/`-infinity`,
/// and a `numeric` writes `NaN`. Nothing else is accepted — a `date` field or
/// literal reading `Infinity` is not what `date_out` writes, so it stays a
/// decode failure, the same strictness the nested codec applies.
///
/// **`interval`'s two are read on every file, not only on a v17 one.** They
/// are v17 values, and no older server could have written one, so accepting
/// the spelling unconditionally is the union rule (I35) rather than a claim
/// about the file's own major. What it costs
/// is a *literal* an older server would have refused, which is one word in an
/// answer nobody's data can match.
///
/// **One absence is deliberate.** `real`/`double precision` are absent
/// because IEEE represents all three and [`decode::decode_f64`] already
/// returns them.
///
/// **The two `numeric` kinds differ, and only about the infinities.**
/// `apply_typmod_special` rejects `±Infinity` under any typmod (I34), so a
/// [`CompareKind::Decimal`] column — which always has one — can hold a `NaN`
/// and never an infinity, and neither can a `numeric(p,s)` past 76 digits.
/// A *bare* `numeric` has all three, in `numeric_out`'s own spellings, which
/// capitalize where `date_out`'s do not.
fn special_order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    match kind {
        CompareKind::Date | CompareKind::Timestamp { .. } | CompareKind::Interval => match text {
            "infinity" => Some(OrderKey::PositiveInfinity),
            "-infinity" => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        CompareKind::Decimal(_) => (text == "NaN").then_some(OrderKey::NotANumber),
        CompareKind::Numeric { infinities } => match text {
            "NaN" => Some(OrderKey::NotANumber),
            "Infinity" if *infinities => Some(OrderKey::PositiveInfinity),
            "-Infinity" if *infinities => Some(OrderKey::NegativeInfinity),
            _ => None,
        },
        _ => None,
    }
}

/// `interval_cmp_value`'s span, in microseconds: months collapse to 30 days,
/// days to 86400 seconds, and the time field is added on (I40). The whole
/// point of the collapse is that `1 mon`, `30 days` and `720:00:00` are one
/// value written three ways, which is why an `interval` is one of the two
/// kinds equality cannot canonicalize once and compare bytewise.
///
/// The walk over the text is [`decode::interval_parts`], shared with the
/// decoder — one reading of `interval_out`'s grammar (I40), which is what
/// keeps a literal this refuses and a field the decoder refuses the same set.
/// **The fusing is this function's alone**: it is what makes three unequal
/// triples one value, so a decoder that did it would lose the fields Arrow
/// carries separately.
fn interval_span(text: &str) -> Option<i128> {
    let (months, days, time) = decode::interval_parts(text)?;
    let whole_days = i128::from(months.checked_mul(30)?.checked_add(days)?);
    whole_days.checked_mul(86_400_000_000)?.checked_add(time)
}

/// A `time with time zone`, split into the UTC-equivalent instant and the
/// zone PostgreSQL stores — seconds *west* of GMT, which is the negation of
/// the offset the value displays (I40). `timetz_cmp_internal` sorts by the
/// first and breaks ties with the second, so `00:00:00+00` and `01:00:00+01`
/// are the same instant and still not equal.
fn timetz_key(text: &str) -> Option<OrderKey> {
    let (time_only, displayed) = decode::extract_offset(text)?;
    let (seconds, micros) = decode::parse_time_of_day(time_only)?;
    let zone = -displayed;
    let utc = seconds.checked_mul(1_000_000)?.checked_add(micros)?.checked_add(zone * 1_000_000)?;
    Some(OrderKey::TimeTz { utc, zone })
}

/// An `inet` or `cidr` value: `<address>[/<bits>]`, the form
/// `pg_inet_net_ntop` writes, with `cidr_out` always appending the netmask
/// and `inet_out` omitting it when it is the family's full width (I40).
///
/// **The address grammar is Rust's, which is narrower than `inet_in`'s.** An
/// abbreviated IPv4 address — `10`, meaning `10.0.0.0/8` — is a spelling the
/// server accepts and this refuses, the same weaker-never-wrong shape as the
/// `interval` grammar above.
///
/// `cidr` additionally refuses a value with a bit set below its netmask,
/// because `cidr_in` does: that is the *only* thing separating the two types,
/// their comparison being identical.
fn network_key(text: &str, cidr: bool) -> Option<OrderKey> {
    let (address, netmask) = match text.split_once('/') {
        Some((address, netmask)) => (address, Some(netmask)),
        None => (text, None),
    };
    let mut addr = [0u8; 16];
    let v6 = match address.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(v4) => {
            addr[..4].copy_from_slice(&v4.octets());
            false
        }
        std::net::IpAddr::V6(v6) => {
            addr.copy_from_slice(&v6.octets());
            true
        }
    };
    let maxbits: u8 = if v6 { 128 } else { 32 };
    let bits = match netmask {
        None => maxbits,
        Some(digits) => {
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            digits.parse::<u8>().ok()?
        }
    };
    if bits > maxbits {
        return None;
    }
    if cidr && (bits..maxbits).any(|bit| addr[usize::from(bit / 8)] & (0x80 >> (bit % 8)) != 0) {
        return None;
    }
    Some(OrderKey::Network(NetworkKey { v6, bits, addr }))
}

/// A `macaddr`/`macaddr8` value: `octets` lowercase hex pairs joined by
/// colons, which is what `macaddr_out` and `macaddr8_out` write (I40). The
/// server's input function takes several other separator conventions and this
/// takes none of them, for the reason the `interval` grammar gives.
fn macaddr_key(text: &str, octets: usize) -> Option<OrderKey> {
    let mut bytes = Vec::with_capacity(octets);
    for part in text.split(':') {
        // The digit check is not what `from_str_radix` does: it accepts a
        // leading sign, so `+f` would otherwise pass as a two-character pair.
        if part.len() != 2 || bytes.len() == octets || !part.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return None;
        }
        bytes.push(u8::from_str_radix(part, 16).ok()?);
    }
    (bytes.len() == octets).then_some(OrderKey::Bytes(bytes))
}

/// Decode one already-COPY-unescaped value into a comparable key. `None`
/// when the text is not a value of that type — for a *field* that is
/// `Error::FieldDecode`, exactly as the typed build path reports it; for the
/// filter's own literal it is `Error::PredicateValueDecode`, raised before a
/// row is read.
///
/// A special value is answered by [`special_order_key`] first, since it is a
/// legal value of the declared type that the *Arrow* type cannot hold — a
/// separate population from text that is genuinely malformed for the column,
/// which is what a `None` from here now means.
fn order_key(kind: &CompareKind, text: &str) -> Option<OrderKey> {
    if let Some(special) = special_order_key(kind, text) {
        return Some(special);
    }
    Some(match kind {
        CompareKind::Bool => OrderKey::Bool(decode::decode_bool(text)?),
        // Parsed as `i64` whatever the column's width: a literal outside a
        // `smallint`'s range still orders correctly against every value the
        // column can hold, and refusing it would be a refusal PostgreSQL's
        // own comparison does not need to make.
        CompareKind::Int => OrderKey::Int(text.parse::<i64>().ok()?),
        // `u32`, and the width *is* the refusal: `oidin` reads `-1` as
        // 4294967295 and this build does not implement that wrap, so a
        // signed literal is `Error::PredicateValueDecode` rather than a
        // negative key no OID could equal. Every value the column can hold
        // widens into `i64` and orders against the rest of the key space
        // unchanged.
        CompareKind::UnsignedInt => OrderKey::Int(text.parse::<u32>().ok()?.into()),
        CompareKind::Float32 => OrderKey::Float(f64::from(decode::decode_f32(text)?)),
        CompareKind::Float64 => OrderKey::Float(decode::decode_f64(text)?),
        CompareKind::Decimal(scale) => {
            OrderKey::Decimal(i256::from_string(&decode::decimal_unscaled_digits(text, *scale)?)?)
        }
        CompareKind::Numeric { .. } => OrderKey::Numeric(NumericKey::parse(text)?),
        // A label the type does not declare is not a value of the column, so
        // it is the same fault an unparseable number is: `Error::FieldDecode`
        // for a field, `Error::PredicateValueDecode` for a literal. The
        // linear scan is over a label list, which is a handful of entries in
        // every enum a dump has ever carried.
        CompareKind::Enum(labels) => OrderKey::Int(labels.iter().position(|l| l == text)? as i64),
        CompareKind::Date => OrderKey::Int(decode::decode_date32(text)?.into()),
        CompareKind::Time => OrderKey::Int(decode::decode_time64_micros(text)?),
        CompareKind::Timestamp { with_tz } => {
            OrderKey::Int(decode::decode_timestamp_micros(text, *with_tz)?)
        }
        CompareKind::Interval => OrderKey::Interval(interval_span(text)?),
        CompareKind::TimeTz => timetz_key(text)?,
        CompareKind::Network { cidr } => network_key(text, *cidr)?,
        CompareKind::MacAddr { octets } => macaddr_key(text, *octets)?,
        CompareKind::Jsonb => jsonb_key(text)?,
        CompareKind::Uuid => OrderKey::Bytes(decode::decode_uuid(text)?.to_vec()),
        CompareKind::Bytea => OrderKey::Bytes(decode::decode_bytea(text)?),
        CompareKind::Text => OrderKey::Text(text.to_string()),
        // `bcTruelen` on both sides, which is what makes this the server's
        // comparison rather than one over the padding (I38). The blank is
        // ASCII `0x20` and nothing else — a tab is a value byte, and it is
        // the byte that separates trim-and-compare from pad-and-compare.
        CompareKind::PaddedText => OrderKey::Text(text.trim_end_matches(' ').to_string()),
    })
}

/// PostgreSQL's float order, not Rust's: `NaN` is greater than every other
/// value, infinities included, and `NaN = NaN` is true — `float8_gt(a, b)` is
/// `!isnan(b) && (isnan(a) || a > b)` (I33). Rust's `partial_cmp` answers
/// `None` for either case. `real`/`double precision` are the only columns whose
/// decoder yields a NaN at all: a `NaN` in a `numeric(p,s)` column has no
/// `Decimal128` representation and fails to decode long before any
/// comparison (I4).
fn pg_float_cmp(a: f64, b: f64) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.partial_cmp(&b).expect("neither side is NaN"),
    }
}

/// Rank first, value second. A special value on either side is decided by
/// [`OrderKey::rank`] alone, so two of the same special are equal —
/// `-infinity = -infinity`, `infinity = infinity`, `NaN = NaN` (I34) — and a
/// special against a finite never has to name a number.
fn compare_keys(a: &OrderKey, b: &OrderKey) -> Ordering {
    let (rank_a, rank_b) = (a.rank(), b.rank());
    if rank_a != rank_b {
        return rank_a.cmp(&rank_b);
    }
    match (a, b) {
        (OrderKey::Bool(x), OrderKey::Bool(y)) => x.cmp(y),
        (OrderKey::Int(x), OrderKey::Int(y)) => x.cmp(y),
        (OrderKey::Float(x), OrderKey::Float(y)) => pg_float_cmp(*x, *y),
        (OrderKey::Decimal(x), OrderKey::Decimal(y)) => x.cmp(y),
        (OrderKey::Numeric(x), OrderKey::Numeric(y)) => x.cmp(y),
        (OrderKey::Interval(x), OrderKey::Interval(y)) => x.cmp(y),
        (
            OrderKey::TimeTz { utc: utc_x, zone: zone_x },
            OrderKey::TimeTz { utc: utc_y, zone: zone_y },
        ) => utc_x.cmp(utc_y).then_with(|| zone_x.cmp(zone_y)),
        (OrderKey::Network(x), OrderKey::Network(y)) => x.cmp(y),
        (OrderKey::Jsonb(x), OrderKey::Jsonb(y)) => x.cmp(y),
        (OrderKey::Bytes(x), OrderKey::Bytes(y)) => x.cmp(y),
        (OrderKey::Text(x), OrderKey::Text(y)) => x.as_bytes().cmp(y.as_bytes()),
        _ if rank_a != FINITE => Ordering::Equal,
        _ => unreachable!("both sides of a comparison decode through one column's `CompareKind`"),
    }
}

/// One side of a **nested** comparison, decoded from a container literal per
/// the column's [`NestedCompare`]. Both sides of any one comparison come from
/// the same plan, so a variant mismatch is unreachable by construction, the
/// same way it is for [`OrderKey`].
///
/// `None` in an element or field position is SQL NULL, which is a value of
/// the container rather than the absence of one: `{1,NULL}` is a two-element
/// array. **The whole field being NULL is a different fact** — that is the
/// `\N` the row carries, and it is [`Truth::Unknown`] exactly as it is for a
/// scalar column.
#[derive(Debug, Clone, PartialEq)]
enum NestedKey {
    /// A scalar position, through the leaf type's own [`OrderKey`].
    Leaf(OrderKey),
    /// An `array_out` value: its elements flattened row-major, plus the shape
    /// they were written in, because `array_cmp` decides on the shape once
    /// the elements agree (I45).
    Array { elements: Vec<Option<NestedKey>>, dims: Vec<usize>, lower_bounds: Vec<i32> },
    /// A `record_out` value: one entry per declared field, in declaration
    /// order.
    Record(Vec<Option<NestedKey>>),
    /// A range value, already through [`make_range`] — so it is in the form
    /// the server would have stored, not the form it was written in. Boxed
    /// because a bound is itself a `NestedKey`, which makes the pair
    /// mutually recursive; the vector the multirange holds is indirection
    /// enough on its own.
    Range(Box<RangeKey>),
    /// A multirange value, already sorted, coalesced and emptied out
    /// ([`canonical_multirange`]). Its members are never empty and never
    /// touch, which is what makes the comparison a plain sequence walk.
    Multirange(Vec<RangeKey>),
}

/// A range value in the form PostgreSQL itself stores, which is the only form
/// two ranges may be compared in: `range_in` runs every literal through
/// `make_range`, so `int4range '[1,10]'` and `int4range '(0,11)'` are one
/// value and the file can only ever hold `[1,11)`.
#[derive(Debug, Clone, PartialEq)]
struct RangeKey {
    /// The empty range, which `range_cmp` sorts below every other value and
    /// which is *not* the same as a range with two absent bounds — `empty`
    /// and `(,)` are different values (I46).
    empty: bool,
    lower: RangeBoundKey,
    upper: RangeBoundKey,
}

/// One bound of a range: PostgreSQL's `RangeBound`, minus the type it is a
/// bound of.
#[derive(Debug, Clone, PartialEq)]
struct RangeBoundKey {
    /// `None` is an **unbounded** bound — the server's `infinite` flag, and
    /// not a value at all. A bound holding `infinity` is a different thing
    /// and lives in the [`OrderKey`] beneath: `daterange
    /// '[2020-01-01,infinity]'` has a finite upper bound whose *value* is
    /// `infinity`, which is why `daterange_canonical` leaves it alone (I34,
    /// I46).
    value: Option<NestedKey>,
    inclusive: bool,
    /// Which end this bound is. It decides the answer whenever two bounds
    /// hold the same value, so it travels with the bound rather than being
    /// inferred from the caller — `bounds_adjacent` deliberately relabels a
    /// pair before comparing it, exactly as the server does.
    lower: bool,
}

impl RangeKey {
    /// The empty range. Its bounds are never read, and are the shape
    /// `range_serialize` leaves behind: absent and exclusive.
    fn empty() -> Self {
        RangeKey {
            empty: true,
            lower: RangeBoundKey { value: None, inclusive: false, lower: true },
            upper: RangeBoundKey { value: None, inclusive: false, lower: false },
        }
    }
}

/// Read one side of a nested comparison out of `text`.
///
/// **`input` is which grammar to read it in**, and it is the whole of what
/// separates the two sides. A *field* comes out of the dump in canonical
/// `*_out` form, so it is read with [`crate::nested`]'s strict `decode_*` —
/// the same strictness that makes decode and render inverses. A *literal* is
/// what the user typed, so it is read with the `array_in`/`record_in`
/// supersets (`parse_*`), which take `{a, b}` and `{ 1 , 2 }`.
///
/// **The leaf grammar does not widen with it.** A leaf is read by
/// [`order_key`], which implements that type's `*_out` form and no more
/// (`docs/design/architecture.md`, "A literal is read in the type's own output
/// form and no wider"), so `--filter 'p=( 1 , a )'` is refused where
/// `record_in` would have handed `" 1 "` to `int4in` and had the blanks
/// thrown away there. One rule at every depth, and it is the rule a scalar
/// column already has.
///
/// `None` is "not a value of this type", which is
/// `Error::PredicateValueDecode` for a literal and `Error::FieldDecode` for a
/// field.
fn nested_key(plan: &NestedCompare, text: &str, input: bool) -> Option<NestedKey> {
    Some(match plan {
        NestedCompare::Leaf { kind, .. } => NestedKey::Leaf(order_key(kind, text)?),
        // Refused before any value is read: `resolve_term` never builds a
        // comparison over a tree holding one.
        NestedCompare::Uncomparable { .. } => return None,
        NestedCompare::Array(element) => {
            let literal =
                if input { nested::parse_array(text) } else { nested::decode_array(text) }?;
            let mut elements = Vec::with_capacity(literal.elements.len());
            for value in &literal.elements {
                elements.push(match value {
                    Some(value) => Some(nested_key(element, value, input)?),
                    None => None,
                });
            }
            NestedKey::Array { elements, dims: literal.dims, lower_bounds: literal.lower_bounds }
        }
        // `int2vector`'s own grammar on both sides, and the elements are
        // built here rather than through `order_key`: `int2vectorout` writes
        // an `int16` and nothing else, so the codec has already produced the
        // value that would be parsed back out of the text.
        //
        // **`dims` and `lower_bounds` are `[n]` and `[0]` for every value,
        // the empty vector included.** `int2vectorin` sets `ndim = 1` and
        // `lbound1 = 0` unconditionally, where `array_out`'s `{}` is
        // zero-dimensional — so an empty `int2vector` is not the empty array,
        // and `array_cmp`'s dimension tie-breaks are constant here rather
        // than absent (I47).
        NestedCompare::Int2Vector => {
            let values = if input {
                nested::parse_int2vector(text)
            } else {
                nested::decode_int2vector(text)
            }?;
            NestedKey::Array {
                elements: values
                    .iter()
                    .map(|v| Some(NestedKey::Leaf(OrderKey::Int(i64::from(*v)))))
                    .collect(),
                dims: vec![values.len()],
                lower_bounds: vec![0],
            }
        }
        NestedCompare::Record(plans) => {
            let mut fields = if input {
                nested::parse_record(text, plans.len())?.fields
            } else {
                let mut fields = nested::decode_record(text)?.fields;
                // A zero-field composite is written `()`, and so is a
                // one-field composite holding NULL — the literal cannot tell
                // them apart, so the declared field list decides (I23).
                // `parse_record` asks the same question with the arity in
                // hand; `decode_record` cannot, so it is asked here.
                if plans.is_empty() && fields == [None] {
                    fields.clear();
                }
                fields
            };
            if fields.len() != plans.len() {
                return None;
            }
            let mut out = Vec::with_capacity(plans.len());
            for ((_, plan), value) in plans.iter().zip(fields.drain(..)) {
                out.push(match value {
                    Some(value) => Some(nested_key(plan, &value, input)?),
                    None => None,
                });
            }
            NestedKey::Record(out)
        }
        NestedCompare::Range { bound, discrete } => {
            let literal =
                if input { nested::parse_range(text) } else { nested::decode_range(text) }?;
            NestedKey::Range(Box::new(range_key(bound, &literal, *discrete, input)?))
        }
        NestedCompare::Multirange { bound, discrete } => {
            let literals = if input {
                nested::parse_multirange(text)
            } else {
                nested::decode_multirange(text)
            }?;
            let mut members = Vec::with_capacity(literals.len());
            for literal in &literals {
                members.push(range_key(bound, literal, *discrete, input)?);
            }
            NestedKey::Multirange(canonical_multirange(members, *discrete)?)
        }
    })
}

/// One range value, read through its bound's plan and then put into the form
/// the server stores it in.
///
/// **Both sides go through [`make_range`], not only the literal**, and that
/// is deliberate rather than wasteful: it is idempotent on a `range_out`
/// field by construction — the server already applied it — so one code path
/// serves both grammars, exactly as [`nested_key`]'s `input` flag does one
/// level up. A field it *did* reject would be a file contradicting its own
/// type, which everywhere else here is an error too.
fn range_key(
    bound: &NestedCompare,
    literal: &nested::RangeLiteral,
    discrete: bool,
    input: bool,
) -> Option<RangeKey> {
    let side = |text: &Option<String>, inclusive: bool, lower: bool| {
        Some(RangeBoundKey {
            value: match text {
                Some(text) => Some(nested_key(bound, text, input)?),
                None => None,
            },
            inclusive,
            lower,
        })
    };
    make_range(
        side(&literal.lower, literal.lower_inclusive, true)?,
        side(&literal.upper, literal.upper_inclusive, false)?,
        literal.empty,
        discrete,
    )
}

/// `make_range`: `range_serialize`'s type-independent checks, then the range
/// type's canonical function where it has one, then those checks again — the
/// order the server applies them in, and the reason `int4range '(1,2)'` is
/// `empty` rather than a range holding nothing.
///
/// `None` is the server's `22000`: a lower bound above its upper. That is a
/// *semantic* refusal the container grammar cannot see — `[10,1)` is
/// perfectly well-formed text — so it is raised here, where the bounds have
/// been decoded and can be compared.
fn make_range(
    lower: RangeBoundKey,
    upper: RangeBoundKey,
    empty: bool,
    discrete: bool,
) -> Option<RangeKey> {
    let serialized = serialize_range(lower, upper, empty)?;
    if !discrete || serialized.empty {
        return Some(serialized);
    }
    // `int4range_canonical` and its two siblings, which differ from each
    // other only in the width they overflow at: an exclusive lower bound
    // becomes inclusive at the successor, an inclusive upper becomes
    // exclusive at the successor.
    //
    // **The `Int` pattern is `daterange_canonical`'s `DATE_NOT_FINITE`
    // guard**, not an approximation of it. A date `infinity` decodes to
    // `OrderKey::PositiveInfinity` rather than to a day count, so it matches
    // no arm here and is left exactly as written — which is what makes
    // `[2020-01-01,infinity]` keep its inclusive upper (I34, I46).
    let successor = |bound: &RangeBoundKey| match &bound.value {
        Some(NestedKey::Leaf(OrderKey::Int(n))) => n.checked_add(1).map(|n| {
            Some(RangeBoundKey {
                value: Some(NestedKey::Leaf(OrderKey::Int(n))),
                inclusive: !bound.inclusive,
                lower: bound.lower,
            })
        }),
        // Not a finite integer bound, so the canonical function skips it.
        _ => Some(None),
    };
    let mut lower = serialized.lower;
    let mut upper = serialized.upper;
    if !lower.inclusive
        && let Some(shifted) = successor(&lower)?
    {
        lower = shifted;
    }
    if upper.inclusive
        && let Some(shifted) = successor(&upper)?
    {
        upper = shifted;
    }
    serialize_range(lower, upper, false)
}

/// `range_serialize`'s type-independent half: the out-of-order refusal, the
/// collapse to `empty`, and "an infinite boundary is never inclusive".
fn serialize_range(
    mut lower: RangeBoundKey,
    mut upper: RangeBoundKey,
    empty: bool,
) -> Option<RangeKey> {
    if empty {
        return Some(RangeKey::empty());
    }
    match compare_bound_values(&lower, &upper) {
        Ordering::Greater => return None,
        // Equal bounds are a value only when both ends include it: `[1,1]` is
        // one point and `[1,1)`, `(1,1]` and `(1,1)` are all `empty`. This
        // runs before canonicalization *and* after it, which is what makes
        // `int4range '(1,2)'` empty.
        Ordering::Equal if !(lower.inclusive && upper.inclusive) => {
            return Some(RangeKey::empty());
        }
        _ => {}
    }
    if lower.value.is_none() {
        lower.inclusive = false;
    }
    if upper.value.is_none() {
        upper.inclusive = false;
    }
    Some(RangeKey { empty: false, lower, upper })
}

/// `range_cmp_bound_values`: the bounds' held values alone, with an absent
/// bound settled by which end it is. Inclusivity is not consulted, which is
/// what separates this from [`compare_bounds`] — the emptiness test and the
/// adjacency test both need the values without it.
fn compare_bound_values(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    match (&a.value, &b.value) {
        (None, None) if a.lower == b.lower => Ordering::Equal,
        (None, None) | (None, Some(_)) => {
            if a.lower {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (Some(_), None) => {
            if b.lower {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `range_cmp_bounds`: infinity, then the held value, then inclusivity — and
/// inclusivity is where which *end* a bound is starts to matter. An exclusive
/// **lower** bound is above an inclusive one at the same value, because it
/// means "just after"; an exclusive **upper** is below, because it means
/// "just before" (I46).
fn compare_bounds(a: &RangeBoundKey, b: &RangeBoundKey) -> Ordering {
    let by_value = compare_bound_values(a, b);
    if a.value.is_none() || b.value.is_none() || by_value.is_ne() {
        return by_value;
    }
    match (a.inclusive, b.inclusive) {
        (true, true) => Ordering::Equal,
        (false, false) if a.lower == b.lower => Ordering::Equal,
        (false, _) if a.lower => Ordering::Greater,
        (false, _) => Ordering::Less,
        (true, false) if b.lower => Ordering::Less,
        (true, false) => Ordering::Greater,
    }
}

/// `range_cmp`: `empty` below everything, then lower bound, then upper.
fn compare_range(a: &RangeKey, b: &RangeKey) -> Ordering {
    match (a.empty, b.empty) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => {
            compare_bounds(&a.lower, &b.lower).then_with(|| compare_bounds(&a.upper, &b.upper))
        }
    }
}

/// `multirange_canonicalize`: sort the members, drop the empty ones, and
/// merge any two that overlap or touch. It is what makes a multirange
/// comparison a plain sequence walk — after it, no member is empty and no two
/// members meet, so the sequence is the value (I46).
///
/// `None` propagates a bound the union could not re-serialize, which the
/// shapes reaching here cannot produce; it is carried rather than unwrapped
/// because every other range fault on this path is a refusal the user reads.
fn canonical_multirange(mut members: Vec<RangeKey>, discrete: bool) -> Option<Vec<RangeKey>> {
    members.sort_by(compare_range);
    let mut out: Vec<RangeKey> = Vec::with_capacity(members.len());
    for current in members {
        if current.empty {
            continue;
        }
        let Some(last) = out.last() else {
            out.push(current);
            continue;
        };
        // The server's own order, and the middle test is the one that needs
        // the sort: `range_adjacent_internal` answers true for "either meets
        // the other", and only sorting rules out the second direction.
        if ranges_adjacent(last, &current, discrete) {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        } else if range_before(last, &current) {
            out.push(current);
        } else {
            *out.last_mut().expect("just read") = range_union(last, &current, discrete)?;
        }
    }
    Some(out)
}

/// `range_before_internal`: every point of `a` is below every point of `b`,
/// with an empty range neither before nor after anything.
fn range_before(a: &RangeKey, b: &RangeKey) -> bool {
    !a.empty && !b.empty && compare_bounds(&a.upper, &b.lower).is_lt()
}

/// `range_adjacent_internal`: the two ranges touch without overlapping,
/// in either direction.
fn ranges_adjacent(a: &RangeKey, b: &RangeKey, discrete: bool) -> bool {
    !a.empty
        && !b.empty
        && (bounds_adjacent(&a.upper, &b.lower, discrete)
            || bounds_adjacent(&b.upper, &a.lower, discrete))
}

/// `bounds_adjacent`: whether an upper bound and a lower bound meet with no
/// point between them.
///
/// Equal values are adjacent exactly when one end includes the point and the
/// other does not. **Values that differ are adjacent only in a discrete
/// range**, and the server decides that by building the range *between* them
/// with both inclusivities flipped and asking whether it came out empty —
/// which is the canonical function answering "there is no value here" rather
/// than a successor being computed twice.
fn bounds_adjacent(upper: &RangeBoundKey, lower: &RangeBoundKey, discrete: bool) -> bool {
    match compare_bound_values(upper, lower) {
        Ordering::Equal => upper.inclusive != lower.inclusive,
        Ordering::Greater => false,
        Ordering::Less => {
            discrete
                && make_range(
                    RangeBoundKey {
                        value: upper.value.clone(),
                        inclusive: !upper.inclusive,
                        lower: true,
                    },
                    RangeBoundKey {
                        value: lower.value.clone(),
                        inclusive: !lower.inclusive,
                        lower: false,
                    },
                    false,
                    discrete,
                )
                .is_some_and(|between| between.empty)
        }
    }
}

/// `range_union_internal` for two ranges already known to overlap or touch:
/// the lower of the two lower bounds, the upper of the two uppers, back
/// through [`make_range`].
fn range_union(a: &RangeKey, b: &RangeKey, discrete: bool) -> Option<RangeKey> {
    let lower =
        if compare_bounds(&a.lower, &b.lower).is_lt() { a.lower.clone() } else { b.lower.clone() };
    let upper =
        if compare_bounds(&a.upper, &b.upper).is_gt() { a.upper.clone() } else { b.upper.clone() };
    make_range(lower, upper, false, discrete)
}

/// **One NULL rule, at every level: two NULLs are equal, and NULL sorts above
/// not-NULL.** `array_cmp` and `record_cmp` carry that sentence verbatim, and
/// it covers equality and ordering alike — which is what makes a nested
/// comparison two-valued throughout, never [`Truth::Unknown`].
fn compare_slot(a: Option<&NestedKey>, b: Option<&NestedKey>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_nested(a, b),
    }
}

/// `array_cmp`/`record_cmp`, structurally.
///
/// **An array compares its elements first, and its shape only afterwards**
/// (I45) — up to the shorter array's length, then element count, then
/// dimension count, then the dimensions, then the lower bounds. That order is
/// not the one `array_eq` uses, which memcmps the shape before it looks at an
/// element; the two agree on *equality* and only `array_cmp` decides an
/// order, so this is the one to reproduce. It is why `{1,2}` is above
/// `[0:1]={1,2}` — the elements are equal and the lower bound settles it —
/// and why `{1,2}` is below `{{1,2},{3,4}}`, whose extra elements are never
/// reached.
///
/// A record's two sides always have the same arity: [`nested_key`] checks it
/// against the composite's own declared field list before building either.
fn compare_nested(a: &NestedKey, b: &NestedKey) -> Ordering {
    match (a, b) {
        (NestedKey::Leaf(a), NestedKey::Leaf(b)) => compare_keys(a, b),
        (
            NestedKey::Array { elements: ae, dims: ad, lower_bounds: al },
            NestedKey::Array { elements: be, dims: bd, lower_bounds: bl },
        ) => first_difference(
            ae.iter()
                .zip(be.iter())
                .map(|(a, b)| compare_slot(a.as_ref(), b.as_ref()))
                .chain(std::iter::once(ae.len().cmp(&be.len())))
                .chain(std::iter::once(ad.len().cmp(&bd.len())))
                .chain(ad.iter().zip(bd.iter()).map(|(a, b)| a.cmp(b)))
                .chain(al.iter().zip(bl.iter()).map(|(a, b)| a.cmp(b))),
        ),
        (NestedKey::Record(a), NestedKey::Record(b)) => first_difference(
            a.iter().zip(b.iter()).map(|(a, b)| compare_slot(a.as_ref(), b.as_ref())),
        ),
        (NestedKey::Range(a), NestedKey::Range(b)) => compare_range(a, b),
        // `multirange_cmp`: member-wise, and the shorter one first where the
        // members it has all agree — which is how the server phrases it too,
        // by treating a missing member as an empty range and `empty` as the
        // lowest value there is.
        (NestedKey::Multirange(a), NestedKey::Multirange(b)) => first_difference(
            a.iter()
                .zip(b.iter())
                .map(|(a, b)| compare_range(a, b))
                .chain(std::iter::once(a.len().cmp(&b.len()))),
        ),
        _ => unreachable!("both sides of a comparison are read through one column's plan"),
    }
}

/// A `macaddr`/`macaddr8` literal in `macaddr_out`'s own spelling: `octets`
/// lowercase hex pairs joined by colons (I40). The whole of that output
/// function is those two rules, which is why this type canonicalizes where
/// the two beside it decode per row — see [`equality_comparison`].
fn render_macaddr(text: &str, octets: usize) -> Option<String> {
    let OrderKey::Bytes(bytes) = macaddr_key(text, octets)? else {
        unreachable!("`macaddr_key` yields its octets")
    };
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"))
}

/// How `=`/`!=` compare two values of a column of `kind`, given the filter's
/// own literal — settled once, when the block's schema resolves.
///
/// **Three canonicalizations, not two states**, and which one a kind takes is
/// decided by one question: is the file's `*_out` text a *unique* spelling of
/// the value it holds?
///
/// - **[`Comparison::Canonical`] — the literal rendered once.** The field is
///   already in `*_out` form and that form is unique, so rendering the
///   literal into it makes the per-row comparison a byte comparison and
///   nothing on the hot path moves. `=` on a `text` or `varchar` column is
///   exactly the compare it always was, since `text`'s rendering is the
///   identity.
/// - **[`Comparison::Trimmed`] — the field narrowed per row.** `character(n)`
///   alone. The dump writes every value padded to `n` and `bpchareq` strips
///   the padding from both sides (I38), so the trim is the comparison. It is
///   admissible on the per-row path because it is not a decode: a reverse
///   scan for `0x20` yielding a shorter slice, no allocation.
/// - **[`Comparison::Decoded`] — both sides decoded per row.** For the kinds
///   where `*_out` is *not* injective over the values one file can hold, so
///   no rendering of the literal can make the comparison bytewise.
///
/// **Five kinds decode, and the spec that named two was short by three.** A
/// bare `numeric` keeps its display scale, so `1.5` and `1.50` are one value
/// written two ways (I33); an `interval` collapses months and days, so
/// `1 mon`, `30 days` and `720:00:00` are one value written three ways (I40);
/// `jsonb` prints its numbers through `numeric_out` and compares them by
/// value, so `{"a": 1.50}` and `{"a": 1.5}` are one document written two ways
/// (I41); and `real`/`double precision` have two zeros, `-0` being a value a
/// dump can write and `float8eq` calling it equal to `0`.
///
/// **Two more decode for a different reason, and it is a reason about this
/// build rather than about PostgreSQL.** `time with time zone` and
/// `inet`/`cidr` *are* uniquely spelled by their output functions — but
/// reproducing those spellings means re-implementing `EncodeTimeOnly` plus
/// `EncodeTimezone`, and `pg_inet_net_ntop`'s IPv6 zero-run compression.
/// Writing an output function to save a fixed-size parse per row is the wrong
/// trade, and getting one subtly wrong is a silently empty result. `macaddr`
/// goes the other way for the same test: its output rule is two sentences
/// long, so it renders.
///
/// `None` when the literal is not a value of the column's type at all, which
/// is `Error::PredicateValueDecode` — the same refusal an ordering operator
/// makes, on the same output-form-only grammar (`docs/design/architecture.md`,
/// "Ordering operators compare typed").
fn equality_comparison(kind: &CompareKind, text: &str) -> Option<Comparison> {
    use CompareKind as K;
    let rendered = match kind {
        // Both sides per row. `order_key` reads a special value on the way,
        // so `NaN = NaN` and `Infinity = Infinity` come out of the same rank
        // rule the ordering operators use (I34).
        K::Float32
        | K::Float64
        | K::Numeric { .. }
        | K::Interval
        | K::Jsonb
        | K::TimeTz
        | K::Network { .. } => {
            return Some(Comparison::Decoded { kind: kind.clone(), bound: order_key(kind, text)? });
        }
        K::PaddedText => return Some(Comparison::Trimmed(text.trim_end_matches(' ').to_string())),
        // A special value is written in its own type's `*_out` spelling on
        // both sides, so it renders to itself. Only `date`, `timestamp` and
        // `numeric(p,s)` reach this arm and admit one; the other two kinds
        // `special_order_key` answers for decode above.
        _ if special_order_key(kind, text).is_some() => text.to_string(),
        K::Bool => decode::render_bool(decode::decode_bool(text)?).to_string(),
        K::Int => text.parse::<i64>().ok()?.to_string(),
        K::UnsignedInt => text.parse::<u32>().ok()?.to_string(),
        K::Decimal(scale) => {
            decode::render_decimal(&decode::decimal_unscaled_digits(text, *scale)?, *scale)
        }
        // A label is its own canonical form; what the lookup buys is the
        // refusal, since a string the type does not declare is not a value of
        // the column and equality against it is a fault rather than a miss.
        K::Enum(labels) => labels.iter().find(|label| label.as_str() == text)?.clone(),
        K::Date => decode::render_date32(decode::decode_date32(text)?),
        K::Time => decode::render_time64_micros(decode::decode_time64_micros(text)?),
        K::Timestamp { with_tz } => decode::render_timestamp_micros(
            decode::decode_timestamp_micros(text, *with_tz)?,
            *with_tz,
        ),
        K::MacAddr { octets } => render_macaddr(text, *octets)?,
        K::Uuid => decode::render_uuid(&decode::decode_uuid(text)?),
        K::Bytea => decode::render_bytea(&decode::decode_bytea(text)?),
        // The identity, and that is the point: `=` on a text column is the
        // byte comparison it has always been, with no per-row work added.
        K::Text => text.to_string(),
    };
    Some(Comparison::Canonical(rendered))
}

/// The form a **nested** literal has to be written in, as one clause of
/// `Error::PredicateValueDecode`'s sentence.
///
/// It names the container's own grammar and then says how to spell what is
/// inside it: as the dump does, which is each leaf type's own output form.
/// **That second half is advice, not the boundary of what is accepted** — the
/// leaf is read by [`order_key`], whose integer arms are `str::parse` and so
/// take a leading `+` and leading zeros that no `*_out` writes
/// (`docs/design/architecture.md`, "A literal is read in the type's own output
/// form and no wider", whose exception this is). Stating the dump's form is
/// still the useful sentence, because what the container's leniency about
/// whitespace and quoting does *not* extend to is the element, and that is
/// what refuses most literals. There is no per-leaf clause list here — the
/// leaf that failed is not reported by [`nested_key`], which answers only "not
/// a value of this type" — so the sentence points at the property rather than
/// at a position.
fn nested_accepted_form(plan: &NestedCompare) -> String {
    // The one form with no leaf clause to add, because it has no leaf: an
    // `int2vector`'s elements are read by `int2vectorin` itself, not handed
    // to some element type's own input function, so the superset reaches all
    // the way down and the sentence below would be false here.
    if matches!(plan, NestedCompare::Int2Vector) {
        return "as whole numbers from -32768 to 32767 separated by spaces, and as nothing at \
                all for the empty vector"
            .to_string();
    }
    let container = match plan {
        NestedCompare::Array(_) => "as an array literal — `{a,b}`, `{}`, a bare `NULL` element",
        NestedCompare::Record(_) => "as a composite literal — `(a,b)`, a field left empty for NULL",
        NestedCompare::Range { .. } => {
            "as a range literal — `[a,b)`, `empty`, a bound left empty for unbounded — whose \
             lower bound is not above its upper"
        }
        NestedCompare::Multirange { .. } => {
            "as a multirange literal — `{[a,b),[c,d)}`, `{}` — each member a range whose lower \
             bound is not above its upper"
        }
        // Answered above, before the leaf clause this arm cannot carry.
        NestedCompare::Int2Vector => "as an int2vector literal",
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
/// **It lives beside the grammar rather than beside [`CompareKind`]** because
/// it describes what [`order_key`] and [`equality_comparison`] accept, and
/// those are here: a widening or a tightening of either has this function in
/// the same file and the same screen, where a phrase carried on the L2 type
/// would drift from the L4 code that decides it.
///
/// **What it buys is a sentence about this build's grammar rather than about
/// the type.** Without it the refusal reads as a claim that `true` is not a
/// `boolean`, which is false; the grammar these two functions implement is
/// each type's `*_out` form and no wider (`docs/design/architecture.md`, "A
/// literal is read in the type's own output form and no wider"), so the one
/// thing the user is missing is what that form looks like. `jsonb` is the
/// exception there and so needs the least here — its grammar is the whole of
/// `jsonb_in`, so "a JSON document" is the complete answer.
///
/// **Two arms answer with the kind's own payload, because there the payload
/// *is* the answer.** An enum's declared labels are the whole of what a
/// refused enum literal is missing, and a `numeric(p,s)`'s scale is what makes
/// the clause true at all — see [`ENUM_LABELS_SHOWN`] and
/// [`decimal_accepted_form`]. Nothing forbids a diagnostic naming resolved
/// schema data; the question at each arm is whether the payload answers the
/// user's question, which for the rest of the table it does not.
///
/// [`CompareKind::Text`] and [`CompareKind::PaddedText`] never refuse a
/// literal — every string is a value of a text column — so their arm is
/// unreachable rather than wrong; it is written out anyway because a
/// `unreachable!` here would turn a future kind's mistake into a panic on a
/// diagnostic path. An enum with no labels is unreachable for a second
/// reason — `pgtype::comparison_for` refuses such a column outright — and is
/// written out the same way.
fn accepted_form(kind: &CompareKind) -> String {
    use CompareKind as K;
    match kind {
        K::Bool => "`t` or `f`".into(),
        K::Int => "as an optionally signed whole number".into(),
        // The width *is* the refusal: `oidin` wraps a negative and this does
        // not, so the range is the useful half of the sentence.
        K::UnsignedInt => "as a whole number from 0 to 4294967295".into(),
        K::Float32 | K::Float64 => "as a number, or `Infinity`, `-Infinity` or `NaN`".into(),
        // A typmod rejects an infinity (I34), so a `numeric(p,s)` — and a
        // `numeric` past 76 digits — has `NaN` and nothing else. Only the
        // typed arm carries a scale to be finer than: the `p > 76` column is
        // compared as text through `NumericKey`, which normalizes rather than
        // rescaling and so refuses no literal for its shape.
        K::Decimal(scale) => decimal_accepted_form(*scale),
        K::Numeric { infinities: false } => "as a number, or `NaN`".into(),
        K::Numeric { infinities: true } => {
            "as a number, or `NaN`, `Infinity` or `-Infinity`".into()
        }
        K::Enum(labels) if !labels.is_empty() => enum_accepted_form(labels),
        K::Enum(_) => "as one of the type's own declared labels".into(),
        K::Date => "`YYYY-MM-DD`, optionally suffixed ` BC`, or `infinity`/`-infinity`".into(),
        K::Time => "`HH:MM:SS`, optionally with a fractional second".into(),
        K::Timestamp { with_tz: false } => {
            "`YYYY-MM-DD HH:MM:SS`, optionally with a fractional second and suffixed ` BC`, or \
             `infinity`/`-infinity`"
                .into()
        }
        K::Timestamp { with_tz: true } => {
            "`YYYY-MM-DD HH:MM:SS+HH`, the offset required, optionally with a fractional second \
             and suffixed ` BC`, or `infinity`/`-infinity`"
                .into()
        }
        K::TimeTz => {
            "`HH:MM:SS+HH`, the offset required, optionally with a fractional second".into()
        }
        // `interval_out` under `IntervalStyle = postgres` (I4), which is the
        // only style a `pg_dump` connection writes.
        K::Interval => {
            "the way `interval` prints it — `1 year 2 mons 3 days`, `-01:00:00`, `00:00:00` for \
             zero — or `infinity`/`-infinity`"
                .into()
        }
        K::Network { cidr: false } => {
            "as a full IPv4 or IPv6 address, optionally followed by `/bits`".into()
        }
        K::Network { cidr: true } => {
            "as a full IPv4 or IPv6 address followed by `/bits`, with no bit set below the netmask"
                .into()
        }
        K::MacAddr { octets: 8 } => "as eight colon-separated hex pairs".into(),
        K::MacAddr { .. } => "as six colon-separated hex pairs".into(),
        K::Uuid => "as 32 hex digits, grouped `8-4-4-4-12`".into(),
        K::Bytea => "as `\\x` followed by hex pairs".into(),
        K::Jsonb => "as a JSON document".into(),
        K::Text | K::PaddedText => "as any text".into(),
    }
}

/// How many of an enum's labels [`accepted_form`] names before it stops
/// counting them out.
///
/// **A count cap rather than a length cap**, which is the other shape this
/// project uses for unbounded file-derived text (`map.rs`'s `TEXT_CAP`): every
/// label a message prints is printed whole, where a length cap would cut one
/// mid-word and hand the user a spelling that is not a label. Nothing bounds
/// how many labels a type declares, and a generated schema with a few hundred
/// of them turns an uncapped clause into a message that scrolls the error
/// itself off screen.
///
/// **The overflow clause has somewhere to send the reader**: `pgdq info
/// --verbose` prints every label of an enum column *and* lists every
/// user-defined type with its labels, both uncapped
/// (`docs/design/architecture.md`, "CLI surface"). A terse rendering is
/// licensed by a complete one existing where the user can reach it.
const ENUM_LABELS_SHOWN: usize = 12;

/// The enum clause: the declared labels themselves, which are the whole of
/// what a refused enum literal is missing — a mistyped or wrong-case label is
/// the only way to fail an enum filter, so the arm that could not answer was
/// the arm that always fires.
///
/// Each label is single-quoted with any interior quote doubled, the spelling
/// the dump's own `CREATE TYPE … AS ENUM (…)` writes and the CLI's `dequote`
/// accepts,
/// so a printed label pastes straight back into `--filter "col=<label>"`. The
/// CLI's own `label_list` renders the same way for the same reason and is not
/// shared with it: it sits a layer above this one.
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
            "as one of the type's declared labels: {list}, and {more} more; see `info --verbose`"
        ),
    }
}

/// The `numeric(p,s)` clause, which is about the **scale** because that is
/// what the refusal is about: `decode::decimal_unscaled_digits` drops a
/// trailing digit only when it is zero, so `--filter 'price>1.005'` on a
/// `numeric(10,2)` is refused — and `1.005` is a number, which makes the
/// scale-free clause false rather than merely narrow.
///
/// Precision says nothing here and is not carried by
/// [`CompareKind::Decimal`]: a literal wider than the column can hold still
/// compares against every value in it.
///
/// A negative scale is legal from PostgreSQL 15 and means the column stores
/// multiples of a power of ten, which the decoder enforces by refusing to drop
/// a non-zero digit off the integer part.
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

/// What one term compares, settled once when the block's schema resolves
/// rather than per row.
#[derive(Debug, Clone)]
enum Comparison {
    /// The four ordering operators: the field is decoded per row with the
    /// column's own decoder and its key compared against `bound`.
    Ordered { kind: CompareKind, bound: OrderKey },
    /// `=`/`!=` against the literal already rendered into the `*_out` form the
    /// file holds — a byte comparison per row (see [`equality_comparison`]).
    Canonical(String),
    /// `=`/`!=` on a `character(n)` column, against the literal with its own
    /// trailing blanks already gone.
    Trimmed(String),
    /// `=`/`!=` where the field has to be decoded per row too, because the
    /// file's spelling of a value is not unique.
    Decoded { kind: CompareKind, bound: OrderKey },
    /// A nested column, under **any** comparing operator: both sides read
    /// into a [`NestedKey`] and compared structurally.
    ///
    /// **One variant for both operator families**, where a scalar has three,
    /// and the reason is that the equality fast path a scalar gets is not
    /// available here for free. A nested value's `=` is byte-comparable only
    /// when *every* leaf beneath it canonicalizes — one `numeric` or
    /// `character(n)` element anywhere makes the rendered literal the wrong
    /// answer — so the fast path would be a second literal-side walk that
    /// exists to be right about a shape the oracle carries no case for. The
    /// structural walk is one path, and it is the path the four ordering
    /// operators already needed.
    ///
    /// *Rejected: rendering the literal back and comparing bytes where every
    /// leaf allows it.* It buys a per-row byte comparison on a filter over an
    /// array or composite column, and costs a second code path whose only
    /// coverage would be unit tests: every nested case in the comparison
    /// oracle canonicalizes, so the *decoded* half — the half that has to be
    /// right when it does not — would be the untested one.
    /// Boxed: a `NestedKey` carries three vectors, and this variant is the
    /// only large one in an enum that sits inside every resolved leaf.
    Nested(Box<NestedComparison>),
}

/// What a nested term compares: the column's plan, and the literal already
/// read through it.
#[derive(Debug, Clone)]
struct NestedComparison {
    plan: NestedCompare,
    bound: NestedKey,
}

/// Everything a comparing term needs, settled once when the block's schema
/// resolves rather than per row.
#[derive(Debug, Clone)]
struct ComparedTerm {
    column: String,
    comparison: Comparison,
    /// The declared PostgreSQL type, for `Error::FieldDecode`'s context and
    /// for [`ComparisonNote::message`].
    declared_type: String,
    /// The register's divergences for this column, already filtered to the
    /// ones that reach *this* term's operator
    /// ([`ComparisonDivergence::affects_equality`]).
    ///
    /// **A list, because a nested column has a position per divergence.** A
    /// scalar column has at most one and it is about the column itself; a
    /// composite can be on the database's collation twice, through two
    /// different fields, and a user who is told about one of them has been
    /// told half of it. Each entry carries the position's own path and
    /// declared type, which is what makes the sentence name the right type.
    divergences: Vec<(Option<String>, String, ComparisonDivergence)>,
}

/// One filter term resolved against one `COPY` block: the operator, the
/// field index it reads, and the comparison it will make.
///
/// The index is into the block's **unprojected** column list, because that is
/// what the raw row's fields are numbered by: a term may name a column the
/// projection dropped.
///
/// **It carries its own operator** rather than being read back against the
/// [`Predicate`] it came from. A resolved filter is a tree
/// ([`ResolvedExpr`]) whose shape mirrors the caller's [`Expr`], and walking
/// two trees in lockstep to pair a leaf with its operator is an invariant
/// nothing checks; carrying the operator makes the resolved tree evaluable
/// on its own.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTerm {
    op: PredicateOp,
    index: usize,
    /// `None` for `IS NULL`/`IS NOT NULL`, the two operators that compare
    /// nothing.
    compared: Option<ComparedTerm>,
}

impl ResolvedTerm {
    /// This term's divergences from PostgreSQL's own comparison. Empty for
    /// the two NULL tests, and for every term whose column answers this
    /// operator the way the server does; more than one only for a nested
    /// column with more than one diverging position.
    pub(crate) fn comparison_notes(&self) -> Vec<ComparisonNote> {
        let Some(compared) = self.compared.as_ref() else { return Vec::new() };
        compared
            .divergences
            .iter()
            .map(|(path, declared_type, divergence)| ComparisonNote {
                column: compared.column.clone(),
                path: path.clone(),
                declared_type: declared_type.clone(),
                divergence: *divergence,
            })
            .collect()
    }
}

/// The refusal an ordering operator earns on a column that cannot carry one.
/// Three reasons, each a different fact about the column.
const NOT_MAPPED: &str = "the column's declared type did not resolve to an Arrow type, so it has no order of its own \
     (`--schema-mode strings` resolves no column, by design)";
const NESTED: &str = "the column is nested (array, composite, range or multirange), and an order over such a \
     literal is not defined here";

/// The refusal a nested column earns when its *shape* is compared here and
/// one position beneath it is not — an element, a field or a bound whose own
/// declared type has no order (`json`, `box`, an unrecognised name). It names
/// the position and its type, because that is the whole of what the user has
/// to change.
fn nested_refusal(path: &str, declared: &str) -> String {
    format!(
        "the column is nested and `{path}` inside it is `{declared}`, which has no order here — \
         PostgreSQL refuses the same comparison, since a container is ordered by its element \
         type's own comparison and this type has none"
    )
}
const NO_ORDER: &str = "this build defines no ordering for the column's declared type";

/// The sentence for a column the register can answer **no** operator on,
/// worded here rather than in `crate::pgtype` for the same reason
/// [`accepted_form`] is: L2 carries the fact, L3 says it in a sentence about
/// the comparison a filter was going to make.
fn unanswerable_reason(reason: &UnanswerableReason) -> String {
    match reason {
        UnanswerableReason::RangeCanonical { range_type, function } => format!(
            "the range type `{range_type}` declares a canonical function (`{function}`), \
             which PostgreSQL applies to every value of it before storing or comparing \
             one — arbitrary server-side code this build cannot run, so two spellings the \
             server calls one value would be two values here"
        ),
    }
}

/// Resolve one filter term against the block's **unprojected**
/// [`ResolvedSchema`], at `index` — the column's position, already looked up
/// by the caller.
///
/// The two NULL tests need nothing. Every other operator reads the column's
/// [`ComparisonPlan`] and decodes the filter's own literal here, so a value
/// that is not of the column's type is a fault reported once rather than a
/// filter that matches nothing.
///
/// **The two operator families part company on a column with no plan.** An
/// ordering operator is *refused*, before a row of this block flows, unless
/// the column resolved `Mapped` with a [`NestedPlan::Scalar`] plan and the
/// register gave it a comparison. `Eq`/`Ne` fall back to the string
/// comparison instead, which is the comparison every column made before this
/// and is right for the same reason: the file holds canonical `*_out` form.
/// So a nested column still answers `=` and still refuses `<`.
///
/// **Nothing here reads the Arrow type.** How a column compares is an L2
/// conclusion resolution already reached
/// (`crate::pgtype::comparison_for`), carried in
/// [`ResolvedSchema::comparisons`]; this layer asks how the column compares,
/// never what it was mapped to.
pub(crate) fn resolve_term(
    predicate: &Predicate,
    index: usize,
    resolved: &ResolvedSchema,
    header_offset: u64,
) -> Result<ResolvedTerm> {
    if matches!(predicate.op, PredicateOp::IsNull | PredicateOp::IsNotNull) {
        return Ok(ResolvedTerm { op: predicate.op, index, compared: None });
    }
    let ordering = predicate.op.is_ordering();
    let refuse = |reason: &str| Error::UnorderedPredicateColumn {
        header_offset,
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        reason: reason.to_string(),
    };
    let declared_type = resolved.notes[index].declared.clone().unwrap_or_default();
    // `value` is `Some` for every operator but the two NULL tests; an
    // embedder that builds a `Gt` term without one gets the same fault as an
    // unparseable literal, named the same way.
    let text = predicate.value.as_deref().unwrap_or_default();
    // `kind` is what knows which grammar was applied, so the refusal is built
    // where it is in scope — which is every site that can raise it, since a
    // column with no `Compared` plan either refuses the operator outright or
    // falls back to a text comparison that cannot fail.
    let refuse_literal = |kind: &CompareKind| Error::PredicateValueDecode {
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        value: text.to_string(),
        declared_type: declared_type.clone(),
        accepted: accepted_form(kind),
    };
    let mut plan = Some(&resolved.comparisons[index]);
    // The nested tree a column fell out of, kept so the bytewise `=` below
    // can say what that fallback costs — which is a fact about the position
    // that refused the order, not about the column's own declared type.
    let mut fell_back: Option<&NestedCompare> = None;
    if resolved.columns[index] != ColumnResolution::Mapped {
        if ordering {
            return Err(refuse(NOT_MAPPED));
        }
        plan = None;
    } else if let Some(ComparisonPlan::Unanswerable(reason)) = plan {
        // The one refusal that does not end by offering `=`/`!=`: the file
        // says the server's equality is not a comparison of the text it
        // holds, so the fall-through below would be a wrong answer rather
        // than a weaker one. The two NULL tests have already returned — they
        // read no value and need no comparison.
        return Err(Error::UncomparablePredicateColumn {
            header_offset,
            column: predicate.column.clone(),
            op: predicate.op.symbol(),
            reason: unanswerable_reason(reason),
        });
    } else if let Some(ComparisonPlan::Nested(tree)) = plan {
        // A nested column whose shape is compared here but one of whose
        // positions is not: the ordering operators are refused naming that
        // position, and `=`/`!=` fall back to a byte comparison of the
        // container's whole text — **announced**, not silent, because the
        // position that took the order away is also what makes the fallback
        // an answer the server does not have (`array_cmp` raises for a
        // `json` element rather than returning a comparison).
        if let Some((path, declared)) = tree.uncomparable() {
            if ordering {
                return Err(refuse(&nested_refusal(&path, &declared)));
            }
            fell_back = Some(tree);
            plan = None;
        }
    } else if resolved.plans[index] != NestedPlan::Scalar {
        // A column the *resolver* calls nested and the register does not.
        // One shape reaches it — a range whose DDL stated no `subtype`, so
        // the resolver keeps the struct with `Utf8View` bounds and the
        // register has no bound type to name a refusal after — and it is
        // also where the two walks would land if they ever came to disagree
        // about a declared type, which matters because the alternative to
        // refusing is ordering a container's literal with a *scalar*
        // comparison, silently.
        if ordering {
            return Err(refuse(NESTED));
        }
        plan = None;
    }
    let (comparison, divergences) = match plan {
        Some(ComparisonPlan::Compared { kind, divergence }) => {
            let comparison = if ordering {
                Comparison::Ordered {
                    kind: kind.clone(),
                    bound: order_key(kind, text).ok_or_else(|| refuse_literal(kind))?,
                }
            } else {
                equality_comparison(kind, text).ok_or_else(|| refuse_literal(kind))?
            };
            (
                comparison,
                divergence
                    .filter(|d| ordering || d.affects_equality())
                    .map(|d| (None, declared_type.clone(), d))
                    .into_iter()
                    .collect(),
            )
        }
        // A nested column, compared structurally: the literal is read once,
        // in the `array_in`/`record_in` superset, and the field per row in
        // the strict `*_out` grammar the dump holds.
        Some(ComparisonPlan::Nested(tree)) => (
            Comparison::Nested(Box::new(NestedComparison {
                plan: tree.clone(),
                bound: nested_key(tree, text, true).ok_or_else(|| Error::PredicateValueDecode {
                    column: predicate.column.clone(),
                    op: predicate.op.symbol(),
                    value: text.to_string(),
                    declared_type: declared_type.clone(),
                    accepted: nested_accepted_form(tree),
                })?,
            })),
            tree.divergences()
                .into_iter()
                .filter(|(_, _, d)| ordering || d.affects_equality())
                .map(|(path, declared, d)| (Some(path), declared, d))
                .collect(),
        ),
        // No plan at all: an ordering operator has already been refused, so
        // this is `Eq`/`Ne` on a column the register does not compare — the
        // string comparison every column made before this one, which the
        // file's own canonical form is what makes right.
        _ if ordering => return Err(refuse(NO_ORDER)),
        //
        // **What it announces comes from one of two places**, and which one
        // is whether a nested tree sent the column here.
        //
        // A column that *fell out of a tree* announces per position, exactly
        // as the structural arm above does: the position that refused the
        // order carries what the bytewise fallback costs there — `AsText`
        // for a `json` element, since `array_cmp` raises rather than
        // comparing and the server has no `=` for the container either — and
        // any other position whose divergence reaches equality is announced
        // beside it. A position the *resolver* declined instead (I22, I26)
        // never reaches here; its column is not `Mapped`.
        //
        // Every other column announces off its own **resolution**, and only
        // two of those outcomes are claims this build cannot stand behind:
        // `UnknownType` and `OpaqueBaseType` say the file *named* a type and
        // this build models nothing for it — `box`, `money`, a C-level base
        // type — and `box_eq` compares areas, so bytewise is a guess there
        // ([`ComparisonDivergence::UnmodelledType`]). Every other outcome is
        // silent: a column with no DDL behind it at all — `--data-only`,
        // `--schema-mode strings` — has nothing said about its type to
        // qualify, which `ResolvedSchema::notes` reports on L2 anyway.
        _ => (
            Comparison::Canonical(text.to_string()),
            match fell_back {
                Some(tree) => tree
                    .divergences()
                    .into_iter()
                    .filter(|(_, _, d)| d.affects_equality())
                    .map(|(path, declared, d)| (Some(path), declared, d))
                    .collect(),
                None => matches!(
                    resolved.columns[index],
                    ColumnResolution::UnknownType | ColumnResolution::OpaqueBaseType
                )
                .then(|| (None, declared_type.clone(), ComparisonDivergence::UnmodelledType))
                .into_iter()
                .collect(),
            },
        ),
    };
    Ok(ResolvedTerm {
        op: predicate.op,
        index,
        compared: Some(ComparedTerm {
            column: predicate.column.clone(),
            comparison,
            declared_type,
            divergences,
        }),
    })
}

impl ResolvedTerm {
    /// Evaluate this term against `raw_row`, in SQL's three-valued domain.
    /// `table` and `row_offset` are context for the one error this can
    /// raise: a field that does not decode as its mapped type under a
    /// comparison that reads it, which is `Error::FieldDecode`, worded
    /// exactly as the typed build path words it.
    ///
    /// **A NULL field is [`Truth::Unknown`]** under every comparing
    /// operator. Four operators answer two-valued instead, and they are
    /// exactly the ones that exist because unknown swallows everything else:
    /// `IsNull`/`IsNotNull`, which compare nothing, and the two `IS DISTINCT
    /// FROM` forms, which count NULL as a value — so `IsDistinctFrom` on a
    /// NULL field is `True` where `Ne` is `Unknown`.
    fn eval(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        let field = split.field(raw_row.bytes(), self.index);
        let decoded = match field {
            Some(f) => raw_row.decode(f)?,
            None => None,
        };
        let Some(compared) = self.compared.as_ref() else {
            return Ok(Truth::of(match self.op {
                PredicateOp::IsNull => decoded.is_none(),
                _ => decoded.is_some(),
            }));
        };
        let Some(text) = decoded else {
            return Ok(match self.op {
                PredicateOp::IsDistinctFrom => Truth::True,
                PredicateOp::IsNotDistinctFrom => Truth::False,
                _ => Truth::Unknown,
            });
        };
        let key = |kind: &CompareKind| {
            order_key(kind, &text).ok_or_else(|| Error::FieldDecode {
                table: table.to_string(),
                column: compared.column.clone(),
                row_offset,
                declared_type: compared.declared_type.clone(),
                value: text.to_string(),
            })
        };
        Ok(Truth::of(match &compared.comparison {
            Comparison::Ordered { kind, bound } => {
                let ord = compare_keys(&key(kind)?, bound);
                match self.op {
                    PredicateOp::Lt => ord.is_lt(),
                    PredicateOp::Le => ord.is_le(),
                    PredicateOp::Gt => ord.is_gt(),
                    _ => ord.is_ge(),
                }
            }
            Comparison::Canonical(bound) => (text.as_ref() == bound.as_str()) == self.wants_equal(),
            Comparison::Trimmed(bound) => {
                (text.trim_end_matches(' ') == bound.as_str()) == self.wants_equal()
            }
            Comparison::Decoded { kind, bound } => {
                compare_keys(&key(kind)?, bound).is_eq() == self.wants_equal()
            }
            // Two-valued throughout: a NULL *inside* the container is a value
            // of it, and only the whole field being NULL is unknown — which
            // was decided above, before any of this runs.
            Comparison::Nested(nested) => {
                let field =
                    nested_key(&nested.plan, &text, false).ok_or_else(|| Error::FieldDecode {
                        table: table.to_string(),
                        column: compared.column.clone(),
                        row_offset,
                        declared_type: compared.declared_type.clone(),
                        value: text.to_string(),
                    })?;
                let ord = compare_nested(&field, &nested.bound);
                match self.op {
                    PredicateOp::Lt => ord.is_lt(),
                    PredicateOp::Le => ord.is_le(),
                    PredicateOp::Gt => ord.is_gt(),
                    PredicateOp::Ge => ord.is_ge(),
                    _ => ord.is_eq() == self.wants_equal(),
                }
            }
        }))
    }

    /// Whether this term keeps the rows its comparison called equal — `Eq`
    /// and `IsNotDistinctFrom` do, `Ne` and `IsDistinctFrom` do not, and no
    /// other operator reaches it.
    fn wants_equal(&self) -> bool {
        matches!(self.op, PredicateOp::Eq | PredicateOp::IsNotDistinctFrom)
    }
}

/// One [`Expr`] resolved against one `COPY` block: the same tree, with each
/// leaf replaced by the [`ResolvedTerm`] that block's schema produced. It is
/// what a block's `Active` state carries, and it is evaluable on its own —
/// nothing walks it beside the caller's `Expr`.
#[derive(Debug, Clone)]
pub(crate) enum ResolvedExpr {
    Term(ResolvedTerm),
    And(Vec<ResolvedExpr>),
    Or(Vec<ResolvedExpr>),
    Not(Box<ResolvedExpr>),
}

impl ResolvedExpr {
    /// Whether `raw_row` survives this filter: its root evaluates
    /// [`Truth::True`]. `Unknown` and `False` both drop the row, which is
    /// what makes the collapse at the root sound even though it is not sound
    /// under a `Not` (`docs/design/architecture.md`, "Predicates").
    pub(crate) fn matches(
        &self,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<bool> {
        Ok(self.eval(false, raw_row, split, table, row_offset)?.is_true())
    }

    /// Kleene evaluation, left to right, stopping as soon as **the root's**
    /// value is determined.
    ///
    /// `exact` is what makes that "the root's" rather than "this node's". A
    /// caller that cannot tell `False` from `Unknown` — [`Self::matches`],
    /// and any `And`/`Or` whose own caller cannot — lets this node return
    /// `False` for an unknown and stop at the first non-`True` conjunct,
    /// which is precisely the short-circuit a bare conjunction had before
    /// expressions existed. Only [`Self::Not`] distinguishes them, so it is
    /// the one node that evaluates its child exactly, and everything beneath
    /// a `Not` is exact too.
    ///
    /// A decode failure is therefore raised only where evaluation reaches
    /// it: which rows error depends on where the term sits in the tree, and
    /// on whether a `Not` sits above it. That is the same asymmetry
    /// projection already has — deciding needs strictly less than
    /// materializing — and a row whose answer was settled by a field that
    /// did decode returns nothing wrong.
    fn eval(
        &self,
        exact: bool,
        raw_row: RawRow<'_>,
        split: &mut RowSplit,
        table: &str,
        row_offset: u64,
    ) -> Result<Truth> {
        Ok(match self {
            Self::Term(term) => term.eval(raw_row, split, table, row_offset)?,
            Self::And(children) => {
                let mut unknown = false;
                for child in children {
                    match child.eval(exact, raw_row, split, table, row_offset)? {
                        Truth::True => {}
                        Truth::False => return Ok(Truth::False),
                        Truth::Unknown if exact => unknown = true,
                        Truth::Unknown => return Ok(Truth::False),
                    }
                }
                if unknown { Truth::Unknown } else { Truth::True }
            }
            Self::Or(children) => {
                let mut unknown = false;
                for child in children {
                    match child.eval(exact, raw_row, split, table, row_offset)? {
                        Truth::True => return Ok(Truth::True),
                        Truth::Unknown if exact => unknown = true,
                        Truth::False | Truth::Unknown => {}
                    }
                }
                if unknown { Truth::Unknown } else { Truth::False }
            }
            Self::Not(inner) => inner.eval(true, raw_row, split, table, row_offset)?.not(),
        })
    }

    /// The divergence notes for every term in this tree, in the order the
    /// caller wrote them — what `TableStream::comparison_notes` hands back.
    /// Derived from the resolved tree rather than stored beside it, so the
    /// two cannot disagree, and per *term* rather than per column because
    /// which divergences reach an operator depends on the operator.
    pub(crate) fn comparison_notes(&self) -> Vec<ComparisonNote> {
        let mut out = Vec::new();
        self.collect_notes(&mut out);
        out
    }

    /// Whether evaluating this tree reads a field at all. An empty
    /// conjunction — the `Expr::And(vec![])` a query with no filter carries —
    /// reads nothing, and a stream whose projection is also empty therefore
    /// decodes nothing and skips the bulk UTF-8 validation
    /// (`docs/design/architecture.md`, "A row's bytes are validated once, in
    /// bulk").
    pub(crate) fn reads_fields(&self) -> bool {
        match self {
            Self::Term(_) => true,
            Self::And(children) | Self::Or(children) => children.iter().any(Self::reads_fields),
            Self::Not(inner) => inner.reads_fields(),
        }
    }

    fn collect_notes(&self, out: &mut Vec<ComparisonNote>) {
        match self {
            Self::Term(term) => out.extend(term.comparison_notes()),
            Self::And(children) | Self::Or(children) => {
                for child in children {
                    child.collect_notes(out);
                }
            }
            Self::Not(inner) => inner.collect_notes(out),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::datatypes::{DataType, Field, IntervalUnit, Schema, TimeUnit};

    use super::*;
    use crate::pgtype::comparison_for;
    use crate::preamble::{CollationDef, ColumnDef, TypeDef, TypeKind};
    use crate::resolve::ColumnNote;

    /// A term resolved against a column the register has no plan for — what
    /// every `Eq`/`Ne` on an unresolved column falls back to, and what the
    /// two NULL tests always are.
    fn text_term(p: &Predicate, index: usize) -> ResolvedTerm {
        let compared = p.value.as_ref().map(|value| ComparedTerm {
            column: p.column.clone(),
            comparison: Comparison::Canonical(value.clone()),
            declared_type: String::new(),
            divergences: Vec::new(),
        });
        ResolvedTerm { op: p.op, index, compared: if p.op.is_ordering() { None } else { compared } }
    }

    /// The one note a term announces, or `None` — the shape every test here
    /// but the nested ones wants, since a scalar column has at most one
    /// diverging position.
    #[track_caller]
    fn only_note(term: &ResolvedTerm) -> Option<ComparisonNote> {
        let notes = term.comparison_notes();
        assert!(notes.len() <= 1, "a scalar column announces at most one note: {notes:?}");
        notes.into_iter().next()
    }

    /// One text term's own truth value over `raw_row`.
    fn truth(p: &Predicate, raw_row: &[u8], index: usize) -> Truth {
        text_term(p, index)
            .eval(RawRow::unchecked(raw_row), &mut RowSplit::default(), "public.t", 0)
            .unwrap()
    }

    /// Whether a row survives a conjunction of text terms — the shape every
    /// filter had before expressions existed.
    fn matches_all(filters: &[Predicate], indices: &[usize], raw_row: &[u8]) -> bool {
        let expr = ResolvedExpr::And(
            filters
                .iter()
                .zip(indices)
                .map(|(p, &i)| ResolvedExpr::Term(text_term(p, i)))
                .collect(),
        );
        expr.matches(RawRow::unchecked(raw_row), &mut RowSplit::default(), "public.t", 0).unwrap()
    }

    /// The type list every one-column schema below resolves against: one
    /// enum, so a column declared `public.mood` reaches the register's enum
    /// arm rather than its "no such type" one.
    fn test_types() -> Vec<TypeDef> {
        vec![
            TypeDef {
                name: "public.mood".into(),
                kind: TypeKind::Enum { labels: vec!["sad".into(), "ok".into()] },
            },
            // A range whose DDL stated no `subtype` — the one shape the
            // resolver still calls nested and the register refuses outright,
            // since there is no bound type to name the refusal after.
            TypeDef {
                name: "public.opaquerange".into(),
                kind: TypeKind::Range {
                    subtype: None,
                    multirange_type_name: None,
                    canonical: None,
                },
            },
        ]
    }

    /// A one-column `ResolvedSchema` for `declared`/`data_type`, mapped and
    /// scalar — the shape an ordering term is allowed on.
    ///
    /// The comparison plan comes from the register itself rather than being
    /// stated here, so these tests exercise the same `declared -> plan` walk
    /// `resolve_columns` makes.
    fn one_column(declared: &str, data_type: DataType) -> ResolvedSchema {
        ResolvedSchema {
            schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
            columns: vec![ColumnResolution::Mapped],
            notes: vec![ColumnNote {
                column: "v".into(),
                declared: Some(declared.into()),
                resolution: ColumnResolution::Mapped,
            }],
            plans: vec![NestedPlan::Scalar],
            comparisons: vec![comparison_for(declared, None, &test_types(), &[])],
        }
    }

    /// A one-column schema for a **nested** declared type, resolved against
    /// `types` so the Arrow type, the nested plan and the comparison plan
    /// agree the way they do in a real query — which is what decides whether
    /// a term is refused, compared structurally, or compared as text.
    fn nested_column(declared: &str, types: &[TypeDef]) -> ResolvedSchema {
        let crate::pgtype::TypeOutcome::Mapped(data_type, plan) =
            crate::pgtype::resolve_declared_type(declared, types)
        else {
            panic!("{declared} does not resolve")
        };
        ResolvedSchema {
            schema: Arc::new(Schema::new(vec![Field::new("v", data_type, true)])),
            columns: vec![ColumnResolution::Mapped],
            notes: vec![ColumnNote {
                column: "v".into(),
                declared: Some(declared.into()),
                resolution: ColumnResolution::Mapped,
            }],
            plans: vec![plan],
            comparisons: vec![comparison_for(declared, None, types, &[])],
        }
    }

    /// One nested term's verdict over a single-field row holding `field`.
    #[track_caller]
    fn nested_verdict(
        declared: &str,
        types: &[TypeDef],
        op: PredicateOp,
        field: &str,
        literal: &str,
    ) -> Result<Truth> {
        let p = Predicate { column: "v".into(), op, value: Some(literal.into()) };
        resolve_term(&p, 0, &nested_column(declared, types), 0)?.eval(
            RawRow::unchecked(field.as_bytes()),
            &mut RowSplit::default(),
            "public.t",
            0,
        )
    }

    fn order_predicate(op: PredicateOp, value: &str) -> Predicate {
        Predicate { column: "v".into(), op, value: Some(value.into()) }
    }

    /// Resolve `op value` against a one-column schema and evaluate it over a
    /// single-field row holding `field`.
    fn ordered(
        declared: &str,
        data_type: DataType,
        op: PredicateOp,
        value: &str,
        field: &str,
    ) -> Result<bool> {
        let p = order_predicate(op, value);
        let term = resolve_term(&p, 0, &one_column(declared, data_type), 0)?;
        Ok(term
            .eval(RawRow::unchecked(field.as_bytes()), &mut RowSplit::default(), "public.t", 0)?
            .is_true())
    }

    #[test]
    fn eq_matches_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a\tb".into()) };
        assert_eq!(truth(&p, b"other\ta\\tb", 1), Truth::True);
        assert_eq!(truth(&p, b"other\tc", 1), Truth::False);
    }

    #[test]
    fn ne_matches_everything_but_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert_eq!(truth(&p, b"other\tb", 1), Truth::True);
        assert_eq!(truth(&p, b"other\ta", 1), Truth::False);
    }

    /// A NULL field is *unknown* under `=` and `!=`, not false — and a row
    /// is kept only where the root is true, so the observable answer is the
    /// one it always was.
    #[test]
    fn null_is_unknown_under_eq_and_ne() {
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
        let ne = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert_eq!(truth(&eq, b"other\t\\N", 1), Truth::Unknown);
        assert_eq!(truth(&ne, b"other\t\\N", 1), Truth::Unknown);
        assert!(!matches_all(&[eq, ne], &[1, 1], b"other\t\\N"));
    }

    /// `IS DISTINCT FROM` is `!=` with NULL counted as a value, which is the
    /// one thing `NOT` cannot express: `NOT (x = a)` drops a NULL row.
    #[test]
    fn is_distinct_from_counts_null_as_a_value() {
        let idf = Predicate {
            column: "x".into(),
            op: PredicateOp::IsDistinctFrom,
            value: Some("a".into()),
        };
        let indf = Predicate {
            column: "x".into(),
            op: PredicateOp::IsNotDistinctFrom,
            value: Some("a".into()),
        };
        assert_eq!(truth(&idf, b"other\t\\N", 1), Truth::True);
        assert_eq!(truth(&idf, b"other\tb", 1), Truth::True);
        assert_eq!(truth(&idf, b"other\ta", 1), Truth::False);
        assert_eq!(truth(&indf, b"other\t\\N", 1), Truth::False);
        assert_eq!(truth(&indf, b"other\tb", 1), Truth::False);
        assert_eq!(truth(&indf, b"other\ta", 1), Truth::True);

        // The half `NOT` gets wrong, stated as the pair it is.
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
        let negated = ResolvedExpr::Not(Box::new(ResolvedExpr::Term(text_term(&eq, 1))));
        assert!(
            !negated
                .matches(RawRow::unchecked(b"other\t\\N"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        assert!(
            ResolvedExpr::Term(text_term(&idf, 1))
                .matches(RawRow::unchecked(b"other\t\\N"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
    }

    #[test]
    fn is_null_and_is_not_null() {
        let is_null = Predicate { column: "x".into(), op: PredicateOp::IsNull, value: None };
        let is_not_null = Predicate { column: "x".into(), op: PredicateOp::IsNotNull, value: None };
        assert_eq!(truth(&is_null, b"other\t\\N", 1), Truth::True);
        assert_eq!(truth(&is_null, b"other\ta", 1), Truth::False);
        assert_eq!(truth(&is_not_null, b"other\t\\N", 1), Truth::False);
        assert_eq!(truth(&is_not_null, b"other\ta", 1), Truth::True);
    }

    #[test]
    fn an_empty_conjunction_matches_every_row() {
        assert!(matches_all(&[], &[], b"a\tb"));
        // And the default filter *is* that conjunction, which is what makes
        // "no filter" need no case of its own on the row path.
        assert!(matches!(Expr::default(), Expr::And(ref children) if children.is_empty()));
    }

    #[test]
    fn every_term_must_match() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Eq, value: Some("1".into()) },
            Predicate { column: "b".into(), op: PredicateOp::Eq, value: Some("2".into()) },
        ];
        assert!(matches_all(&filters, &[0, 1], b"1\t2"));
        assert!(!matches_all(&filters, &[0, 1], b"1\t3"));
        assert!(!matches_all(&filters, &[0, 1], b"9\t2"));
    }

    /// Two terms on one column are an ordinary conjunction, and a
    /// contradictory pair simply matches nothing — no term is special-cased.
    #[test]
    fn two_terms_may_name_the_same_column() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("1".into()) },
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("2".into()) },
        ];
        assert!(matches_all(&filters, &[0, 0], b"3"));
        assert!(!matches_all(&filters, &[0, 0], b"2"));
    }

    /// The row every tree test below is evaluated over: field 0 holds `1`
    /// and field 1 is NULL, which is how a leaf of each truth value is
    /// built out of real terms rather than out of a constant.
    const THREE_VALUED_ROW: &[u8] = b"1\t\\N";

    /// A leaf that evaluates to `want` over [`THREE_VALUED_ROW`]: `f0 = 1`,
    /// `f0 = 2`, and `f1 = 1` against the NULL field.
    fn leaf(want: Truth) -> ResolvedExpr {
        let (index, value) = match want {
            Truth::True => (0, "1"),
            Truth::False => (0, "2"),
            Truth::Unknown => (1, "1"),
        };
        let p = Predicate { column: "v".into(), op: PredicateOp::Eq, value: Some(value.into()) };
        ResolvedExpr::Term(text_term(&p, index))
    }

    fn exact(expr: &ResolvedExpr) -> Truth {
        expr.eval(
            true,
            RawRow::unchecked(THREE_VALUED_ROW),
            &mut RowSplit::default(),
            "public.t",
            0,
        )
        .unwrap()
    }

    const VALUES: [Truth; 3] = [Truth::True, Truth::False, Truth::Unknown];

    /// Kleene's tables, written out rather than derived, over trees whose
    /// leaves are real terms.
    #[test]
    fn and_or_and_not_are_three_valued() {
        use Truth::{False, True, Unknown};

        assert_eq!(exact(&ResolvedExpr::And(vec![])), True, "an empty conjunction is true");
        assert_eq!(exact(&ResolvedExpr::Or(vec![])), False, "an empty disjunction is false");

        for a in VALUES {
            assert_eq!(exact(&ResolvedExpr::Not(Box::new(leaf(a)))), a.not());
            for b in VALUES {
                let pair = || vec![leaf(a), leaf(b)];
                let and = match (a, b) {
                    (False, _) | (_, False) => False,
                    (Unknown, _) | (_, Unknown) => Unknown,
                    _ => True,
                };
                let or = match (a, b) {
                    (True, _) | (_, True) => True,
                    (Unknown, _) | (_, Unknown) => Unknown,
                    _ => False,
                };
                assert_eq!(exact(&ResolvedExpr::And(pair())), and, "{a:?} AND {b:?}");
                assert_eq!(exact(&ResolvedExpr::Or(pair())), or, "{a:?} OR {b:?}");
            }
        }
    }

    /// Every tree of height at most three over the three truth values, with
    /// `And`/`Or` arity two.
    fn trees(height: usize) -> Vec<ResolvedExpr> {
        let mut out: Vec<ResolvedExpr> = VALUES.iter().copied().map(leaf).collect();
        if height == 0 {
            return out;
        }
        let sub = trees(height - 1);
        for a in &sub {
            out.push(ResolvedExpr::Not(Box::new(a.clone())));
            for b in &sub {
                out.push(ResolvedExpr::And(vec![a.clone(), b.clone()]));
                out.push(ResolvedExpr::Or(vec![a.clone(), b.clone()]));
            }
        }
        out
    }

    /// **The short-circuit is verdict-preserving.** `matches` evaluates the
    /// root inexactly — an `And` may report `False` for an `Unknown` and
    /// stop, which is the short-circuit a bare conjunction always had — and
    /// that is sound only because nothing above the root tells the two
    /// apart. Asserted over every tree of height three rather than argued.
    #[test]
    fn the_root_verdict_survives_the_short_circuit() {
        let all = trees(2);
        assert!(all.len() > 1000, "only {} trees", all.len());
        for expr in &all {
            assert_eq!(
                expr.matches(
                    RawRow::unchecked(THREE_VALUED_ROW),
                    &mut RowSplit::default(),
                    "public.t",
                    0
                )
                .unwrap(),
                exact(expr).is_true(),
                "{expr:?}"
            );
        }
    }

    /// A two-column schema, both `integer`, for the tests about *where* a
    /// decode failure surfaces.
    fn two_integers() -> ResolvedSchema {
        let note = |name: &str| ColumnNote {
            column: name.into(),
            declared: Some("integer".into()),
            resolution: ColumnResolution::Mapped,
        };
        ResolvedSchema {
            schema: Arc::new(Schema::new(vec![
                Field::new("a", DataType::Int32, true),
                Field::new("b", DataType::Int32, true),
            ])),
            columns: vec![ColumnResolution::Mapped; 2],
            notes: vec![note("a"), note("b")],
            plans: vec![NestedPlan::Scalar; 2],
            comparisons: vec![comparison_for("integer", None, &[], &[]); 2],
        }
    }

    /// **A decode failure surfaces only where evaluation reaches it**, so
    /// which rows error depends on where the term sits in the tree — and on
    /// whether a `Not` sits above it, since a `Not` is the one node that has
    /// to tell `False` from `Unknown` and therefore cannot short-circuit an
    /// unknown away.
    #[test]
    fn a_decode_failure_surfaces_only_where_it_is_reached() {
        let schema = two_integers();
        let term = |column: &str, op, value: &str, index| {
            let p = Predicate { column: column.into(), op, value: Some(value.into()) };
            ResolvedExpr::Term(resolve_term(&p, index, &schema, 0).unwrap())
        };
        // `b` holds text no `integer` decoder will read, so any term over it
        // is `Error::FieldDecode` the moment it is evaluated.
        let row: &[u8] = b"1\tnope";
        let corrupt = || term("b", PredicateOp::Gt, "0", 1);
        let ok = |value| term("a", PredicateOp::Eq, value, 0);

        assert!(
            corrupt()
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .is_err()
        );
        // Settled by a field that did decode, in both directions.
        assert!(
            ResolvedExpr::Or(vec![ok("1"), corrupt()])
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        assert!(
            !ResolvedExpr::And(vec![ok("2"), corrupt()])
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        // Not settled: the walk reaches the corrupt field.
        assert!(
            ResolvedExpr::And(vec![ok("1"), corrupt()])
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .is_err()
        );

        // An unknown conjunct settles the *root*, so the walk stops there —
        // until a `Not` above it makes `Unknown` and `False` different
        // answers and the conjunction has to finish. Field 2 does not exist
        // in this row, so the leading term reads NULL and is unknown.
        let unknown = || {
            let p = Predicate { column: "c".into(), op: PredicateOp::Eq, value: Some("x".into()) };
            ResolvedExpr::And(vec![ResolvedExpr::Term(text_term(&p, 2)), corrupt()])
        };
        assert!(
            !unknown()
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
        );
        assert!(
            ResolvedExpr::Not(Box::new(unknown()))
                .matches(RawRow::unchecked(row), &mut RowSplit::default(), "public.t", 0)
                .is_err()
        );
    }

    #[test]
    fn missing_column_index_is_treated_as_null() {
        // Can't happen once a caller resolves the index from the block's own
        // schema, but the fallback is still exercised here.
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert_eq!(truth(&p, b"onlyone", 5), Truth::Unknown);
    }

    /// The four operators over the boundary itself — the case a `<` / `<=`
    /// pair differs on, and the one an off-by-one would pass.
    #[test]
    fn the_four_operators_differ_only_at_the_boundary() {
        for (op, below, at, above) in [
            (PredicateOp::Lt, true, false, false),
            (PredicateOp::Le, true, true, false),
            (PredicateOp::Gt, false, false, true),
            (PredicateOp::Ge, false, true, true),
        ] {
            for (field, expected) in [("4", below), ("5", at), ("6", above)] {
                assert_eq!(
                    ordered("integer", DataType::Int32, op, "5", field).unwrap(),
                    expected,
                    "{} {} 5",
                    field,
                    op.symbol()
                );
            }
        }
    }

    /// Numbers order as numbers, which is the whole point: the text
    /// comparison `Eq` uses would put `9` after `10`.
    #[test]
    fn integers_order_numerically_not_lexicographically() {
        assert!(ordered("integer", DataType::Int32, PredicateOp::Gt, "9", "10").unwrap());
        assert!(!ordered("integer", DataType::Int32, PredicateOp::Lt, "9", "10").unwrap());
    }

    /// An `oid` is unsigned across its whole range: `4294967295` is above
    /// `2147483648`, which an `Int32` reading of the same bytes would make
    /// two negative numbers.
    #[test]
    fn an_oid_orders_over_the_whole_unsigned_range() {
        let oid = |op, value, field| ordered("oid", DataType::UInt32, op, value, field).unwrap();
        assert!(oid(PredicateOp::Gt, "2147483648", "4294967295"));
        assert!(oid(PredicateOp::Lt, "2147483648", "0"));
        assert!(oid(PredicateOp::Ge, "4294967295", "4294967295"));
    }

    /// `oidin` reads a signed literal by wrapping it — `-1` is 4294967295 —
    /// and this build does not implement that. The literal is refused rather
    /// than read as −1, which no OID could equal: a weaker answer, never a
    /// wrong one.
    #[test]
    fn a_signed_oid_literal_is_refused_rather_than_wrapped() {
        let p = order_predicate(PredicateOp::Lt, "-1");
        let err = resolve_term(&p, 0, &one_column("oid", DataType::UInt32), 0).unwrap_err();
        assert!(
            matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v"),
            "{err:?}"
        );
    }

    /// A `numeric(10,2)` literal and field are both taken to the column's
    /// scale before comparing, so `1.5` and `1.50` are one value.
    #[test]
    fn a_decimal_compares_at_the_columns_scale() {
        let t = DataType::Decimal128(10, 2);
        assert!(ordered("numeric(10,2)", t.clone(), PredicateOp::Ge, "1.5", "1.50").unwrap());
        assert!(ordered("numeric(10,2)", t.clone(), PredicateOp::Le, "1.5", "1.50").unwrap());
        assert!(ordered("numeric(10,2)", t, PredicateOp::Lt, "1.5", "-1.50").unwrap());
    }

    /// A literal finer than the column's scale is refused rather than
    /// rounded: it is decoded with the column's own decoder, and that decoder
    /// does not drop non-zero digits.
    #[test]
    fn a_literal_finer_than_the_columns_scale_is_refused() {
        let err =
            ordered("numeric(10,2)", DataType::Decimal128(10, 2), PredicateOp::Gt, "1.005", "1.00")
                .unwrap_err();
        assert!(
            matches!(err, Error::PredicateValueDecode { ref value, .. } if value == "1.005"),
            "{err:?}"
        );
    }

    /// PostgreSQL's NaN, not Rust's: greater than everything, equal to
    /// itself, and never `None`.
    #[test]
    fn nan_is_the_largest_float_and_equals_itself() {
        let f = |op, value, field| {
            ordered("double precision", DataType::Float64, op, value, field).unwrap()
        };
        assert!(f(PredicateOp::Gt, "Infinity", "NaN"));
        assert!(f(PredicateOp::Ge, "NaN", "NaN"));
        assert!(f(PredicateOp::Le, "NaN", "NaN"));
        assert!(!f(PredicateOp::Gt, "NaN", "NaN"));
        assert!(f(PredicateOp::Lt, "NaN", "-Infinity"));
    }

    /// A NULL field is excluded by every ordering operator, the same collapse
    /// `Eq`/`Ne` make.
    #[test]
    fn a_null_field_matches_no_ordering_operator() {
        for op in [PredicateOp::Lt, PredicateOp::Le, PredicateOp::Gt, PredicateOp::Ge] {
            assert!(!ordered("integer", DataType::Int32, op, "0", "\\N").unwrap());
        }
    }

    /// The three special values are ordered exactly, on whichever side they
    /// appear: `-infinity` below every finite value, `infinity` above it, and
    /// each equal to itself (I34). What cannot hold them is `Date32`, not the
    /// file.
    #[test]
    fn date_and_timestamp_infinities_are_ordered() {
        let date = |op, value, field| ordered("date", DataType::Date32, op, value, field).unwrap();
        assert!(date(PredicateOp::Gt, "2020-01-01", "infinity"));
        assert!(!date(PredicateOp::Lt, "2020-01-01", "infinity"));
        assert!(date(PredicateOp::Lt, "0044-01-01 BC", "-infinity"));
        assert!(date(PredicateOp::Lt, "infinity", "-infinity"));
        // Each special equals itself, so the boundary pair splits on it.
        assert!(date(PredicateOp::Ge, "infinity", "infinity"));
        assert!(!date(PredicateOp::Gt, "infinity", "infinity"));
        assert!(date(PredicateOp::Le, "-infinity", "-infinity"));

        let ts = |op, value, field| {
            ordered(
                "timestamp without time zone",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                op,
                value,
                field,
            )
            .unwrap()
        };
        assert!(ts(PredicateOp::Gt, "2020-01-01 00:00:00", "infinity"));
        assert!(ts(PredicateOp::Lt, "2020-01-01 00:00:00", "-infinity"));
        let tstz = ordered(
            "timestamp with time zone",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            PredicateOp::Gt,
            "2020-01-01 00:00:00+00",
            "infinity",
        );
        assert!(tstz.unwrap());
    }

    /// A `numeric(p,s)` column can hold `NaN` — the typmod does not apply to
    /// it — and PostgreSQL orders it above every other value, itself included
    /// (I34). An *infinity* cannot reach this kind at all: any typmod rejects
    /// one, and a `numeric` without a typmod is held as text.
    #[test]
    fn a_decimal_nan_is_the_largest_value_and_equals_itself() {
        let t = DataType::Decimal128(10, 2);
        let nan = |op, value, field| ordered("numeric(10,2)", t.clone(), op, value, field).unwrap();
        assert!(nan(PredicateOp::Gt, "999999.99", "NaN"));
        assert!(nan(PredicateOp::Gt, "-1.50", "NaN"));
        assert!(nan(PredicateOp::Ge, "NaN", "NaN"));
        assert!(!nan(PredicateOp::Gt, "NaN", "NaN"));
        assert!(nan(PredicateOp::Lt, "NaN", "0.00"));
    }

    /// The spelling is the one that type's own `*_out` writes and nothing
    /// else, so a `date` reading `Infinity` is still undecodable — on either
    /// side. The same strictness the nested codec applies, and the reason a
    /// text column holding the word is unaffected.
    #[test]
    fn only_the_types_own_spelling_is_special() {
        let err = ordered("date", DataType::Date32, PredicateOp::Gt, "Infinity", "2020-01-01")
            .unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
        let err = ordered("date", DataType::Date32, PredicateOp::Gt, "2020-01-01", "Infinity")
            .unwrap_err();
        assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");

        // A `text` column holding `infinity` holds the *word*: it compares
        // bytewise, below `zzz`, where a special value would sort above it.
        let text = |op, field| ordered("text", DataType::Utf8View, op, "zzz", field).unwrap();
        assert!(text(PredicateOp::Lt, "infinity"));
        assert!(!text(PredicateOp::Gt, "infinity"));
    }

    /// A field that is not a value of its mapped type is the same fault the
    /// typed build path reports, with the same wording and the same escape.
    /// This is the population the special values were separated *from*:
    /// nothing can be concluded about `twelve` in an `integer`.
    #[test]
    fn an_undecodable_field_is_a_field_decode_error() {
        let err = ordered("integer", DataType::Int32, PredicateOp::Gt, "0", "twelve").unwrap_err();
        assert!(
            matches!(err, Error::FieldDecode { ref column, ref value, .. }
                if column == "v" && value == "twelve"),
            "{err:?}"
        );
    }

    /// A literal that is not a value of the column's type is refused when the
    /// block's schema resolves — before a row is read, and once rather than
    /// per row.
    #[test]
    fn an_undecodable_literal_is_refused_at_resolution() {
        let p = order_predicate(PredicateOp::Gt, "twelve");
        let err = resolve_term(&p, 0, &one_column("integer", DataType::Int32), 0).unwrap_err();
        assert!(
            matches!(err, Error::PredicateValueDecode { ref column, .. } if column == "v"),
            "{err:?}"
        );
    }

    /// Refusal is by resolution and plan, and the three reasons are distinct
    /// facts about the column rather than one catch-all.
    #[test]
    fn an_ordering_operator_is_refused_off_a_mapped_scalar_column() {
        let p = order_predicate(PredicateOp::Gt, "1");

        let mut unmapped = one_column("mystery", DataType::Utf8View);
        unmapped.columns[0] = ColumnResolution::UnknownType;
        assert!(matches!(
            resolve_term(&p, 0, &unmapped, 7).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, header_offset: 7, .. } if reason == NOT_MAPPED
        ));

        // A nested column the register compares not at all — a range whose
        // DDL stated no subtype, so there is no bound type to compare by and
        // none to name in a refusal either.
        let mut nested = one_column("public.opaquerange", DataType::Utf8View);
        nested.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
        assert!(matches!(
            resolve_term(&p, 0, &nested, 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NESTED
        ));

        // A nested column whose *shape* compares here and one of whose
        // positions does not: refused too, and the sentence names the
        // position and the type rather than the nesting.
        let mut inherited = one_column("json[]", DataType::Utf8View);
        inherited.plans[0] = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        let err = resolve_term(&p, 0, &inherited, 0).unwrap_err();
        let Error::UnorderedPredicateColumn { reason, .. } = &err else { panic!("{err:?}") };
        assert!(reason.contains("`[]` inside it is `json`"), "{reason}");

        // A `Mapped` scalar column whose *declared* type the register
        // refuses: unreachable from the mapping table today — everything it
        // maps to a scalar has a comparison in the same arm — and refused
        // anyway.
        assert!(matches!(
            resolve_term(&p, 0, &one_column("mystery", DataType::UInt8), 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NO_ORDER
        ));
    }

    /// A range type declaring a `canonical` function refuses **every**
    /// comparing operator, `=` and `!=` included, and does it through an
    /// error of its own — the one refusal that cannot end by offering the
    /// text comparison, because that comparison is exactly what the file says
    /// is not the server's.
    ///
    /// The two NULL tests still answer: they read no value and consult no
    /// plan.
    #[test]
    fn a_range_declaring_a_canonical_function_refuses_every_operator() {
        let types = vec![TypeDef {
            name: "public.canonrange".into(),
            kind: TypeKind::Range {
                subtype: Some("integer".into()),
                multirange_type_name: None,
                canonical: Some("public.canonrange_canonical".into()),
            },
        }];
        let schema = nested_column("public.canonrange", &types);
        for op in [
            PredicateOp::Lt,
            PredicateOp::Le,
            PredicateOp::Gt,
            PredicateOp::Ge,
            PredicateOp::Eq,
            PredicateOp::Ne,
            PredicateOp::IsDistinctFrom,
            PredicateOp::IsNotDistinctFrom,
        ] {
            let p = Predicate { column: "v".into(), op, value: Some("[1,10]".into()) };
            let err = resolve_term(&p, 0, &schema, 7).unwrap_err();
            let Error::UncomparablePredicateColumn { reason, header_offset: 7, .. } = &err else {
                panic!("{op:?}: {err:?}")
            };
            // The sentence names the range type and its function, which is
            // what a user needs to go and look at the DDL.
            assert!(reason.contains("`public.canonrange`"), "{op:?}: {reason}");
            assert!(reason.contains("`public.canonrange_canonical`"), "{op:?}: {reason}");
        }
        for op in [PredicateOp::IsNull, PredicateOp::IsNotNull] {
            let p = Predicate { column: "v".into(), op, value: None };
            let term = resolve_term(&p, 0, &schema, 0).expect("a NULL test consults no plan");
            assert!(term.compared.is_none(), "{op:?}");
        }
    }

    /// Only a divergent classification produces a note, and the sentence is
    /// chosen from the declared type rather than the Arrow one.
    #[test]
    fn a_divergent_comparison_produces_a_note_naming_why() {
        let note = |declared, data_type, literal| {
            let p = order_predicate(PredicateOp::Gt, literal);
            only_note(&resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap())
        };

        assert_eq!(note("integer", DataType::Int32, "1"), None, "an agreeing type says nothing");

        let text = note("character varying(10)", DataType::Utf8View, "a").unwrap();
        assert_eq!(text.divergence, ComparisonDivergence::UnknownCollation);
        assert!(text.message().contains("collation"), "{}", text.message());

        // `character(n)` asks the same collation question, over its own
        // trimming comparison — so a bare column gets the collation
        // sentence and one declaring `COLLATE "C"` gets no note at all.
        let padded = note("character(10)", DataType::Utf8View, "a").unwrap();
        assert_eq!(padded.divergence, ComparisonDivergence::UnknownCollation);
        assert!(padded.message().contains("collation"), "{}", padded.message());

        // `json` is what `AsText` has left: the server defines no comparison
        // for it at all, so bytewise offers more than the server does.
        let other = note("json", DataType::Utf8View, "1").unwrap();
        assert_eq!(other.divergence, ComparisonDivergence::AsText);
        assert!(other.message().contains("no comparison"), "{}", other.message());

        // Types that share `Utf8View` with the rows above and say nothing:
        // a bare `numeric`, an enum, and the three the text-held row lost.
        assert_eq!(note("numeric", DataType::Utf8View, "10"), None);
        for (declared, literal) in [
            ("time with time zone", "00:00:00+00"),
            ("inet", "10.0.0.1"),
            ("macaddr", "08:00:2b:01:02:03"),
        ] {
            assert_eq!(note(declared, DataType::Utf8View, literal), None, "{declared}");
        }
        assert_eq!(
            note(
                "public.mood",
                DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
                "sad",
            ),
            None
        );
    }

    /// `jsonb` orders by kind before value, and the kind order is
    /// `JsonbValue`'s own type codes: an object above an array, an array
    /// above every scalar, and a boolean above a number above a string above
    /// JSON `null` (I41). The first three pairs are cells of
    /// `fixtures/16/oracle/comparisons.tsv`; the rest are the probe I41
    /// records, since no oracle case carries a boolean or a string leaf.
    ///
    /// `holds` reads `field <op> literal`, the direction a filter asks in.
    #[test]
    fn jsonb_orders_by_kind_before_value() {
        let holds =
            |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("1", PredicateOp::Gt, "null"));
        assert!(holds("[1, 2]", PredicateOp::Gt, "1"));
        assert!(holds(r#"{"a": 1}"#, PredicateOp::Gt, "[1, 2]"));
        assert!(holds("true", PredicateOp::Gt, "1"));
        assert!(holds("1", PredicateOp::Gt, r#""a""#));
        assert!(holds("false", PredicateOp::Lt, "true"));
        // A bytewise comparison of the same text gets that third one
        // backwards: `{` is below `n`.
        assert!(holds(r#"{"a": 1}"#, PredicateOp::Gt, "null"));
    }

    /// A container is ordered by its **size** before any member of it, and
    /// only then member-wise — which is what makes a one-pair object sort
    /// below a two-pair one whatever the keys say.
    #[test]
    fn a_jsonb_container_is_ordered_by_size_first() {
        let holds =
            |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("[1, 2]", PredicateOp::Gt, "[3]"));
        assert!(holds(r#"{"z": 1}"#, PredicateOp::Lt, r#"{"a": 1, "b": 2}"#));
        assert!(holds("[]", PredicateOp::Lt, "[[]]"));
        // Same size: the members decide, keys before values.
        assert!(holds(r#"{"z": 1}"#, PredicateOp::Gt, r#"{"a": 2}"#));
        assert!(holds(r#"{"a": 2}"#, PredicateOp::Gt, r#"{"a": 1}"#));
        // A container element outranks a scalar one at the same position.
        assert!(holds("[[1], 2]", PredicateOp::Gt, "[3, [4]]"));
    }

    /// The pairs of an object are walked in **storage** order — key length
    /// first, then bytes — while the keys themselves are compared as strings.
    /// The two orders disagree here: stored, this is `z` against `y`, and
    /// sorted alphabetically it would be `aa` against `y`, which answers the
    /// other way (I41).
    #[test]
    fn jsonb_object_pairs_are_walked_in_storage_order() {
        assert!(
            ordered(
                "jsonb",
                DataType::Utf8View,
                PredicateOp::Gt,
                r#"{"y": 3, "zz": 4}"#,
                r#"{"z": 1, "aa": 2}"#,
            )
            .unwrap()
        );
    }

    /// A top-level scalar is stored in a one-element pseudo-array, and
    /// `compareJsonbContainers` lets the element count *overwrite* the
    /// `rawScalar` answer — so a scalar sorts below a one- or many-element
    /// array and **above an empty one** (I41). It is the one place a "scalars
    /// sort below arrays" rule is wrong, and nothing but modelling the
    /// wrapper produces it.
    #[test]
    fn a_top_level_scalar_outranks_an_empty_array() {
        let holds =
            |field, op, literal| ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("1", PredicateOp::Gt, "[]"));
        assert!(holds("null", PredicateOp::Gt, "[]"));
        assert!(holds("1", PredicateOp::Lt, "[1]"));
        assert!(holds("1", PredicateOp::Lt, "[1, 2]"));
        // An object is above a scalar whatever its size, because the two
        // opening tokens differ and the type order decides before any count.
        assert!(holds("1", PredicateOp::Lt, "{}"));
        assert!(holds("[]", PredicateOp::Lt, "{}"));
    }

    /// A `jsonb` number is a `numeric`, so `1`, `1.0` and `1e0` are one value
    /// and `9` is below `10` — none of which a bytewise comparison of the
    /// text gives. Whitespace, key order and a duplicate key are normalized
    /// on the way in, the last of them to the **last** value written, which
    /// is what the server stores.
    #[test]
    fn a_jsonb_literal_is_canonicalized_the_way_the_server_stores_it() {
        fn holds(field: &str, op: PredicateOp, literal: &str) -> bool {
            ordered("jsonb", DataType::Utf8View, op, literal, field).unwrap()
        }
        fn equal(field: &str, literal: &str) {
            assert!(holds(field, PredicateOp::Ge, literal), "{field} >= {literal}");
            assert!(holds(field, PredicateOp::Le, literal), "{field} <= {literal}");
        }
        for spelling in ["1", "1.0", "1e0", "1.00", "0.1e1", "100e-2"] {
            equal("1", spelling);
        }
        assert!(holds("10", PredicateOp::Gt, "9"));
        equal("-0", "0");
        equal(r#"{"a": 1}"#, r#"{  "a" : 1  }"#);
        equal(r#"{"a": 1}"#, r#"{"a":1}"#);
        equal(r#"{"a": 2}"#, r#"{"a":1,"a":2}"#);
        equal(r#"{"a": 1, "b": 2}"#, r#"{"b":2,"a":1}"#);
        // `escape_json` writes a tab as `\t`, and the `COPY` row then doubles
        // that backslash — so the field below is four characters of escaping
        // deep and the literal, which is COPY-decoded already, is two. Both
        // spellings a JSON escape has for the character are one value;
        // *unescaped* it is not a JSON string at all, which the refusal test
        // below pins.
        equal("\"a\\\\tb\"", "\"a\\u0009b\"");
        // A character above the BMP, which `escape_json` writes as itself and
        // a literal may write as a surrogate pair.
        equal("\"\u{1f600}\"", "\"\\ud83d\\ude00\"");
    }

    /// The literal grammar is `jsonb_in`'s and nothing wider: every one of
    /// these is text the server itself refuses (I41), so it is
    /// `Error::PredicateValueDecode` naming the value rather than a
    /// comparison that means something else.
    #[test]
    fn a_jsonb_literal_outside_the_input_grammar_is_refused() {
        for literal in [
            "01",
            "+1",
            ".5",
            "1.",
            "NaN",
            "1 2",
            "[1,]",
            "{a:1}",
            "{",
            "{\"a\":1",
            "'a'",
            "",
            "\"\\x41\"",
            "\"\\ud83d\"",
            "\"\\u0000\"",
            "\"a\nb\"",
            "1e999999",
        ] {
            let err =
                ordered("jsonb", DataType::Utf8View, PredicateOp::Gt, literal, "1").unwrap_err();
            assert!(matches!(err, Error::PredicateValueDecode { .. }), "{literal:?}: {err:?}");
        }
        // A field the parser refuses is the other fault, worded as the build
        // path words it.
        let err = ordered("jsonb", DataType::Utf8View, PredicateOp::Gt, "1", "{oops}").unwrap_err();
        assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");
    }

    /// A `jsonb` column announces one thing and it is not the text-held
    /// sentence: the structure is compared exactly and only a string leaf is
    /// left, on the database's collation (I32, I41). `json` keeps the
    /// text-held sentence, because the server defines no order for it at all.
    #[test]
    fn jsonb_announces_its_string_leaves_and_json_stays_text_held() {
        let note = |declared, literal| {
            let p = order_predicate(PredicateOp::Gt, literal);
            only_note(&resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap())
        };
        let jsonb = note("jsonb", "1").unwrap();
        assert_eq!(jsonb.divergence, ComparisonDivergence::JsonbStringCollation);
        assert!(jsonb.message().contains("structurally"), "{}", jsonb.message());
        assert!(jsonb.message().contains("object key"), "{}", jsonb.message());

        let json = note("json", "anything").unwrap();
        assert_eq!(json.divergence, ComparisonDivergence::AsText);
        assert!(json.message().contains("no comparison"), "{}", json.message());
    }

    /// A bare `numeric` orders as a decimal, which is the row's whole
    /// content: `9` is below `10` where the text it is held as puts it above,
    /// and trailing zeros are not part of the value (I33).
    #[test]
    fn a_bare_numeric_orders_by_value_not_by_its_text() {
        let n =
            |op, value, field| ordered("numeric", DataType::Utf8View, op, value, field).unwrap();
        assert!(n(PredicateOp::Gt, "9", "10"));
        assert!(!n(PredicateOp::Lt, "9", "10"));
        // Equal by value, written two ways — a bare `numeric` preserves the
        // scale it was written with.
        assert!(n(PredicateOp::Ge, "1.5", "1.50"));
        assert!(n(PredicateOp::Le, "1.5", "1.50"));
        assert!(!n(PredicateOp::Gt, "1.5", "1.50"));
        // Sign, then magnitude, then the fraction.
        assert!(n(PredicateOp::Lt, "0", "-0.001"));
        assert!(n(PredicateOp::Gt, "-2", "-1.9"));
        assert!(n(PredicateOp::Gt, "0.45", "0.5"));
        assert!(n(PredicateOp::Lt, "0.55", "0.5"));
        // One zero, however either side spells it.
        assert!(n(PredicateOp::Ge, "-0", "0.000"));
        assert!(n(PredicateOp::Le, "-0", "0.000"));
        // Past every fixed-width integer, which is why the key is digits.
        let big = "1".repeat(200);
        assert!(n(PredicateOp::Gt, &big, &format!("{big}0")));
    }

    /// All three of `numeric`'s specials are ordered, and only a *bare*
    /// column admits the two infinities: any typmod rejects them (I34), so
    /// on a `numeric(77,0)` the literal is refused rather than compared.
    #[test]
    fn a_bare_numeric_carries_all_three_specials() {
        let n =
            |op, value, field| ordered("numeric", DataType::Utf8View, op, value, field).unwrap();
        assert!(n(PredicateOp::Gt, "1.5", "Infinity"));
        assert!(n(PredicateOp::Lt, "1.5", "-Infinity"));
        assert!(n(PredicateOp::Gt, "Infinity", "NaN"));
        assert!(n(PredicateOp::Ge, "NaN", "NaN"));
        assert!(!n(PredicateOp::Gt, "NaN", "NaN"));
        assert!(n(PredicateOp::Gt, "-Infinity", "Infinity"));
        // `numeric_out`'s spelling and nothing else, the same strictness
        // `date`'s lower-case `infinity` gets.
        let err = ordered("numeric", DataType::Utf8View, PredicateOp::Gt, "inf", "0").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");

        // Past `Decimal256`: a `NaN` is reachable, an infinity is not.
        let wide =
            |op, value, field| ordered("numeric(77,0)", DataType::Utf8View, op, value, field);
        assert!(wide(PredicateOp::Gt, "0", "NaN").unwrap());
        let err = wide(PredicateOp::Gt, "Infinity", "0").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
    }

    /// An enum orders by declaration order, which is what the server does
    /// (I33) and is the reverse of the label text here: `sad` is declared
    /// first and sorts last alphabetically.
    #[test]
    fn an_enum_orders_by_declaration_order() {
        let dict = || DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8));
        let e = |op, value, field| ordered("public.mood", dict(), op, value, field).unwrap();
        assert!(e(PredicateOp::Gt, "sad", "ok"));
        assert!(!e(PredicateOp::Lt, "sad", "ok"));
        assert!(e(PredicateOp::Ge, "sad", "sad"));
        assert!(!e(PredicateOp::Gt, "sad", "sad"));

        // A label the type does not declare is not a value of the column, on
        // either side — the same two faults every other type raises.
        let err = ordered("public.mood", dict(), PredicateOp::Gt, "nope", "sad").unwrap_err();
        assert!(matches!(err, Error::PredicateValueDecode { .. }), "{err:?}");
        let err = ordered("public.mood", dict(), PredicateOp::Gt, "sad", "SAD").unwrap_err();
        assert!(matches!(err, Error::FieldDecode { .. }), "{err:?}");
    }

    /// An `interval`'s months collapse to 30 days and its days to 86400
    /// seconds, so `1 mon`, `30 days` and `720:00:00` are one value written
    /// three ways — every answer here read out of
    /// `fixtures/17/oracle/comparisons.tsv`, where the server itself says so.
    /// A bytewise comparison gets all three wrong.
    ///
    /// `holds` reads `field <op> literal`, which is the direction a filter
    /// asks in: the row's own value on the left.
    #[test]
    fn an_interval_orders_by_its_collapsed_span() {
        let holds = |field, op, literal| {
            ordered("interval", DataType::Interval(IntervalUnit::MonthDayNano), op, literal, field)
                .unwrap()
        };
        for spelling in ["30 days", "720:00:00"] {
            assert!(holds(spelling, PredicateOp::Ge, "1 mon"), "{spelling}");
            assert!(holds(spelling, PredicateOp::Le, "1 mon"), "{spelling}");
        }
        // `-1 days` is below `00:00:00`, which bytewise it is not.
        assert!(holds("-1 days", PredicateOp::Lt, "00:00:00"));
        // The full `interval_out` form, with a year part and a time tail:
        // fourteen months and change, so above `1 mon` and below `2 years`.
        let full = "1 year 2 mons 3 days 04:05:06";
        assert!(holds(full, PredicateOp::Lt, "2 years"));
        assert!(holds(full, PredicateOp::Gt, "1 mon"));
        // The sign on a time tail belongs to the whole tail, and a `+` on a
        // part that follows a negative one is `AddPostgresIntPart`'s.
        assert!(holds("-1 days -04:00:00", PredicateOp::Lt, "-1 days"));
        assert!(holds("-1 days +04:00:00", PredicateOp::Gt, "-1 days"));
        // The two infinities are v17 values read on every file, and they are
        // `date_out`'s spellings rather than `numeric_out`'s.
        assert!(holds("infinity", PredicateOp::Gt, "1 mon"));
        assert!(holds("-infinity", PredicateOp::Lt, "1 mon"));
        assert!(holds("infinity", PredicateOp::Ge, "infinity"));
        assert!(matches!(
            ordered(
                "interval",
                DataType::Interval(IntervalUnit::MonthDayNano),
                PredicateOp::Gt,
                "Infinity",
                "1 mon"
            )
            .unwrap_err(),
            Error::PredicateValueDecode { .. }
        ));
    }

    /// The `interval` grammar is `interval_out`'s under `IntervalStyle =
    /// postgres` (I4) and nothing wider, so a spelling the server's *input*
    /// function takes is refused rather than guessed at. The first three are
    /// literals `fixtures/17/oracle/literals.tsv` records the server
    /// accepting.
    #[test]
    fn an_interval_literal_outside_the_output_grammar_is_refused() {
        for literal in ["1.5 hours", "P1Y2M", "1 century", "1 month", "@ 1 day", "", "1 day "] {
            let err = ordered(
                "interval",
                DataType::Interval(IntervalUnit::MonthDayNano),
                PredicateOp::Gt,
                literal,
                "1 mon",
            )
            .unwrap_err();
            assert!(matches!(err, Error::PredicateValueDecode { .. }), "{literal:?}: {err:?}");
        }
    }

    /// A `time with time zone` sorts by the UTC instant and breaks a tie on
    /// the zone, so `00:00:00-05` is above `00:00:00+00` — five hours later,
    /// though it reads earlier — and two spellings of one instant are still
    /// unequal (`fixtures/16/oracle/comparisons.tsv`).
    #[test]
    fn a_timetz_orders_by_the_utc_instant_then_the_zone() {
        let holds = |field, op, literal| {
            ordered("time with time zone", DataType::Utf8View, op, literal, field).unwrap()
        };
        assert!(holds("00:00:00-05", PredicateOp::Gt, "00:00:00+00"));
        // Same instant, different zone: ordered, and not equal. PostgreSQL
        // sorts by the stored zone, which is seconds *west* of GMT, so the
        // value displaying `-05` ranks above the one displaying `+00`.
        assert!(holds("00:00:00-05", PredicateOp::Gt, "05:00:00+00"));
        assert!(!holds("00:00:00-05", PredicateOp::Le, "05:00:00+00"));
        // `24:00:00` is a real boundary value, above everything finite here.
        assert!(holds("24:00:00+00", PredicateOp::Gt, "05:00:00+00"));
        // A `timetz` always carries an offset; one without is not a value.
        assert!(matches!(
            ordered("time with time zone", DataType::Utf8View, PredicateOp::Gt, "00:00:00", "\\N")
                .unwrap_err(),
            Error::PredicateValueDecode { .. }
        ));
    }

    /// `network_cmp_internal`: family first, then the shorter netmask's worth
    /// of address bits, then the netmask, then the whole address. The last
    /// pair is the one no address-then-netmask key can get right.
    #[test]
    fn inet_orders_by_family_then_prefix_then_netmask() {
        let holds =
            |field, op, literal| ordered("inet", DataType::Utf8View, op, literal, field).unwrap();
        // IPv4 below every IPv6 address, whatever the bytes say.
        assert!(holds("192.168.1.1", PredicateOp::Lt, "::1"));
        assert!(holds("192.168.1.1", PredicateOp::Gt, "10.0.0.1"));
        // Same first eight bits, different netmask: the netmask decides, and
        // it decides *before* the host bits below it are looked at.
        assert!(holds("10.1.0.0/8", PredicateOp::Lt, "10.0.0.0/16"));
        assert!(holds("10.0.0.0/16", PredicateOp::Gt, "10.1.0.0/8"));
    }

    /// `cidr` compares exactly as `inet` does; the only difference is that
    /// `cidr_in` refuses a value with a bit set below its netmask, and so
    /// does this — the refusal `oid`'s signed literal earns, on a wider
    /// grammar (`fixtures/16/oracle/literals.tsv`).
    #[test]
    fn a_cidr_refuses_a_literal_with_host_bits_set() {
        let holds =
            |field, op, literal| ordered("cidr", DataType::Utf8View, op, literal, field).unwrap();
        assert!(holds("10.0.0.0/8", PredicateOp::Lt, "192.168.1.0/24"));
        assert!(holds("192.168.1.0/24", PredicateOp::Lt, "::/0"));
        assert!(matches!(
            ordered("cidr", DataType::Utf8View, PredicateOp::Gt, "192.168.1.1/24", "10.0.0.0/8")
                .unwrap_err(),
            Error::PredicateValueDecode { .. }
        ));
        // The same value is an ordinary `inet`.
        assert!(
            ordered("inet", DataType::Utf8View, PredicateOp::Lt, "192.168.1.1/24", "10.0.0.0/8")
                .unwrap()
        );
    }

    /// The two MAC types compare as their bytes and differ only in width, so
    /// a six-octet literal is not a `macaddr8` value and the reverse holds
    /// too. The colon form is the only one read, which is what
    /// `macaddr_out` writes.
    #[test]
    fn a_macaddr_compares_as_its_octets_at_its_own_width() {
        let holds = |declared, field, op, literal| {
            ordered(declared, DataType::Utf8View, op, literal, field).unwrap()
        };
        assert!(holds("macaddr", "08:00:2b:01:02:03", PredicateOp::Lt, "08:00:2b:01:02:04"));
        assert!(holds(
            "macaddr8",
            "08:00:2b:01:02:03:04:05",
            PredicateOp::Lt,
            "08:00:2b:01:02:03:04:06"
        ));
        for (declared, literal) in [
            ("macaddr", "08:00:2b:01:02:03:04:05"),
            ("macaddr8", "08:00:2b:01:02:03"),
            // A separator the server takes and `macaddr_out` never writes.
            ("macaddr", "08-00-2b-01-02-03"),
            // `from_str_radix` would take the sign; the digit check does not.
            ("macaddr", "+8:00:2b:01:02:03"),
        ] {
            let err =
                ordered(declared, DataType::Utf8View, PredicateOp::Gt, literal, "\\N").unwrap_err();
            assert!(
                matches!(err, Error::PredicateValueDecode { .. }),
                "{declared} {literal}: {err:?}"
            );
        }
    }

    /// `uuid` and `bytea` compare as their bytes, which is what
    /// `uuid_internal_cmp` and `byteacmp` do.
    #[test]
    fn uuid_and_bytea_compare_bytewise() {
        assert!(
            ordered(
                "uuid",
                DataType::FixedSizeBinary(16),
                PredicateOp::Gt,
                "00000000-0000-0000-0000-000000000000",
                "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
            )
            .unwrap()
        );
        // `\x00ff` is COPY-escaped in the row, and sorts after `\x00`:
        // `memcmp` ties, then the longer value wins.
        assert!(ordered("bytea", DataType::Binary, PredicateOp::Gt, "\\x00", "\\\\x00ff").unwrap());
    }

    /// Dates and timestamps compare as the instant they decode to, so a
    /// BC date is below every AD one however its text sorts.
    #[test]
    fn dates_compare_as_instants() {
        assert!(
            ordered("date", DataType::Date32, PredicateOp::Lt, "2024-01-01", "0044-01-01 BC")
                .unwrap()
        );
        assert!(
            ordered(
                "timestamp with time zone",
                DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
                PredicateOp::Lt,
                "2024-01-01 00:00:00+00",
                "2023-12-31 18:30:00+00",
            )
            .unwrap()
        );
    }

    /// `=` renders the filter's literal into the spelling the file holds and
    /// compares bytes, so a literal that is the same *value* written
    /// differently now matches — which is what the untyped comparison could
    /// not do.
    #[test]
    fn equality_canonicalizes_the_literal_once() {
        let eq = |declared, data_type, literal, field| {
            ordered(declared, data_type, PredicateOp::Eq, literal, field).unwrap()
        };
        // The scale the column declares, which the file always writes out in
        // full and a person never does.
        assert!(eq("numeric(10,2)", DataType::Decimal128(10, 2), "1.5", "1.50"));
        assert!(!eq("numeric(10,2)", DataType::Decimal128(10, 2), "1.5", "1.51"));
        // A leading zero, a leading plus, a case difference, an input-only
        // separator: every one of them is the same value the file spells one
        // way.
        assert!(eq("integer", DataType::Int32, "+007", "7"));
        assert!(eq("oid", DataType::UInt32, "0042", "42"));
        assert!(eq(
            "uuid",
            DataType::FixedSizeBinary(16),
            "0AF1C2D3-0000-0000-0000-000000000000",
            "0af1c2d3-0000-0000-0000-000000000000"
        ));
        assert!(eq("macaddr", DataType::Utf8View, "08:00:2B:01:02:03", "08:00:2b:01:02:03"));
        // The two `numeric` kinds part company on `NaN`, and both spell it
        // the way their own `*_out` does, so it renders to itself.
        assert!(eq("numeric(10,2)", DataType::Decimal128(10, 2), "NaN", "NaN"));
        assert!(eq("date", DataType::Date32, "infinity", "infinity"));
        assert!(!eq("date", DataType::Date32, "infinity", "-infinity"));
    }

    /// `=` on a `text` or `varchar` column is the byte comparison it has
    /// always been: `CompareKind::Text` renders the literal to itself, so
    /// nothing on the per-row path moved for the commonest column there is.
    #[test]
    fn equality_on_a_text_column_is_unchanged() {
        for declared in ["text", "character varying(10)", "name", "json"] {
            assert!(ordered(declared, DataType::Utf8View, PredicateOp::Eq, "a b", "a b").unwrap());
            assert!(
                !ordered(declared, DataType::Utf8View, PredicateOp::Eq, "a b", "a  b").unwrap()
            );
        }
    }

    /// `character(n)` is the third category — the *field* narrowed per row.
    /// The dump writes every value padded to `n` and `bpchareq` strips the
    /// padding from both sides (I38), so an unpadded literal matches.
    #[test]
    fn equality_on_a_char_column_trims_both_sides() {
        let eq = |literal, field| {
            ordered("character(10)", DataType::Utf8View, PredicateOp::Eq, literal, field).unwrap()
        };
        assert!(eq("a", "a         "));
        assert!(eq("a         ", "a"));
        // A tab is a value byte, not padding — the byte that separates
        // trim-and-compare from pad-and-compare. The *field* side is COPY
        // TEXT, so the tab is written escaped; the literal side is a
        // `Predicate`'s own value and is not.
        assert!(!eq("a", "a\\t        "));
        assert!(eq("a\t", "a\\t        "));
    }

    /// The five kinds whose `*_out` is not injective over the values one file
    /// can hold: no rendering of the literal makes them bytewise, so both
    /// sides are decoded per row.
    #[test]
    fn the_decoded_kinds_call_two_spellings_of_one_value_equal() {
        let eq = |declared, literal, field| {
            ordered(declared, DataType::Utf8View, PredicateOp::Eq, literal, field).unwrap()
        };
        // A bare `numeric` keeps its display scale (I33).
        assert!(eq("numeric", "1.5", "1.50"));
        assert!(eq("numeric", "1.50", "1.5"));
        assert!(!eq("numeric", "1.5", "1.51"));
        // An `interval` collapses months to 30 days and days to 86400 s (I40).
        assert!(eq("interval", "1 mon", "30 days"));
        assert!(eq("interval", "30 days", "720:00:00"));
        // `float8eq` has one zero, and a dump can write `-0`.
        assert!(
            ordered("double precision", DataType::Float64, PredicateOp::Eq, "0", "-0").unwrap()
        );
        // ... and `NaN = NaN` is true on the server, which is neither IEEE's
        // answer nor Rust's (I33).
        assert!(
            ordered("double precision", DataType::Float64, PredicateOp::Eq, "NaN", "NaN").unwrap()
        );
        // `jsonb` prints its numbers through `numeric_out` and compares them
        // by value (I41), so one document is written two ways.
        assert!(eq("jsonb", "{\"a\": 1.50}", "{\"a\": 1.5}"));
        assert!(!eq("jsonb", "{\"a\": 1.5}", "{\"a\": 2}"));
        // The two that decode for a reason about this build rather than about
        // PostgreSQL: reproducing their output functions is the cost being
        // refused, not a non-unique spelling.
        assert!(eq("inet", "10.0.0.1/32", "10.0.0.1"));
        assert!(eq("time with time zone", "00:00:00+00", "00:00:00+00"));
        assert!(!eq("time with time zone", "00:00:00+00", "01:00:00+01"));
    }

    /// `!=` is `=` inverted, and a NULL field matches neither — the collapse
    /// `IS NULL` exists to get past.
    #[test]
    fn typed_inequality_inverts_and_a_null_matches_neither() {
        let ne = |literal, field| {
            ordered("numeric(10,2)", DataType::Decimal128(10, 2), PredicateOp::Ne, literal, field)
                .unwrap()
        };
        assert!(!ne("1.5", "1.50"));
        assert!(ne("1.5", "1.51"));
        for op in [PredicateOp::Eq, PredicateOp::Ne] {
            assert!(
                !ordered("integer", DataType::Int32, op, "1", "\\N").unwrap(),
                "a NULL field matches no comparison"
            );
        }
    }

    /// A literal that is not a value of the column's type is
    /// `Error::PredicateValueDecode` before a row is read — the same refusal
    /// an ordering operator makes, on the same output-form-only grammar. The
    /// answer it replaces is an empty result that reads like an answer.
    #[test]
    fn an_undecodable_equality_literal_is_refused_at_resolution() {
        for (declared, data_type, literal) in [
            ("integer", DataType::Int32, "abc"),
            ("boolean", DataType::Boolean, "true"),
            ("oid", DataType::UInt32, "-1"),
            ("bytea", DataType::Binary, "abc"),
            ("public.mood", DataType::Utf8View, "furious"),
            ("numeric(10,2)", DataType::Decimal128(10, 2), "1.005"),
            ("interval", DataType::Interval(IntervalUnit::MonthDayNano), "1 month"),
        ] {
            let p = order_predicate(PredicateOp::Eq, literal);
            assert!(
                matches!(
                    resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap_err(),
                    Error::PredicateValueDecode { value, .. } if value == literal
                ),
                "{declared} = {literal}"
            );
        }
    }

    /// The refusal names the form the column's `CompareKind` reads, not only
    /// the value it turned down — so a `boolean` is told how a `boolean` is
    /// written rather than told that `true` is not one, and `interval`,
    /// `inet` and `macaddr` are answered by the same sentence.
    #[test]
    fn a_refused_literal_names_the_form_the_column_accepts() {
        for (declared, data_type, literal, accepted) in [
            ("boolean", DataType::Boolean, "true", "`t` or `f`"),
            (
                "interval",
                DataType::Interval(IntervalUnit::MonthDayNano),
                "1 month",
                "the way `interval` prints it",
            ),
            ("inet", DataType::Utf8View, "10", "a full IPv4 or IPv6 address"),
            ("macaddr", DataType::Utf8View, "08-00-2b-01-02-03", "six colon-separated hex pairs"),
            // The two arms where the kind's own payload is the answer: the
            // labels the enum actually declares, and the scale that makes the
            // `numeric` clause true rather than false.
            ("public.mood", DataType::Utf8View, "furious", "declared labels: 'sad', 'ok'"),
            (
                "numeric(10,2)",
                DataType::Decimal128(10, 2),
                "1.005",
                "as a number with at most 2 decimal places, or `NaN`",
            ),
        ] {
            let p = order_predicate(PredicateOp::Eq, literal);
            let message =
                resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap_err().to_string();
            assert!(message.contains(literal), "{declared} = {literal}: {message}");
            assert!(message.contains(accepted), "{declared} = {literal}: {message}");
        }
    }

    /// The enum clause names the labels themselves, quoted the way the dump
    /// writes them and a `--filter` value reads them back, and stops at
    /// [`ENUM_LABELS_SHOWN`] with a pointer at the output that carries the
    /// rest.
    #[test]
    fn the_enum_clause_quotes_its_labels_and_caps_the_list() {
        let kind = |labels: &[&str]| {
            CompareKind::Enum(labels.iter().map(|l| (*l).to_string()).collect::<Arc<[String]>>())
        };
        assert_eq!(
            accepted_form(&kind(&["sad", "has space", "has'quote"])),
            "as one of the type's declared labels: 'sad', 'has space', 'has''quote'"
        );

        let many: Vec<String> = (0..ENUM_LABELS_SHOWN + 3).map(|i| format!("l{i}")).collect();
        let clause = accepted_form(&CompareKind::Enum(many.iter().cloned().collect()));
        assert!(clause.contains("'l0'"), "{clause}");
        assert!(clause.contains(&format!("'l{}'", ENUM_LABELS_SHOWN - 1)), "{clause}");
        assert!(!clause.contains(&format!("'l{ENUM_LABELS_SHOWN}'")), "{clause}");
        assert!(clause.ends_with("and 3 more; see `info --verbose`"), "{clause}");
    }

    /// The `numeric(p,s)` clause branches on the sign of the scale, because
    /// what the column refuses does: a positive scale bounds the fraction, a
    /// zero scale admits no fraction at all, and a negative one — legal from
    /// PostgreSQL 15 — admits only multiples of a power of ten.
    #[test]
    fn the_numeric_clause_branches_on_the_sign_of_the_scale() {
        for (scale, expected) in [
            (2i8, "as a number with at most 2 decimal places, or `NaN`"),
            (1, "as a number with at most 1 decimal place, or `NaN`"),
            (0, "as a whole number, or `NaN`"),
            (-2, "as a whole number that is a multiple of 100, or `NaN`"),
        ] {
            assert_eq!(accepted_form(&CompareKind::Decimal(scale)), expected, "scale {scale}");
        }
        // The scale-free arm keeps the scale-free clause, and it is true
        // there: a `p > 76` column compares as text and refuses no literal
        // for its shape.
        assert_eq!(
            accepted_form(&CompareKind::Numeric { infinities: false }),
            "as a number, or `NaN`"
        );
    }

    /// A divergence is operator-conditional, and three of the five reach
    /// ordering alone: every libc collation is deterministic, so `texteq` is a
    /// byte comparison whatever the collation is.
    #[test]
    fn a_collation_divergence_does_not_reach_equality() {
        let note = |declared, op, literal| {
            let p = order_predicate(op, literal);
            only_note(&resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0).unwrap())
        };
        for declared in ["text", "character varying(10)", "character(10)"] {
            assert_eq!(
                note(declared, PredicateOp::Gt, "a").map(|n| n.divergence),
                Some(ComparisonDivergence::UnknownCollation),
                "{declared} under an ordering operator"
            );
            assert_eq!(note(declared, PredicateOp::Eq, "a"), None, "{declared} under `=`");
        }
        assert_eq!(
            note("jsonb", PredicateOp::Gt, "1").map(|n| n.divergence),
            Some(ComparisonDivergence::JsonbStringCollation)
        );
        assert_eq!(note("jsonb", PredicateOp::Eq, "1"), None);
        // `json` is the one that reaches both, because the server defines
        // neither operator for it.
        for op in [PredicateOp::Gt, PredicateOp::Eq] {
            assert_eq!(
                note("json", op, "1").map(|n| n.divergence),
                Some(ComparisonDivergence::AsText)
            );
        }
    }

    /// A column the register has no comparison for still answers `=`, as
    /// text — and says so where the column is a resolved scalar, because
    /// `box_eq` compares areas and a byte comparison does not.
    #[test]
    fn a_column_with_no_comparison_answers_equality_and_says_when_that_is_a_guess() {
        let p = order_predicate(PredicateOp::Eq, "(1,1),(0,0)");

        // `box` is `ColumnResolution::UnknownType` in a real query — this
        // build maps no Arrow type for it — which is the outcome the
        // announcement is keyed on.
        let mut scalar = one_column("box", DataType::Utf8View);
        scalar.columns[0] = ColumnResolution::UnknownType;
        let term = resolve_term(&p, 0, &scalar, 0).unwrap();
        let note = only_note(&term).expect("an unmodelled scalar announces");
        assert_eq!(note.divergence, ComparisonDivergence::UnmodelledType);
        assert!(note.message().contains("`box` compares areas"), "{}", note.message());
        assert!(
            term.eval(RawRow::unchecked(b"(1,1),(0,0)"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );
        assert!(
            !term
                .eval(RawRow::unchecked(b"(3,3),(2,2)"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );

        // A nested column the register does not compare is the other
        // plan-less population and is silent: its `range_out` text is a
        // faithful rendering of the value, so a byte comparison of two
        // canonical spellings is the server's answer.
        let mut nested = one_column("public.opaquerange", DataType::Utf8View);
        nested.plans[0] = NestedPlan::Range(Box::new(NestedPlan::Scalar));
        assert_eq!(only_note(&resolve_term(&p, 0, &nested, 0).unwrap()), None);

        // So is a column no DDL explained — `--data-only`, or
        // `--schema-mode strings`, which resolves nothing by design. There is
        // no declared type to qualify, and `ResolvedSchema::notes` has
        // already said so on L2.
        let mut undeclared = one_column("mystery", DataType::Utf8View);
        undeclared.columns[0] = ColumnResolution::NotDeclared;
        assert_eq!(only_note(&resolve_term(&p, 0, &undeclared, 0).unwrap()), None);
    }

    /// A nested column one of whose positions has no order here still
    /// answers `=` — over the container's whole text — and **says what that
    /// costs**, where it used to say nothing.
    ///
    /// The position is what makes it worth saying: `array_cmp` and
    /// `record_cmp` look up the position type's comparison proc and raise
    /// when there is none, so a `json` element takes the server's `=` away
    /// as surely as it takes its order, and a byte comparison of two
    /// `array_out` strings is an answer PostgreSQL does not have rather than
    /// a weaker one. That is
    /// [`ComparisonDivergence::AsText`]'s own sentence, one level down, and
    /// the note names the position exactly as the ordering refusal does.
    #[test]
    fn a_nested_uncomparable_position_announces_under_equality() {
        let types = vec![TypeDef {
            name: "public.jsonpair".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("ok", "integer"), ColumnDef::new("doc", "json")]),
            },
        }];
        for (declared, path) in
            [("json[]", "[]"), ("public.jsonpair", ".doc"), ("public.jsonpair[]", "[].doc")]
        {
            let resolved = nested_column(declared, &types);
            for op in [PredicateOp::Eq, PredicateOp::Ne] {
                let term = resolve_term(&order_predicate(op, "{}"), 0, &resolved, 0).unwrap();
                let note =
                    only_note(&term).unwrap_or_else(|| panic!("{declared} under {}", op.symbol()));
                assert_eq!(note.column, "v", "{declared}");
                assert_eq!(note.path.as_deref(), Some(path), "{declared}");
                assert_eq!(note.declared_type, "json", "{declared}");
                assert_eq!(note.divergence, ComparisonDivergence::AsText, "{declared}");
                assert!(note.message().contains("no equality"), "{}", note.message());
            }
            // The rows themselves have not moved: `=` is still the byte
            // comparison of the container's own canonical text it was.
            let term =
                resolve_term(&order_predicate(PredicateOp::Eq, "{1,2}"), 0, &resolved, 0).unwrap();
            assert!(
                term.eval(RawRow::unchecked(b"{1,2}"), &mut RowSplit::default(), "public.t", 0)
                    .unwrap()
                    .is_true(),
                "{declared}"
            );
            assert!(
                !term
                    .eval(RawRow::unchecked(b"{1,3}"), &mut RowSplit::default(), "public.t", 0)
                    .unwrap()
                    .is_true(),
                "{declared}"
            );
            // And the announcement replaces a silence under `=`, not the
            // refusal under `<`, which still names the same position.
            let Err(err) = resolve_term(&order_predicate(PredicateOp::Lt, "{}"), 0, &resolved, 0)
            else {
                panic!("{declared} is ordered");
            };
            assert!(err.to_string().contains(&format!("`{path}`")), "{err}");
        }
    }

    /// A column the **resolver** declined announces off its resolution and
    /// never off its tree — which is the boundary worth pinning, because
    /// the tree such a column carries does hold an `Uncomparable` position
    /// and the announcement above must not reach it. Both shapes resolve
    /// the column itself to text (I22, I26), so `resolve_term` drops the
    /// plan one branch earlier than the nested one.
    ///
    /// Silence is the right answer for `public.intarr[]` — PostgreSQL
    /// orders it through `array_ops` and its `array_out` text renders the
    /// value, so bytewise is the server's answer. For `box[]` it is `KD10`
    /// one level down, `box_eq` comparing areas; closing that is a question
    /// about which *resolutions* announce, not about this tree.
    #[test]
    fn a_position_the_resolver_declined_announces_nothing() {
        let types =
            vec![TypeDef { name: "public.intarr".into(), kind: TypeKind::domain("integer[]") }];
        for (declared, resolution) in [
            ("box[]", ColumnResolution::OpaqueElementType),
            ("public.intarr[]", ColumnResolution::NestedArrayElement),
        ] {
            let mut resolved = one_column(declared, DataType::Utf8View);
            resolved.comparisons = vec![comparison_for(declared, None, &types, &[])];
            assert!(
                matches!(resolved.comparisons[0], ComparisonPlan::Nested(NestedCompare::Array(_))),
                "{declared} carries a tree with an uncomparable position"
            );
            resolved.columns[0] = resolution;
            let term =
                resolve_term(&order_predicate(PredicateOp::Eq, "{}"), 0, &resolved, 0).unwrap();
            assert_eq!(only_note(&term), None, "{declared}");
        }
    }

    /// A column stating a collation the dump declares `deterministic = false`
    /// announces under **both** operator families (I42) — the only collation
    /// divergence that reaches `=`.
    ///
    /// The rows still come back bytewise, which is the defect the note is
    /// there to name: under a non-deterministic collation two values spelled
    /// differently can be equal to the server, so `=` is a weaker filter than
    /// the server's and `<` is a different order.
    #[test]
    fn a_non_deterministic_collation_announces_under_equality_and_ordering() {
        let collations = [CollationDef { name: "public.icu_ci".to_string(), deterministic: false }];
        let schema = |collation: Option<&str>, declared: &[CollationDef]| {
            let mut resolved = one_column("text", DataType::Utf8View);
            resolved.comparisons = vec![comparison_for("text", collation, &[], declared)];
            resolved
        };

        for op in [PredicateOp::Gt, PredicateOp::Eq, PredicateOp::Ne] {
            let p = order_predicate(op, "a");
            let resolved = schema(Some("public.icu_ci"), &collations);
            let note = only_note(&resolve_term(&p, 0, &resolved, 0).unwrap())
                .unwrap_or_else(|| panic!("{op:?} announces"));
            assert_eq!(note.divergence, ComparisonDivergence::NonDeterministicCollation);
            assert!(note.message().contains("non-deterministic"), "{}", note.message());
        }

        // The same clause with nothing declared about it is the ordinary
        // named-collation divergence, which `=` does not see.
        let eq = order_predicate(PredicateOp::Eq, "a");
        let plain = schema(Some("public.icu_ci"), &[]);
        assert_eq!(only_note(&resolve_term(&eq, 0, &plain, 0).unwrap()), None);

        // And the comparison itself has not moved: it is still `Text`, so the
        // term evaluates bytewise.
        let term = resolve_term(&eq, 0, &schema(Some("public.icu_ci"), &collations), 0).unwrap();
        assert!(only_note(&term).is_some());
        assert!(
            term.eval(RawRow::unchecked(b"a"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );
        assert!(
            !term
                .eval(RawRow::unchecked(b"A"), &mut RowSplit::default(), "public.t", 0)
                .unwrap()
                .is_true()
        );
    }

    /// The three tie-breaks `array_cmp` reaches only when the elements
    /// agree, in the order it reaches them: element count, then dimension
    /// count, then the dimensions themselves, then the lower bounds (I45).
    ///
    /// **The dimension pair is the one no oracle case carries.**
    /// `{{1,2,3,4}}` and `{{1,2},{3,4}}` hold the same four elements in the
    /// same order, are both two-dimensional, and differ only in `dims` —
    /// `[1,4]` against `[2,2]` — which is three steps into the tie-break.
    #[test]
    fn an_array_falls_back_to_its_shape_only_when_the_elements_agree() {
        let types = test_types();
        let lt = |field: &str, literal: &str| {
            nested_verdict("integer[]", &types, PredicateOp::Lt, field, literal).unwrap()
        };
        // Elements first: `{1,9}` is above `{2,0}` nowhere, because element 0
        // settles it before any count is looked at.
        assert_eq!(lt("{1,9}", "{2,0}"), Truth::True);
        // Element count, once the shorter array's elements agree.
        assert_eq!(lt("{1,2}", "{1,2,3}"), Truth::True);
        // Dimension count, at equal element counts.
        assert_eq!(lt("{{1},{2}}", "{1,2}"), Truth::False);
        // The dimensions, at equal element and dimension counts.
        assert_eq!(lt("{{1,2,3,4}}", "{{1,2},{3,4}}"), Truth::True);
        // The lower bounds, last of all.
        assert_eq!(lt("[0:1]={1,2}", "{1,2}"), Truth::True);
        // Two NULLs are equal and a NULL is above every value, at every
        // level and under every operator.
        assert_eq!(
            nested_verdict("integer[]", &types, PredicateOp::Eq, "{NULL}", "{NULL}").unwrap(),
            Truth::True
        );
        assert_eq!(lt("{NULL}", "{2147483647}"), Truth::False);
    }

    /// A zero-field composite is written `()`, and so is a one-field
    /// composite holding NULL — the literal cannot tell them apart and the
    /// declared field list decides (I23). The strict field decoder cannot ask
    /// that question for itself, so the comparison asks it.
    #[test]
    fn a_zero_field_composite_compares_equal_to_itself() {
        let types = [
            TypeDef {
                name: "public.empty_comp".into(),
                kind: TypeKind::Composite { fields: Some(Vec::new()) },
            },
            TypeDef {
                name: "public.one".into(),
                kind: TypeKind::Composite { fields: Some(vec![ColumnDef::new("x", "integer")]) },
            },
        ];
        assert_eq!(
            nested_verdict("public.empty_comp", &types, PredicateOp::Eq, "()", "()").unwrap(),
            Truth::True
        );
        assert_eq!(
            nested_verdict("public.one", &types, PredicateOp::Eq, "()", "()").unwrap(),
            Truth::True,
            "one field, holding NULL, on both sides"
        );
        // A field count the composite does not declare is not a value of the
        // type: `Error::FieldDecode` for a field, and the literal is refused
        // at resolution.
        assert!(matches!(
            nested_verdict("public.one", &types, PredicateOp::Eq, "(1,2)", "(1)"),
            Err(Error::FieldDecode { .. })
        ));
        assert!(matches!(
            nested_verdict("public.one", &types, PredicateOp::Eq, "(1)", "(1,2)"),
            Err(Error::PredicateValueDecode { .. })
        ));
    }

    /// `int2vector` names no operator of its own, so the server compares it
    /// through `anyarray` polymorphism — element-wise, not over the text.
    /// `'2' < '10'` is where the two answers part company, which is the
    /// property the committed oracle case is built around and the one a
    /// text fallback would get wrong.
    #[test]
    fn an_int2vector_compares_element_wise_and_not_as_text() {
        let lt = |field: &str, literal: &str| {
            nested_verdict("int2vector", &[], PredicateOp::Lt, field, literal).unwrap()
        };
        assert_eq!(lt("2", "10"), Truth::True, "element-wise; bytewise would say false");
        assert_eq!(lt("10", "2"), Truth::False);
        // `array_cmp`: the elements first, then the shorter one.
        assert_eq!(lt("1 2", "1 2 3"), Truth::True);
        assert_eq!(lt("", "0"), Truth::True);
        assert_eq!(lt("-32768 32767", "0"), Truth::True);
        assert_eq!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2 3", "1 2 3").unwrap(),
            Truth::True
        );
        // The literal takes `int2vectorin`'s superset, since the type has no
        // element input function for the leaf rule to apply to.
        assert_eq!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2", "  +1   02  ").unwrap(),
            Truth::True
        );
        // And the array spelling is not this type's, on either side.
        assert!(matches!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "1 2", "{1,2}"),
            Err(Error::PredicateValueDecode { .. })
        ));
        assert!(matches!(
            nested_verdict("int2vector", &[], PredicateOp::Eq, "{1,2}", "1 2"),
            Err(Error::FieldDecode { .. })
        ));
    }

    /// The literal side reads the container's `*_in` superset and each leaf
    /// its own `*_out` form — one rule at every depth, and it is the rule a
    /// scalar column already has.
    ///
    /// So `{ 1 , 2 }` is `{1,2}` (that is `array_in` dropping whitespace
    /// around an element) while `( 1 ,a)` is refused (that is `record_in`
    /// keeping it, and `1 ` not being what `int4out` writes).
    #[test]
    fn a_nested_literal_is_lenient_about_the_container_and_strict_about_the_leaf() {
        let types = [TypeDef {
            name: "public.point2d".into(),
            kind: TypeKind::Composite {
                fields: Some(vec![ColumnDef::new("x", "integer"), ColumnDef::new("y", "text")]),
            },
        }];
        let ints = test_types();
        for literal in ["{ 1 , 2 }", "{1,2}", "[1:2]={1,2}", "{1,\"2\"}"] {
            assert_eq!(
                nested_verdict("integer[]", &ints, PredicateOp::Eq, "{1,2}", literal).unwrap(),
                Truth::True,
                "{literal}"
            );
        }
        assert_eq!(
            nested_verdict("public.point2d", &types, PredicateOp::Eq, "(1,a)", "(1,a)").unwrap(),
            Truth::True
        );
        let err = nested_verdict("public.point2d", &types, PredicateOp::Eq, "(1,a)", "( 1 ,a)")
            .unwrap_err();
        let Error::PredicateValueDecode { accepted, .. } = &err else { panic!("{err:?}") };
        assert!(accepted.contains("spelled as the dump spells it"), "{accepted}");
    }

    /// A range's bounds settle infinity, then the held value, then
    /// inclusivity — and an exclusive bound's answer depends on *which end*
    /// it is, because it means "just after" at the lower end and "just
    /// before" at the upper (I46).
    ///
    /// `empty` below everything is the fourth rule, and it is not
    /// recoverable from the bounds: `empty` and `(,)` both have two absent
    /// bounds and sit at opposite ends of the order.
    #[test]
    fn a_range_bound_settles_infinity_then_value_then_inclusivity() {
        let types = test_types();
        let lt = |field: &str, literal: &str| {
            nested_verdict("numrange", &types, PredicateOp::Lt, field, literal).unwrap()
        };
        assert_eq!(lt("empty", "(,)"), Truth::True);
        assert_eq!(lt("(,)", "empty"), Truth::False);
        assert_eq!(
            nested_verdict("numrange", &types, PredicateOp::Eq, "empty", "empty").unwrap(),
            Truth::True
        );
        // An absent bound is the extreme of its own end.
        assert_eq!(lt("(,5)", "[1,10)"), Truth::True);
        assert_eq!(lt("[1,10)", "[1,)"), Truth::True);
        // Same value, different inclusivity, and the two ends disagree about
        // which way it goes.
        assert_eq!(lt("[1,10)", "(1,10)"), Truth::True);
        assert_eq!(lt("[1,10)", "[1,10]"), Truth::True);
    }

    /// A discrete range is rewritten into canonical form on the way in and a
    /// continuous one is not, so `[1,10]` and `[1,10)` are one value of
    /// `int4range` and two of `numrange` (I46).
    ///
    /// **Two rewrites, and the second is not a successor at all**: bounds
    /// that end up equal without both ends including the point collapse to
    /// `empty`, which is why `int4range '(1,2)'` holds nothing. It runs
    /// before the canonical function *and* after it, which is the only way
    /// `(1,2)` reaches it.
    #[test]
    fn a_discrete_range_is_canonicalized_and_a_continuous_one_is_not() {
        let types = test_types();
        let eq = |declared: &str, field: &str, literal: &str| {
            nested_verdict(declared, &types, PredicateOp::Eq, field, literal).unwrap()
        };
        for literal in ["[1,10)", "[1,9]", "(0,10)", "(0,9]"] {
            assert_eq!(eq("int4range", "[1,10)", literal), Truth::True, "{literal}");
        }
        assert_eq!(eq("int4range", "empty", "(1,2)"), Truth::True);
        assert_eq!(eq("int4range", "empty", "[1,1)"), Truth::True);
        assert_eq!(eq("numrange", "empty", "[1,1)"), Truth::True, "the collapse is not discrete");
        assert_eq!(eq("numrange", "[1,10)", "[1,10]"), Truth::False);
        // `daterange_canonical` skips a bound that is not a finite date, so
        // an infinity keeps the inclusivity it was written with (I34) — and
        // the *other* bound is still rewritten.
        assert_eq!(eq("daterange", "[2020-01-01,infinity]", "[2020-01-01,infinity]"), Truth::True);
        assert_eq!(
            eq("daterange", "[-infinity,2020-01-02)", "[-infinity,2020-01-01]"),
            Truth::True
        );
        // The successor can leave the subtype's range, which the server
        // raises on. `int8range` is where this build can see it; `int4range`
        // cannot, because a leaf literal is read as `i64` whatever the
        // column's width (see `order_key`) and the register never learns the
        // narrower one.
        assert!(matches!(
            nested_verdict(
                "int8range",
                &types,
                PredicateOp::Eq,
                "[1,10)",
                "[1,9223372036854775807]"
            ),
            Err(Error::PredicateValueDecode { .. })
        ));
        // A lower bound above its upper is `22000` on the server — a fault
        // the container grammar cannot see, since the text is well formed.
        assert!(matches!(
            nested_verdict("int4range", &types, PredicateOp::Gt, "[1,10)", "[10,1)"),
            Err(Error::PredicateValueDecode { .. })
        ));
        assert!(matches!(
            nested_verdict("int4multirange", &types, PredicateOp::Gt, "{[1,10)}", "{[10,1)}"),
            Err(Error::PredicateValueDecode { .. })
        ));
    }

    /// A multirange's members are sorted, emptied out and merged before
    /// anything is compared, so several spellings are one value and the
    /// comparison itself is a plain sequence walk (I46).
    ///
    /// **Whether two members merge is the range type's question, not the
    /// bounds'.** `{[1,5),[6,10)}` stays two members even in `int4range`,
    /// where 5 is missing between them; `{[1,5],[6,10)}` becomes one, because
    /// canonicalization has already made the first `[1,6)`. The same pair of
    /// literals in `numrange` never merges, since a continuous range has
    /// points between any two values.
    #[test]
    fn a_multirange_is_sorted_coalesced_and_emptied_before_it_is_compared() {
        let types = test_types();
        let eq = |declared: &str, field: &str, literal: &str| {
            nested_verdict(declared, &types, PredicateOp::Eq, field, literal).unwrap()
        };
        for literal in ["{[1,10)}", "{[1,5),[5,10)}", "{[5,10),[1,5)}", "{[1,1),[1,10)}"] {
            assert_eq!(eq("int4multirange", "{[1,10)}", literal), Truth::True, "{literal}");
        }
        assert_eq!(eq("int4multirange", "{[1,10)}", "{empty,[1,10)}"), Truth::True);
        assert_eq!(eq("int4multirange", "{[1,5)}", "{[1,3),[2,5)}"), Truth::True, "overlapping");
        assert_eq!(eq("int4multirange", "{[1,5),[6,10)}", "{[1,5),[6,10)}"), Truth::True);
        assert_eq!(eq("int4multirange", "{[1,10)}", "{[1,5],[6,10)}"), Truth::True, "adjacent");
        assert_eq!(eq("nummultirange", "{[1,5),[6,10)}", "{[1,5),[6,10)}"), Truth::True);
        assert_eq!(eq("nummultirange", "{[1,5),(5,10)}", "{[1,5),(5,10)}"), Truth::True);
        // Member-wise, then the shorter one first.
        let lt = |field: &str, literal: &str| {
            nested_verdict("int4multirange", &types, PredicateOp::Lt, field, literal).unwrap()
        };
        assert_eq!(lt("{}", "{[1,10)}"), Truth::True);
        assert_eq!(lt("{[1,5),[6,10)}", "{[1,10)}"), Truth::True);
    }

    /// The comparison register against the committed comparison oracle:
    /// every cell of `fixtures/<13-18>/oracle/comparisons.tsv`, answered by
    /// the same `resolve_term`/`matches` path a `--filter` takes, and
    /// compared with what the server itself said
    /// (`docs/design/architecture.md`, "The comparison oracle").
    ///
    /// **This is the check the oracle exists for.** `oracle_register.py`
    /// reconciles register arms against oracle *cases* and
    /// `oracle_differences.py` reconciles majors against each other; both are
    /// about which rows exist. Nothing else compares an answer to an answer.
    mod oracle {
        use std::collections::{BTreeMap, BTreeSet};
        use std::path::{Path, PathBuf};

        use super::*;
        use crate::cache::CacheMode;
        use crate::copy::{decode_field, encode_field, split_fields};
        use crate::index::preamble_only;
        use crate::io::LocalFileSource;
        use crate::preamble::{ColumnDef, DatabaseMetadata, DumpMetadata};
        use crate::resolve::{SchemaMode, resolve_columns};
        use crate::scan::ScanOptions;

        /// The majors `scripts/generate_fixtures.py` generates, which is what
        /// `fixtures/` holds a directory per.
        const MAJORS: [u32; 6] = [13, 14, 15, 16, 17, 18];

        /// The six operators this asserts, in `comparisons.tsv`'s own column
        /// order, paired with their cell offset after the four key columns.
        ///
        /// **`=` and `<>` are asked over a wider population than the other
        /// four**, because equality is never refused: a column the register
        /// has no comparison for still answers `=` as text, so a nested or
        /// unmapped case is skipped for the ordering operators and asserted
        /// for these two.
        ///
        /// What they add over `<=`/`>=`, which already carried the register's
        /// equality, is the **canonicalization** — the three-way choice
        /// `equality_comparison` makes. Both operands of a cell are values the
        /// server itself stored, so for a canonicalized kind the assertion is
        /// that the file's own spelling compares byte for byte; the content is
        /// in the kinds where it cannot, and every one of them has a case
        /// here: `real`'s `-0` against `0`, a bare `numeric`'s `1.5` against
        /// `1.50`, a `jsonb` number written two ways, and a `character(10)`
        /// value padded against one that is not.
        const ASSERTED: [(usize, PredicateOp); 6] = [
            (0, PredicateOp::Lt),
            (1, PredicateOp::Le),
            (2, PredicateOp::Gt),
            (3, PredicateOp::Ge),
            (4, PredicateOp::Eq),
            (5, PredicateOp::Ne),
        ];

        /// The declared types whose columns the register refuses an ordering
        /// operator on, so no cell of theirs is asserted: `xml`, an enum with
        /// no labels, a user-defined base type with no operator class, and the
        /// two nested shapes that are refused for reasons of their own.
        /// PostgreSQL orders all of them and this build does not.
        ///
        /// **`json` is not here**, and the difference is the point: it *is*
        /// compared, bytewise, and every cell of it is `E42883` because
        /// PostgreSQL defines no comparison at all — so the walk skips it as
        /// a server refusal rather than as a refusal of ours.
        ///
        /// It is asserted as an exact set, so a type that quietly stops
        /// comparing fails here rather than passing as one more skip.
        const REFUSED: [&str; 6] = [
            // I26: an array whose element is itself an array. The column
            // resolves to text, and the register agrees rather than claiming
            // an order the resolver has already declined.
            "public.intarr[]",
            // A multirange companion **before v14**, where the type does not
            // exist and the range's DDL carries no `multirange_type_name` to
            // find it through (I10). Its v14+ cells are asserted; a case is
            // recorded here if it was refused at *any* major.
            "public.myrange_multi",
            // Scalars with no comparison in the register at all.
            "public.box_domain",
            "public.mybase",
            "public.empty_enum",
            "xml",
        ];

        /// The cases where this build's answer is knowingly not
        /// PostgreSQL's, as `(type, collation, left, right)` — the exception
        /// set, and it is **met**: every entry is a real disagreement in the
        /// committed files, and every disagreement is an entry.
        ///
        /// **Met means met everywhere it is permitted.** An entry is keyed by
        /// the case, so it has to disagree in every major that carries the
        /// case and under every operator its divergence reaches — 24 cells
        /// each today, four ordering operators by six majors. Taking
        /// *somewhere in the walk* as met would instead leave the block
        /// passing after a collation moved for one major, or after the
        /// comparator stopped being antisymmetric under one operator.
        ///
        /// **`=` and `<>` are not among those cells, and that is the
        /// assertion rather than an omission.** Every divergence in this list
        /// is a divergence of *order*
        /// ([`ComparisonDivergence::affects_equality`]) — a libc collation is
        /// deterministic, so `texteq` is a byte comparison whatever the
        /// collation is — so a disagreement under `=` would fail the
        /// announcement check below rather than count toward an entry.
        ///
        /// Each entry's column also has to *announce* its divergence through
        /// [`ComparisonDivergence`], **under this operator**, which is
        /// asserted alongside — so an exception cannot be claimed for a
        /// column the register tells the user it is confident about. **Two
        /// populations, and they are one
        /// statement asked at two depths** — a collation the file does not
        /// carry (I32), reached once through a column and once through a
        /// string inside a document:
        ///
        /// - **A `jsonb` string leaf.** `compareJsonbScalarValue` passes
        ///   `DEFAULT_COLLATION_OID` to `varstr_cmp`, so a leaf is ordered by
        ///   the database's collation, which a plain dump does not record
        ///   (I32); everything structural above it — the kind order, a
        ///   container's size, storage order, the raw-scalar wrapper — is
        ///   asserted rather than excepted, which is what makes this list two
        ///   entries rather than the arm.
        /// - **`box`'s area equality**, which is the third population and the
        ///   only one this build could close by writing code. `box_eq`
        ///   compares the two rectangles' *areas*, so the server calls
        ///   `(1,1),(0,0)` and `(3,3),(2,2)` equal where a byte comparison
        ///   does not — and `box` has no comparison in the register, so the
        ///   four ordering operators are refused on it and `=` falls back to
        ///   text. `<>` is not among the cells because PostgreSQL defines no
        ///   `box <> box` at all, which is why each entry is met over six
        ///   cells rather than twenty-four. The column announces
        ///   `ComparisonDivergence::UnmodelledType`, and the deficiency is
        ///   `KD10`.
        /// - **`text` under the database's own collation**, which is glibc's
        ///   `en_US.utf8` on this apparatus: case is a lower-weight
        ///   difference than letter, an accent sorts with its base letter
        ///   rather than after `z`, and punctuation is ignored at the primary
        ///   level, so `_x` sorts where `x` does. The four unordered pairs the
        ///   case table chose for those reasons are here in both directions,
        ///   and `_x` reaches every letter in the alphabet rather than only
        ///   `ax`.
        const EXCEPTIONS: &[(&str, &str, &str, &str)] = &[
            // A `text[]` column inherits its element's collation boundary:
            // the elements are ordered by `varstr_cmp` under the database's
            // collation exactly as a `text` column's values are, one level
            // down. `b` is below `B` under `en_US.utf8` and above it
            // bytewise, and `NULL` is below `a` bytewise and above it under
            // the locale — the same two disagreements the `text` rows below
            // carry, reached through an element.
            ("text[]", "\\N", "{a,b}", "{a,B}"),
            ("text[]", "\\N", "{a,B}", "{a,b}"),
            ("text[]", "\\N", "{\"NULL\"}", "{a,b}"),
            ("text[]", "\\N", "{a,b}", "{\"NULL\"}"),
            ("text[]", "\\N", "{\"NULL\"}", "{a,B}"),
            ("text[]", "\\N", "{a,B}", "{\"NULL\"}"),
            // `box_eq` is an area comparison, not a value one.
            ("public.box_domain", "\\N", "(1,1),(0,0)", "(3,3),(2,2)"),
            ("public.box_domain", "\\N", "(3,3),(2,2)", "(1,1),(0,0)"),
            ("jsonb", "\\N", "{\"a\": \"a\"}", "{\"a\": \"A\"}"),
            ("jsonb", "\\N", "{\"a\": \"A\"}", "{\"a\": \"a\"}"),
            // Case is a lower-weight difference than letter.
            ("text", "default", "A", "a"),
            ("text", "default", "a", "A"),
            ("text", "default", "a", "B"),
            ("text", "default", "B", "a"),
            ("text", "default", "ax", "B"),
            ("text", "default", "B", "ax"),
            // An accent sorts with its base letter, not after `z`.
            ("text", "default", "é", "f"),
            ("text", "default", "f", "é"),
            ("text", "default", "é", "hello"),
            ("text", "default", "hello", "é"),
            // Punctuation is ignored at the primary level, so `_x` sorts
            // where `x` does — above every letter in the alphabet.
            ("text", "default", "_x", "a"),
            ("text", "default", "a", "_x"),
            ("text", "default", "_x", "ax"),
            ("text", "default", "ax", "_x"),
            ("text", "default", "_x", "co-op"),
            ("text", "default", "co-op", "_x"),
            ("text", "default", "_x", "coop"),
            ("text", "default", "coop", "_x"),
            ("text", "default", "_x", "de luge"),
            ("text", "default", "de luge", "_x"),
            ("text", "default", "_x", "deluge"),
            ("text", "default", "deluge", "_x"),
            ("text", "default", "_x", "e"),
            ("text", "default", "e", "_x"),
            ("text", "default", "_x", "é"),
            ("text", "default", "é", "_x"),
            ("text", "default", "_x", "f"),
            ("text", "default", "f", "_x"),
            ("text", "default", "_x", "hello"),
            ("text", "default", "hello", "_x"),
        ];

        fn fixture(major: u32, rest: &str) -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../fixtures/{major}/{rest}"))
        }

        /// Split one COPY TEXT line of a committed oracle file into its
        /// decoded fields — the same L1 decoder that reads a dump, because
        /// the server wrote these files with `COPY ... TO STDOUT`.
        fn fields(line: &[u8]) -> Vec<Option<String>> {
            split_fields(line).map(|f| decode_field(f).unwrap().map(|v| v.into_owned())).collect()
        }

        fn rows(path: &Path) -> Vec<Vec<Option<String>>> {
            let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            bytes.split(|&b| b == b'\n').filter(|l| !l.is_empty()).map(fields).collect()
        }

        /// The `COLLATE` clause a dump would carry for a case labelled with
        /// this collation. `C` is written; `default` is *not*, because
        /// `pg_dump` writes a clause only where the column's collation is not
        /// its type's default (I37) — so the dump-realistic form of the
        /// oracle's `COLLATE "default"` case is a bare column, which is the
        /// register's `UnknownCollation` arm. Both reach the same comparison.
        fn clause(collation: Option<&String>) -> Option<&'static str> {
            match collation.map(String::as_str) {
                Some("C") => Some("pg_catalog.\"C\""),
                _ => None,
            }
        }

        /// A one-column schema for a case's declared type and collation,
        /// built by handing `resolve_columns` a one-table `DumpMetadata` —
        /// the same function a real block resolves through.
        ///
        /// **Not a hand-built `ResolvedSchema`**, and the difference is
        /// load-bearing now that `=` is asserted: the resolution, the nested
        /// plan and the comparison plan have to agree the way they do in a
        /// real query, because which of them `resolve_term` consults decides
        /// whether an equality term announces a divergence. A schema claiming
        /// every case is `Mapped` and `Scalar` would put `integer[]` and
        /// `box` in one population that no dump ever produces.
        fn schema(declared: &str, collation: Option<&str>, types: &[TypeDef]) -> ResolvedSchema {
            let column = ColumnDef {
                name: "v".into(),
                declared_type: declared.into(),
                collation: collation.map(str::to_string),
            };
            let metadata = DumpMetadata {
                databases: vec![DatabaseMetadata {
                    name: None,
                    preamble_complete: true,
                    server_version: None,
                    pg_dump_version: None,
                    extensions: Vec::new(),
                    types: types.to_vec(),
                    collations: Vec::new(),
                    tables: [("public.t".to_string(), vec![column])].into_iter().collect(),
                }],
            };
            resolve_columns(
                "public.t",
                &["v".to_string()],
                Some(&metadata),
                None,
                SchemaMode::Typed,
                &[],
            )
        }

        /// The [`Truth`] this build answers a cell with, or the fault it
        /// raised instead — a literal it will not decode, or a field it will
        /// not. `field` is `None` for a SQL NULL one, which reaches the row
        /// as `\N` like any other, and answers `Unknown` exactly where the
        /// server's cell reads `u`.
        ///
        /// The second half of the pair is whether the resolved term
        /// **announces** a divergence, read off `ComparisonNote` — the channel
        /// a user actually sees — rather than off the register's plan. Which
        /// divergences reach which operator is the term's own conclusion
        /// ([`ComparisonDivergence::affects_equality`]), and asking the plan
        /// would be a second copy of that rule living in the test.
        fn answer(
            declared: &str,
            collation: Option<&str>,
            types: &[TypeDef],
            op: PredicateOp,
            field: Option<&str>,
            bound: &str,
        ) -> (std::result::Result<Truth, String>, bool) {
            let resolved = schema(declared, collation, types);
            let p = Predicate { column: "v".into(), op, value: Some(bound.into()) };
            let term = match resolve_term(&p, 0, &resolved, 0) {
                Ok(term) => term,
                Err(e) => return (Err(e.to_string()), false),
            };
            let announced = !term.comparison_notes().is_empty();
            let got = term
                .eval(
                    RawRow::unchecked(&encode_field(field)),
                    &mut RowSplit::default(),
                    "public.t",
                    0,
                )
                .map_err(|e| e.to_string());
            (got, announced)
        }

        /// Every major's `CREATE TYPE`/`CREATE DOMAIN` list, read from the
        /// `types` fixture that major's oracle database was loaded from — the
        /// same DDL, so a case's declared type resolves against the list the
        /// server itself had.
        async fn types_of(major: u32) -> Vec<TypeDef> {
            let source = LocalFileSource::open(fixture(major, "types/default.sql")).unwrap();
            let (metadata, _) =
                preamble_only(&source, &ScanOptions::default(), &CacheMode::Disabled)
                    .await
                    .unwrap();
            metadata.databases.into_iter().next().expect("a dump names a database").types
        }

        #[tokio::test]
        async fn the_register_answers_every_committed_oracle_cell() {
            // Keyed by case so one disagreement is reported as the pair it
            // is, not as four cells; the value is the majors and operators
            // it showed up under.
            let mut disagreed: BTreeMap<(String, String, String, String), BTreeSet<String>> =
                BTreeMap::new();
            // How many cells each case was asserted over at all, so "met"
            // can mean met everywhere rather than met somewhere.
            let mut walked: BTreeMap<(String, String, String, String), usize> = BTreeMap::new();
            let mut refused: BTreeSet<String> = BTreeSet::new();
            let mut asserted = 0usize;

            for major in MAJORS {
                let types = types_of(major).await;
                // The `*_out` text the server canonicalizes each accepted
                // literal to, keyed by type and literal. It is what a dump
                // *holds*, so every value below is put to the register in
                // this form rather than as the case's input spelling: a
                // `character(10)`'s blank padding, a `jsonb`'s single space
                // after each `:`, a `numeric(10,2)`'s rounded scale.
                let outputs: BTreeMap<(String, String), String> =
                    rows(&fixture(major, "oracle/literals.tsv"))
                        .into_iter()
                        .filter_map(|r| Some(((r[0].clone()?, r[1].clone()?), r[3].clone()?)))
                        .collect();

                for row in rows(&fixture(major, "oracle/comparisons.tsv")) {
                    let declared = row[0].clone().expect("a case names a type");
                    let collation = clause(row[3].as_ref());
                    let plan = comparison_for(&declared, collation, &types, &[]);
                    // A column the register has no comparison for is skipped
                    // for the four ordering operators, which it refuses, and
                    // still asserted for `=`/`<>`, which fall back to text.
                    let ordered = plan.orders();
                    if !ordered {
                        refused.insert(declared.clone());
                    }
                    // How the case names itself in a report and in
                    // `EXCEPTIONS`: the raw cells, so an entry can be copied
                    // out of a failure straight into the table.
                    let case = |left: &str, right: &str| {
                        (
                            declared.clone(),
                            row[3].clone().unwrap_or_else(|| "\\N".into()),
                            left.to_string(),
                            right.to_string(),
                        )
                    };
                    let output = |literal: &str| {
                        outputs.get(&(declared.clone(), literal.to_string())).cloned()
                    };
                    for (offset, op) in ASSERTED {
                        if op.is_ordering() && !ordered {
                            continue;
                        }
                        let cell = row[4 + offset].as_deref().expect("a cell is never NULL");
                        // A cell recording what the server *refused* says
                        // nothing about how it compares.
                        if cell.starts_with('E') {
                            continue;
                        }
                        // A NULL *right* operand has no spelling in the
                        // filter grammar at all — `IS NULL` is how a filter
                        // asks for one — so there is no question to put to
                        // the register.
                        let Some(right) = row[2].as_deref() else { continue };
                        let bound = output(right).expect("an accepted literal has an output");
                        // A NULL *left* operand is the field, and this
                        // build answers `Unknown` for every comparing
                        // operator — the same value the server's `u` records,
                        // asserted as itself rather than as the "excluded"
                        // it collapses to at the root.
                        let Some(left) = row[1].as_deref() else {
                            assert_eq!(cell, "u", "{major} {declared}: a NULL operand");
                            assert_eq!(
                                answer(&declared, collation, &types, op, None, &bound).0,
                                Ok(Truth::Unknown),
                                "{major} {declared}: a NULL field is unknown"
                            );
                            continue;
                        };
                        let field = output(left).expect("an accepted literal has an output");
                        asserted += 1;
                        // `announced` is whether a disagreement is *permitted*
                        // here — the term announces a divergence that reaches
                        // this operator. It is what an exception is checked
                        // against and what the cell count is taken over, so an
                        // entry is never asked to be met under an operator its
                        // divergence does not claim.
                        let (got, announced) =
                            answer(&declared, collation, &types, op, Some(&field), &bound);
                        if announced {
                            *walked.entry(case(left, right)).or_default() += 1;
                        }
                        let expected = match cell {
                            "t" => Truth::True,
                            "f" => Truth::False,
                            other => panic!("{major} {declared}: unexpected cell {other:?}"),
                        };
                        if got != Ok(expected) {
                            // An exception is only ever claimable where the
                            // register has already told the user its answer
                            // may differ *under this operator*.
                            assert!(
                                announced,
                                "{major} {declared} {}: disagrees under {} while announcing no \
                                 divergence that reaches it",
                                row[1].as_deref().unwrap_or("\\N"),
                                op.symbol()
                            );
                            disagreed
                                .entry(case(left, right))
                                .or_default()
                                .insert(format!("{major} {} {got:?}", op.symbol()));
                        }
                    }
                }
            }

            let expected_exceptions: BTreeSet<_> = EXCEPTIONS
                .iter()
                .map(|(t, c, l, r)| (t.to_string(), c.to_string(), l.to_string(), r.to_string()))
                .collect();
            let found: BTreeSet<_> = disagreed.keys().cloned().collect();
            let unexpected: Vec<_> = found.difference(&expected_exceptions).collect();
            let stale: Vec<_> = expected_exceptions.difference(&found).collect();
            assert!(
                unexpected.is_empty(),
                "{} cases disagree with the server and are not in the exception set:\n{}",
                unexpected.len(),
                unexpected
                    .iter()
                    .map(|k| format!("  {k:?} {:?}", disagreed[*k]))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            assert!(stale.is_empty(), "exceptions that no longer disagree: {stale:?}");
            // An entry is keyed by the case, so "met" has to mean met over
            // every cell the case has: a collation that moves for one major,
            // or a comparator that stops being antisymmetric under one
            // operator, would otherwise leave the entry satisfied by the
            // majors and operators it still disagrees under.
            let partial: Vec<_> = expected_exceptions
                .iter()
                .filter(|case| disagreed[*case].len() != walked[*case])
                .map(|case| format!("  {case:?} {:?} of {} cells", disagreed[case], walked[case]))
                .collect();
            assert!(
                partial.is_empty(),
                "{} exceptions disagree in only part of the walk:\n{}",
                partial.len(),
                partial.join("\n")
            );
            assert_eq!(
                refused,
                REFUSED.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
                "the set of declared types the register refuses an ordering operator on"
            );
            // A floor, not a count: the walk skips a cell for four good
            // reasons, and a bug in any of them would leave it asserting
            // almost nothing while passing. 52,338 today.
            assert!(asserted > 45_000, "only {asserted} cells asserted");
        }

        /// A range or multirange literal is put into the form the server
        /// stores it in *before* it is compared, checked against the server's
        /// own rewriting of every such literal in `literals.tsv`.
        ///
        /// **The cell walk above cannot reach this**, and that is why this
        /// test exists rather than one more assertion inside it: it puts both
        /// operands of every cell through `outputs` by construction, so both
        /// sides arrive already canonical and a build that rewrote nothing
        /// would pass. The input spellings are only in `literals.tsv` — and
        /// they are exactly the interesting ones, `int4range '[1,10]'` being
        /// the value the file holds as `[1,11)`.
        ///
        /// So each accepted row is asked `<output> = <input>`, which is
        /// `True` only where the two are one value here as they are on the
        /// server, and each refused row is asked for the refusal — which is
        /// where `[10,1)` lands, a `22000` the container grammar cannot see
        /// (I44) and [`make_range`] raises.
        #[tokio::test]
        async fn a_range_literal_is_canonicalized_before_it_is_compared() {
            let mut asked = 0usize;
            let mut rewritten = 0usize;
            for major in MAJORS {
                let types = types_of(major).await;
                for row in rows(&fixture(major, "oracle/literals.tsv")) {
                    let declared = row[0].clone().expect("a case names a type");
                    if !matches!(
                        comparison_for(&declared, None, &types, &[]),
                        ComparisonPlan::Nested(
                            NestedCompare::Range { .. } | NestedCompare::Multirange { .. }
                        )
                    ) {
                        continue;
                    }
                    // A SQL NULL input is the `\N` field, not a literal.
                    let Some(input) = row[1].clone() else { continue };
                    let status = row[2].as_deref().expect("a case records its status");
                    // The type does not exist at this major — a user range's
                    // multirange companion before v14 (I10).
                    if status == "E42704" {
                        continue;
                    }
                    asked += 1;
                    let ask = |field: &str| {
                        answer(&declared, None, &types, PredicateOp::Eq, Some(field), &input).0
                    };
                    let Some(output) = row[3].as_deref().filter(|_| status == "ok") else {
                        // The field is never read: the literal is refused
                        // when the block's schema resolves, before a row.
                        assert!(
                            ask("empty").is_err(),
                            "{major} {declared}: accepted {input:?}, which the server refused \
                             with {status}"
                        );
                        continue;
                    };
                    assert_eq!(
                        ask(output),
                        Ok(Truth::True),
                        "{major} {declared}: {input:?} is not the value the server stored it as \
                         ({output:?})"
                    );
                    if input != output {
                        rewritten += 1;
                    }
                }
            }
            // Floors, not counts, and the second is the one that matters: a
            // build that canonicalized nothing would still satisfy the first,
            // because most rows are already written the way the server stores
            // them. 193 and 42 today.
            assert!(asked > 150, "only {asked} range literals asserted");
            assert!(rewritten > 30, "only {rewritten} of them needed rewriting");
        }
    }
}
