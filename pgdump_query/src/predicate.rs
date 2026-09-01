//! Post-parse row filtering (`docs/design/architecture.md`, "Predicates").

use std::cmp::Ordering;

use arrow::datatypes::i256;

use crate::copy::{decode_field, split_fields};
use crate::decode;
use crate::pgtype::{CompareKind, ComparisonPlan, NestedPlan, OrderingDivergence};
use crate::resolve::{ColumnResolution, ResolvedSchema};
use crate::{Error, Result};

/// Comparison operator for [`Predicate`].
///
/// `Eq`/`Ne` compare the field as an unparsed string; the four ordering
/// operators compare **typed**, decoding both the field and the filter's own
/// literal with the column's resolved decoder and comparing the values
/// (`docs/design/architecture.md`, "Predicates"). That split is why a
/// nested column still answers `Eq`/`Ne` and refuses `<`: every value in a
/// dump is already in canonical `*_out` form, so a string comparison against
/// it is right, while an *order* over a composite or an array literal is not
/// a thing this layer can define.
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
        }
    }
}

/// A single-column post-parse filter: `column <op> value`, and one **term**
/// of a conjunction — a query carries a list of these and keeps a row only
/// if every one of them matches (`QueryOptions::filters`). `column` is
/// matched against the queried table's column names (the `COPY` header list,
/// or the `column1`, `column2`, ... placeholders used when the header has
/// none). `value` is `None` for `IsNull`/`IsNotNull`, which need no
/// comparison value; it is always `Some` for every other operator.
///
/// **How `value` is compared depends on the operator.** `Eq`/`Ne` compare it
/// against each row's decoded (unescaped) field as a plain string. The four
/// ordering operators decode it once, when the block's schema resolves, with
/// the column's own decoder, and compare decoded values — so both sides of a
/// `numeric(p,s)` comparison carry that column's scale, and a literal that
/// does not parse as the column's type is `Error::PredicateValueDecode`
/// before any row is read.
///
/// A NULL field matches nothing at all — not `Eq`, not `Ne`, and not an
/// ordering operator — because SQL's own three-valued logic collapses
/// unknown to "excluded", which is exactly why `IsNull`/`IsNotNull` exist:
/// without them there is no way to ask for a NULL explicitly
/// (`docs/status/history/2026-08-22.md`). That collapse is what bounds the
/// conjunction to `AND`: it is sound under `AND` and unsound under `NOT`,
/// which is why `OR`/`NOT` are deferred rather than added alongside
/// (`docs/design/architecture.md`, "Predicates").
#[derive(Debug, Clone)]
pub struct Predicate {
    pub column: String,
    pub op: PredicateOp,
    pub value: Option<String>,
}

/// One column of one query whose ordering comparison diverges from
/// PostgreSQL's — reported per stream by
/// `crate::stream::TableStream::ordering_notes`.
///
/// **Neither a `Diagnostic` nor a [`crate::resolve::ColumnNote`]**, and
/// deliberately: `DumpIndex.diagnostics` is the L1 file-level channel and
/// `ResolvedSchema.notes` is the L2 per-column one, while this is per-column
/// *and* conditional on a predicate — L4. Writing it into either would
/// invert the layering (`docs/design/layering.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderingNote {
    pub column: String,
    /// The declared PostgreSQL type, as the DDL spelled it.
    pub declared_type: String,
    pub divergence: OrderingDivergence,
}

impl OrderingNote {
    /// One sentence naming the column and what its order is not.
    ///
    /// Each sentence names the column and its declared type, and the
    /// declared type is what sharpens it: the collatable text types say what
    /// the *column* stated, where `AsText` says what the type means. Both
    /// exist because several declared types reach one Arrow type for
    /// different reasons.
    pub fn message(&self) -> String {
        let column = &self.column;
        let declared = &self.declared_type;
        let bytewise = |why: &str| format!("`{column}` ({declared}) is compared bytewise: {why}");
        match self.divergence {
            OrderingDivergence::AsText => {
                bytewise("PostgreSQL orders this type by its own operator, not bytewise")
            }
            OrderingDivergence::UnknownCollation => bytewise(
                "the column declares no COLLATE clause, so its collation is the database's, which \
                 a plain dump does not record — this matches the server only if that collation is \
                 C or POSIX",
            ),
            OrderingDivergence::NonBytewiseCollation => bytewise(
                "the column declares a collation other than C/POSIX, and PostgreSQL orders it by \
                 that collation",
            ),
            OrderingDivergence::BlankPadded => bytewise(
                "the dump writes every value blank-padded to the declared length, while \
                 PostgreSQL strips trailing blanks before comparing, so a value equal to the \
                 filter's own sorts after it here",
            ),
            // Not a `bytewise` sentence: the structure *is* compared the way
            // PostgreSQL compares it, and only a string leaf is left.
            OrderingDivergence::JsonbStringCollation => format!(
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
    /// `OrderingDivergence::JsonbStringCollation` is the column's announcement
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
/// the spelling unconditionally is the union rule
/// (`docs/design/roadmap-P11-typed-predicates.md`, "Version-varying
/// semantics") rather than a claim about the file's own major. What it costs
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

/// A signed count of `year`/`mon`/`day` units out of an `interval`'s text.
/// The leading `+` is `AddPostgresIntPart`'s, written on a positive part that
/// follows a negative one (I40); everything else is digits.
fn interval_count(text: &str) -> Option<i64> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let magnitude: i64 = digits.parse().ok()?;
    Some(if text.starts_with('-') { -magnitude } else { magnitude })
}

/// The `[+|-]HH:MM:SS[.ffffff]` tail of an `interval`, in microseconds.
/// `EncodeInterval` writes one sign for the whole time part — `minus` is set
/// if any of hours, minutes, seconds or the fraction is negative, and the
/// three fields are then printed as absolute values — so the sign is applied
/// to the total rather than per field (I40).
///
/// The hour field is unbounded, so this is not `decode_time64_micros`:
/// `720:00:00` is an ordinary `interval` and not a `time`. Every field is
/// checked to be digits, which is what keeps `04:-5:06` — a string no
/// `interval_out` writes and no `interval_in` accepts — from parsing as a
/// negative minute count.
fn interval_time_micros(text: &str) -> Option<i128> {
    let (negative, rest) = match text.strip_prefix(['+', '-']) {
        Some(rest) => (text.starts_with('-'), rest),
        None => (false, text),
    };
    let (hms, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut parts = hms.split(':');
    let mut field = |max_len: Option<usize>| -> Option<i128> {
        let digits = parts.next()?;
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if max_len.is_some_and(|n| digits.len() != n) {
            return None;
        }
        digits.parse::<i128>().ok()
    };
    let hours = field(None)?;
    let minutes = field(Some(2))?;
    let seconds = field(Some(2))?;
    if parts.next().is_some() {
        return None;
    }
    if frac.is_empty() && text.contains('.') {
        return None;
    }
    if frac.len() > 6 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let micros: i128 = if frac.is_empty() {
        0
    } else {
        let mut padded = frac.to_string();
        padded.push_str(&"0".repeat(6 - padded.len()));
        padded.parse().ok()?
    };
    let total = hours
        .checked_mul(3600)?
        .checked_add(minutes * 60 + seconds)?
        .checked_mul(1_000_000)?
        .checked_add(micros)?;
    Some(if negative { -total } else { total })
}

/// `interval_cmp_value`'s span, in microseconds: months collapse to 30 days,
/// days to 86400 seconds, and the time field is added on (I40). The whole
/// point of the collapse is that `1 mon`, `30 days` and `720:00:00` are one
/// value written three ways, which is why an `interval` is one of the two
/// kinds equality cannot canonicalize once and compare bytewise.
///
/// **The grammar is `interval_out`'s under `IntervalStyle = postgres`,
/// exactly**, which `pg_dump` pins on its own connection (I4): an optional
/// `<n> year[s]`, `<n> mon[s]` and `<n> day[s]`, then an optional signed time
/// part, separated by single spaces, with a wholly-zero interval written
/// `00:00:00`. Nothing broader is accepted — `1 hour`, `1.5 hours`, `P1Y2M`
/// and `1 month` are all spellings `interval_in` takes and `interval_out`
/// never writes, so a filter using one is `Error::PredicateValueDecode`
/// rather than a comparison meaning something else. That is the refusal
/// `CompareKind::UnsignedInt` makes for `oid`, on a wider grammar.
fn interval_span(text: &str) -> Option<i128> {
    let tokens: Vec<&str> = text.split(' ').collect();
    let (mut months, mut days) = (0i64, 0i64);
    let mut time = 0i128;
    let mut at = 0;
    while at < tokens.len() {
        let count = || interval_count(tokens[at]);
        match tokens.get(at + 1).copied() {
            Some("year" | "years") => months = months.checked_add(count()?.checked_mul(12)?)?,
            Some("mon" | "mons") => months = months.checked_add(count()?)?,
            Some("day" | "days") => days = days.checked_add(count()?)?,
            // Not a counted part, so this token is the time tail — which is
            // last, and of which there is at most one.
            _ => {
                if at + 1 != tokens.len() {
                    return None;
                }
                time = interval_time_micros(tokens[at])?;
                at += 1;
                break;
            }
        }
        at += 2;
    }
    if at != tokens.len() {
        return None;
    }
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

/// Everything an ordering term needs, settled once when the block's schema
/// resolves rather than per row.
#[derive(Debug, Clone)]
struct OrderTerm {
    column: String,
    kind: CompareKind,
    /// The filter's literal, decoded once with the column's own decoder.
    bound: OrderKey,
    /// The declared PostgreSQL type, for `Error::FieldDecode`'s context and
    /// for [`OrderingNote::message`].
    declared_type: String,
    divergence: Option<OrderingDivergence>,
}

/// One filter term resolved against one `COPY` block: the field index it
/// reads, plus — for an ordering operator only — the typed comparison it
/// will make.
///
/// The index is into the block's **unprojected** column list, because that is
/// what the raw row's fields are numbered by: a term may name a column the
/// projection dropped.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedTerm {
    index: usize,
    order: Option<OrderTerm>,
}

impl ResolvedTerm {
    /// This term's divergence from PostgreSQL's own comparison, if it has
    /// one. `None` for every non-ordering term and for every ordering term
    /// whose type agrees.
    pub(crate) fn ordering_note(&self) -> Option<OrderingNote> {
        let order = self.order.as_ref()?;
        Some(OrderingNote {
            column: order.column.clone(),
            declared_type: order.declared_type.clone(),
            divergence: order.divergence?,
        })
    }
}

/// The refusal an ordering operator earns on a column that cannot carry one.
/// Three reasons, each a different fact about the column.
const NOT_MAPPED: &str = "the column's declared type did not resolve to an Arrow type, so it has no order of its own \
     (`--schema-mode strings` resolves no column, by design)";
const NESTED: &str = "the column is nested (array, composite, range or multirange), and an order over such a \
     literal is not defined here";
const NO_ORDER: &str = "this build defines no ordering for the column's declared type";

/// Resolve one filter term against the block's **unprojected**
/// [`ResolvedSchema`], at `index` — the column's position, already looked up
/// by the caller.
///
/// A non-ordering term needs nothing else. An ordering term is refused here,
/// before a row of this block flows, unless the column resolved `Mapped` with
/// a [`NestedPlan::Scalar`] plan and the comparison register gave it a plan;
/// and its literal is decoded here too, so a value that is not of the
/// column's type is a fault reported once rather than a filter that matches
/// nothing.
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
    if !predicate.op.is_ordering() {
        return Ok(ResolvedTerm { index, order: None });
    }
    let refuse = |reason: &'static str| Error::UnorderedPredicateColumn {
        header_offset,
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        reason,
    };
    if resolved.columns[index] != ColumnResolution::Mapped {
        return Err(refuse(NOT_MAPPED));
    }
    if resolved.plans[index] != NestedPlan::Scalar {
        return Err(refuse(NESTED));
    }
    let ComparisonPlan::Compared { kind, divergence } = &resolved.comparisons[index] else {
        return Err(refuse(NO_ORDER));
    };
    let declared_type = resolved.notes[index].declared.clone().unwrap_or_default();
    // `value` is `Some` for every operator but the two NULL tests; an
    // embedder that builds a `Gt` term without one gets the same fault as an
    // unparseable literal, named the same way.
    let text = predicate.value.as_deref().unwrap_or_default();
    let bound = order_key(kind, text).ok_or_else(|| Error::PredicateValueDecode {
        column: predicate.column.clone(),
        op: predicate.op.symbol(),
        value: text.to_string(),
        declared_type: declared_type.clone(),
    })?;
    Ok(ResolvedTerm {
        index,
        order: Some(OrderTerm {
            column: predicate.column.clone(),
            kind: kind.clone(),
            bound,
            declared_type,
            divergence: *divergence,
        }),
    })
}

impl Predicate {
    /// Evaluate this predicate against `raw_row`'s field, per `term` — the
    /// resolution of *this* predicate against the block being replayed.
    /// `table` and `row_offset` are context for the one error this can
    /// raise: a field that does not decode as its mapped type under an
    /// ordering operator, which is `Error::FieldDecode`, worded exactly as
    /// the typed build path words it.
    pub(crate) fn matches(
        &self,
        raw_row: &[u8],
        term: &ResolvedTerm,
        table: &str,
        row_offset: u64,
    ) -> Result<bool> {
        let field = split_fields(raw_row).nth(term.index);
        let decoded = match field {
            Some(f) => decode_field(f)?,
            None => None,
        };
        Ok(match self.op {
            PredicateOp::IsNull => decoded.is_none(),
            PredicateOp::IsNotNull => decoded.is_some(),
            PredicateOp::Eq => decoded.is_some_and(|v| Some(v.as_ref()) == self.value.as_deref()),
            PredicateOp::Ne => decoded.is_some_and(|v| Some(v.as_ref()) != self.value.as_deref()),
            PredicateOp::Lt | PredicateOp::Le | PredicateOp::Gt | PredicateOp::Ge => {
                let order = term
                    .order
                    .as_ref()
                    .expect("an ordering predicate resolves to a term carrying its comparison");
                match decoded {
                    // A NULL compares to nothing: unknown collapses to false,
                    // exactly as it does under `Eq`/`Ne`.
                    None => false,
                    Some(text) => {
                        let key =
                            order_key(&order.kind, &text).ok_or_else(|| Error::FieldDecode {
                                table: table.to_string(),
                                column: order.column.clone(),
                                row_offset,
                                declared_type: order.declared_type.clone(),
                                value: text.to_string(),
                            })?;
                        let ord = compare_keys(&key, &order.bound);
                        match self.op {
                            PredicateOp::Lt => ord.is_lt(),
                            PredicateOp::Le => ord.is_le(),
                            PredicateOp::Gt => ord.is_gt(),
                            _ => ord.is_ge(),
                        }
                    }
                }
            }
        })
    }
}

/// Evaluate a conjunction against `raw_row`: every term must match.
/// `terms[i]` is term `i` resolved against the block's **unprojected**
/// schema, so the two slices are parallel by construction
/// (`docs/design/architecture.md`, "Predicates").
///
/// An empty conjunction matches every row, which is what makes "no filter"
/// need no separate case anywhere above this.
///
/// Terms are tested in the order they were given and the walk stops at the
/// first that fails, so the ordinary case costs one pass over the row. Each
/// term does walk the row itself — there is no shared pass — which is a real
/// cost only for a conjunction whose leading terms nearly always pass, and
/// which buys the short-circuit for the common shape.
pub(crate) fn matches_all(
    filters: &[Predicate],
    terms: &[ResolvedTerm],
    raw_row: &[u8],
    table: &str,
    row_offset: u64,
) -> Result<bool> {
    for (filter, term) in filters.iter().zip(terms) {
        if !filter.matches(raw_row, term, table, row_offset)? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};

    use super::*;
    use crate::pgtype::comparison_for;
    use crate::preamble::{TypeDef, TypeKind};
    use crate::resolve::ColumnNote;

    /// A term with no typed comparison behind it — what every `Eq`/`Ne`/NULL
    /// test resolves to.
    fn text_term(index: usize) -> ResolvedTerm {
        ResolvedTerm { index, order: None }
    }

    fn matches(p: &Predicate, raw_row: &[u8], index: usize) -> bool {
        p.matches(raw_row, &text_term(index), "public.t", 0).unwrap()
    }

    /// The type list every one-column schema below resolves against: one
    /// enum, so a column declared `public.mood` reaches the register's enum
    /// arm rather than its "no such type" one.
    fn test_types() -> Vec<TypeDef> {
        vec![TypeDef {
            name: "public.mood".into(),
            kind: TypeKind::Enum { labels: vec!["sad".into(), "ok".into()] },
        }]
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
            comparisons: vec![comparison_for(declared, None, &test_types())],
        }
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
        p.matches(field.as_bytes(), &term, "public.t", 0)
    }

    #[test]
    fn eq_matches_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a\tb".into()) };
        assert!(matches(&p, b"other\ta\\tb", 1));
        assert!(!matches(&p, b"other\tc", 1));
    }

    #[test]
    fn ne_matches_everything_but_the_decoded_value() {
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(matches(&p, b"other\tb", 1));
        assert!(!matches(&p, b"other\ta", 1));
    }

    #[test]
    fn null_matches_neither_eq_nor_ne() {
        let eq = Predicate { column: "x".into(), op: PredicateOp::Eq, value: Some("a".into()) };
        let ne = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(!matches(&eq, b"other\t\\N", 1));
        assert!(!matches(&ne, b"other\t\\N", 1));
    }

    #[test]
    fn is_null_and_is_not_null() {
        let is_null = Predicate { column: "x".into(), op: PredicateOp::IsNull, value: None };
        let is_not_null = Predicate { column: "x".into(), op: PredicateOp::IsNotNull, value: None };
        assert!(matches(&is_null, b"other\t\\N", 1));
        assert!(!matches(&is_null, b"other\ta", 1));
        assert!(!matches(&is_not_null, b"other\t\\N", 1));
        assert!(matches(&is_not_null, b"other\ta", 1));
    }

    #[test]
    fn an_empty_conjunction_matches_every_row() {
        assert!(matches_all(&[], &[], b"a\tb", "public.t", 0).unwrap());
    }

    #[test]
    fn every_term_must_match() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Eq, value: Some("1".into()) },
            Predicate { column: "b".into(), op: PredicateOp::Eq, value: Some("2".into()) },
        ];
        let terms = [text_term(0), text_term(1)];
        assert!(matches_all(&filters, &terms, b"1\t2", "public.t", 0).unwrap());
        assert!(!matches_all(&filters, &terms, b"1\t3", "public.t", 0).unwrap());
        assert!(!matches_all(&filters, &terms, b"9\t2", "public.t", 0).unwrap());
    }

    /// Two terms on one column are an ordinary conjunction, and a
    /// contradictory pair simply matches nothing — no term is special-cased.
    #[test]
    fn two_terms_may_name_the_same_column() {
        let filters = [
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("1".into()) },
            Predicate { column: "a".into(), op: PredicateOp::Ne, value: Some("2".into()) },
        ];
        let terms = [text_term(0), text_term(0)];
        assert!(matches_all(&filters, &terms, b"3", "public.t", 0).unwrap());
        assert!(!matches_all(&filters, &terms, b"2", "public.t", 0).unwrap());
    }

    #[test]
    fn missing_column_index_is_treated_as_null() {
        // Can't happen once a caller resolves the index from the block's own
        // schema, but the fallback is still exercised here.
        let p = Predicate { column: "x".into(), op: PredicateOp::Ne, value: Some("a".into()) };
        assert!(!matches(&p, b"onlyone", 5));
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

        let mut nested = one_column("integer[]", DataType::Utf8View);
        nested.plans[0] = NestedPlan::Array(Box::new(NestedPlan::Scalar));
        assert!(matches!(
            resolve_term(&p, 0, &nested, 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NESTED
        ));

        // A `Mapped` scalar column whose *declared* type the register
        // refuses: unreachable from the mapping table today — everything it
        // maps to a scalar has a comparison in the same arm — and refused
        // anyway.
        assert!(matches!(
            resolve_term(&p, 0, &one_column("mystery", DataType::UInt8), 0).unwrap_err(),
            Error::UnorderedPredicateColumn { reason, .. } if reason == NO_ORDER
        ));
    }

    /// Only a divergent classification produces a note, and the sentence is
    /// chosen from the declared type rather than the Arrow one.
    #[test]
    fn a_divergent_comparison_produces_a_note_naming_why() {
        let note = |declared, data_type, literal| {
            let p = order_predicate(PredicateOp::Gt, literal);
            resolve_term(&p, 0, &one_column(declared, data_type), 0).unwrap().ordering_note()
        };

        assert_eq!(note("integer", DataType::Int32, "1"), None, "an agreeing type says nothing");

        let text = note("character varying(10)", DataType::Utf8View, "a").unwrap();
        assert_eq!(text.divergence, OrderingDivergence::UnknownCollation);
        assert!(text.message().contains("collation"), "{}", text.message());

        // `character(n)` diverges for a reason that is not its collation, so
        // its sentence must not be the collation one.
        let padded = note("character(10)", DataType::Utf8View, "a").unwrap();
        assert_eq!(padded.divergence, OrderingDivergence::BlankPadded);
        assert!(padded.message().contains("blank-padded"), "{}", padded.message());

        // `json` is what `AsText` has left: the server defines no order for
        // it, so bytewise offers more than the server does.
        let other = note("json", DataType::Utf8View, "1").unwrap();
        assert_eq!(other.divergence, OrderingDivergence::AsText);
        assert!(other.message().contains("its own operator"), "{}", other.message());

        // Types that share `Utf8View` with the rows above and say nothing:
        // a bare `numeric`, an enum, and the four the text-held row lost.
        assert_eq!(note("numeric", DataType::Utf8View, "10"), None);
        for (declared, literal) in [
            ("interval", "1 day"),
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
            resolve_term(&p, 0, &one_column(declared, DataType::Utf8View), 0)
                .unwrap()
                .ordering_note()
        };
        let jsonb = note("jsonb", "1").unwrap();
        assert_eq!(jsonb.divergence, OrderingDivergence::JsonbStringCollation);
        assert!(jsonb.message().contains("structurally"), "{}", jsonb.message());
        assert!(jsonb.message().contains("object key"), "{}", jsonb.message());

        let json = note("json", "anything").unwrap();
        assert_eq!(json.divergence, OrderingDivergence::AsText);
        assert!(json.message().contains("its own operator"), "{}", json.message());
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
            ordered("interval", DataType::Utf8View, op, literal, field).unwrap()
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
            ordered("interval", DataType::Utf8View, PredicateOp::Gt, "Infinity", "1 mon")
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
            let err = ordered("interval", DataType::Utf8View, PredicateOp::Gt, literal, "1 mon")
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
        use crate::copy::encode_field;
        use crate::index::preamble_only;
        use crate::io::LocalFileSource;
        use crate::scan::ScanOptions;

        /// The majors `scripts/generate_fixtures.py` generates, which is what
        /// `fixtures/` holds a directory per.
        const MAJORS: [u32; 6] = [13, 14, 15, 16, 17, 18];

        /// The four operators this asserts, in `comparisons.tsv`'s own column
        /// order, paired with their cell offset after the four key columns.
        ///
        /// **`=` and `<>` are deliberately not here.** They are the two cells
        /// the register does not answer: `PredicateOp::Eq`/`Ne` compare the
        /// field as an unparsed string and never reach `resolve_term`'s
        /// typed path at all, so asserting them would be asserting a
        /// different mechanism whose contract is textual by design (see
        /// [`Predicate`]). Nothing about the register's *equality* is lost:
        /// `<=` and `>=` carry it, which is why `numeric(10,2)`'s `1.5`
        /// against `1.50` is checked here at all.
        const ASSERTED: [(usize, PredicateOp); 4] = [
            (0, PredicateOp::Lt),
            (1, PredicateOp::Le),
            (2, PredicateOp::Gt),
            (3, PredicateOp::Ge),
        ];

        /// The declared types whose columns the register refuses an ordering
        /// operator on, so no cell of theirs is asserted: everything nested
        /// (array, composite, range, multirange), `xml`, an enum with no
        /// labels, and a user-defined base type with no operator class.
        /// PostgreSQL orders all of them and this build does not.
        ///
        /// **`json` is not here**, and the difference is the point: it *is*
        /// compared, bytewise, and every cell of it is `E42883` because
        /// PostgreSQL defines no comparison at all — so the walk skips it as
        /// a server refusal rather than as a refusal of ours.
        ///
        /// It is asserted as an exact set, so a type that quietly stops
        /// comparing fails here rather than passing as one more skip.
        const REFUSED: [&str; 20] = [
            "integer[]",
            "text[]",
            "public.mood[]",
            "public.intarr[]",
            "public.point2d",
            "public.tagged",
            "public.empty_comp",
            "public.myrange",
            "public.myrange_multi",
            "public.textrange",
            "public.box_domain",
            "public.mybase",
            "public.empty_enum",
            "xml",
            "int4range",
            "int4multirange",
            "numrange",
            "daterange",
            "tsrange",
            "tstzrange",
        ];

        /// The cases where this build's answer is knowingly not
        /// PostgreSQL's, as `(type, collation, left, right)` — the exception
        /// set, and it is **met**: every entry is a real disagreement in the
        /// committed files, and every disagreement is an entry.
        ///
        /// **Met means met everywhere.** An entry is keyed by the case, so it
        /// has to disagree in every major that carries the case and under all
        /// four asserted operators — 24 cells each today. Taking *somewhere
        /// in the walk* as met would instead leave the block passing after a
        /// collation moved for one major, or after the comparator stopped
        /// being antisymmetric under one operator.
        ///
        /// Each entry's column also has to *announce* its divergence through
        /// [`OrderingDivergence`], which is asserted alongside — so an
        /// exception cannot be claimed for a column the register tells the
        /// user it is confident about. Three populations, and the first is
        /// the one the `jsonb` cases exist to reach:
        ///
        /// - **A `jsonb` string leaf.** `compareJsonbScalarValue` passes
        ///   `DEFAULT_COLLATION_OID` to `varstr_cmp`, so a leaf is ordered by
        ///   the database's collation, which a plain dump does not record
        ///   (I32); everything structural above it — the kind order, a
        ///   container's size, storage order, the raw-scalar wrapper — is
        ///   asserted rather than excepted, which is what makes this list two
        ///   entries rather than the arm.
        /// - **`character(10)` against a value holding a byte below `0x20`.**
        ///   The dump writes the padded form and `bpcharcmp` strips the
        ///   padding before comparing (I38), and the two orders disagree only
        ///   under the pad space — which is why the case table carries a tab.
        ///   Its collation is not what does it, and both collations are here.
        /// - **`text` under the database's own collation**, which is glibc's
        ///   `en_US.utf8` on this apparatus: case is a lower-weight
        ///   difference than letter, an accent sorts with its base letter
        ///   rather than after `z`, and punctuation is ignored at the primary
        ///   level, so `_x` sorts where `x` does. The four unordered pairs the
        ///   case table chose for those reasons are here in both directions,
        ///   and `_x` reaches every letter in the alphabet rather than only
        ///   `ax`.
        const EXCEPTIONS: &[(&str, &str, &str, &str)] = &[
            ("jsonb", "\\N", "{\"a\": \"a\"}", "{\"a\": \"A\"}"),
            ("jsonb", "\\N", "{\"a\": \"A\"}", "{\"a\": \"a\"}"),
            ("character(10)", "C", "a", "a\t"),
            ("character(10)", "C", "a\t", "a"),
            ("character(10)", "C", "a\t", "a         "),
            ("character(10)", "C", "a         ", "a\t"),
            ("character(10)", "default", "a", "a\t"),
            ("character(10)", "default", "a\t", "a"),
            ("character(10)", "default", "a\t", "a         "),
            ("character(10)", "default", "a         ", "a\t"),
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

        /// A one-column schema for a case's declared type and collation. The
        /// Arrow type is `Utf8View` throughout and never consulted:
        /// `resolve_term` reads the comparison the register gave the column,
        /// never what it was mapped to.
        fn schema(declared: &str, collation: Option<&str>, types: &[TypeDef]) -> ResolvedSchema {
            ResolvedSchema {
                schema: Arc::new(Schema::new(vec![Field::new("v", DataType::Utf8View, true)])),
                columns: vec![ColumnResolution::Mapped],
                notes: vec![ColumnNote {
                    column: "v".into(),
                    declared: Some(declared.into()),
                    resolution: ColumnResolution::Mapped,
                }],
                plans: vec![NestedPlan::Scalar],
                comparisons: vec![comparison_for(declared, collation, types)],
            }
        }

        /// `t`/`f` for a cell this build answers, or the fault it raised
        /// instead — a literal it will not decode, or a field it will not.
        /// `field` is `None` for a SQL NULL one, which reaches the row as
        /// `\N` like any other.
        fn answer(
            declared: &str,
            collation: Option<&str>,
            types: &[TypeDef],
            op: PredicateOp,
            field: Option<&str>,
            bound: &str,
        ) -> std::result::Result<bool, String> {
            let resolved = schema(declared, collation, types);
            let p = Predicate { column: "v".into(), op, value: Some(bound.into()) };
            let term = resolve_term(&p, 0, &resolved, 0).map_err(|e| e.to_string())?;
            p.matches(&encode_field(field), &term, "public.t", 0).map_err(|e| e.to_string())
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
                    if !matches!(
                        comparison_for(&declared, collation, &types),
                        ComparisonPlan::Compared { .. }
                    ) {
                        refused.insert(declared);
                        continue;
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
                        // A NULL *left* operand is the field, and this build
                        // collapses unknown to "excluded" for every operator
                        // — which is what the server's `u` means to a
                        // `WHERE` clause.
                        let Some(left) = row[1].as_deref() else {
                            assert_eq!(cell, "u", "{major} {declared}: a NULL operand");
                            assert_eq!(
                                answer(&declared, collation, &types, op, None, &bound),
                                Ok(false),
                                "{major} {declared}: a NULL field matches nothing"
                            );
                            continue;
                        };
                        let field = output(left).expect("an accepted literal has an output");
                        asserted += 1;
                        *walked.entry(case(left, right)).or_default() += 1;
                        let got = answer(&declared, collation, &types, op, Some(&field), &bound);
                        let expected = match cell {
                            "t" => true,
                            "f" => false,
                            other => panic!("{major} {declared}: unexpected cell {other:?}"),
                        };
                        if got != Ok(expected) {
                            // An exception is only ever claimable where the
                            // register has already told the user its answer
                            // may differ.
                            assert!(
                                matches!(
                                    comparison_for(&declared, collation, &types),
                                    ComparisonPlan::Compared { divergence: Some(_), .. }
                                ),
                                "{major} {declared}: disagrees while announcing no divergence"
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
            // almost nothing while passing. 28,536 today.
            assert!(asserted > 25_000, "only {asserted} cells asserted");
        }
    }
}
