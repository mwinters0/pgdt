//! **`numeric_in` fed a text a byte at a time** ([`NumericIn`]): the grammar
//! this build reads, and beside it each supported major's, saying why a text
//! it does not read is no value — holding the significant digits its answer
//! reads and counting every other (`docs/design/decisions.md`, "D148") — and
//! what a column holds a value to, the storage format's bounds and a typmod's
//! rounding.

use crate::decode::{Read, Unread};
use crate::value::libc::is_c_space;

/// `NUMERIC_DSCALE_MAX`: the most digits `numeric_in` stores after the point,
/// a trailing zero counting (I63).
pub const NUMERIC_DSCALE_MAX: usize = 16383;

/// The most digits `numeric_in` stores before the point, a leading zero not
/// counting: `NUMERIC_WEIGHT_MAX`, `i16::MAX`, base-10000 digits past the
/// first (I63).
pub const NUMERIC_INTEGER_DIGITS_MAX: usize = 4 * (i16::MAX as usize + 1);

/// What a `numeric` column's declaration does with a value `numeric_in` reads:
/// keeps it within the storage format's bounds (I63), rounds it to `scale` and
/// holds it to `precision` (I51), or a typmod this build does not read, which
/// does one of those to a value it cannot tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NumericColumn {
    Bare,
    Typmod { precision: u16, scale: i16 },
    UnreadTypmod,
}

/// [`typmod_unscaled_digits`] once the text is split: `int_part` and
/// `frac_part` ASCII digits, either empty, a leading or trailing zero read
/// as none.
// pg-refuses: I51 — a value past the precision once rounded to the scale.
pub(super) fn typmod_round(
    neg: bool,
    int_part: &str,
    frac_part: &str,
    precision: u16,
    scale: i16,
) -> Read<String> {
    // As in `decimal_unscaled_digits`, `int_part ++ frac_part` is indexed and
    // never materialized: the unscaled value is its first `keep` digits,
    // incremented where the first digit past the scale is 5 or more, then
    // `pad` zeros.
    let total = int_part.len() + frac_part.len();
    let digit = |i: usize| {
        if i < int_part.len() {
            int_part.as_bytes()[i]
        } else {
            frac_part.as_bytes()[i - int_part.len()]
        }
    };
    let shift = i64::from(scale) - frac_part.len() as i64;
    let (keep, pad, round_up) = match usize::try_from(-shift) {
        // Every digit past the scale is cut. Past the last written one, the
        // first cut digit is an implied leading zero, which rounds down.
        Ok(cut) if cut > total => (0, 0, false),
        Ok(cut) => (total - cut, 0, cut > 0 && digit(total - cut) >= b'5'),
        Err(_) => (total, usize::try_from(shift).expect("a positive shift"), false),
    };
    let start = (0..keep).find(|&i| digit(i) != b'0').unwrap_or(keep);
    // Zero, rounded or written, carries no sign: `-0.004` at scale 2 is `0`.
    if start == keep && !round_up {
        return Ok("0".to_string());
    }
    let sign = usize::from(neg);
    let mut out = String::with_capacity(sign + keep - start + 1 + pad);
    if neg {
        out.push('-');
    }
    out.extend((start..keep).map(|i| char::from(digit(i))));
    if round_up {
        // The carry turns a run of trailing nines into zeros and raises the
        // digit before them, or adds a leading `1` where every digit was a
        // nine: rounding can raise the weight, which is why the precision is
        // checked after it.
        let nines = out[sign..].bytes().rev().take_while(|&b| b == b'9').count();
        out.truncate(out.len() - nines);
        if out.len() > sign {
            let last = out.pop().expect("a digit is past the sign");
            out.push(char::from(last as u8 + 1));
        } else {
            out.push('1');
        }
        out.extend(std::iter::repeat_n('0', nines));
    }
    if out.len() - sign + pad > usize::from(precision) {
        return Err(Unread::Refused);
    }
    out.extend(std::iter::repeat_n('0', pad));
    Ok(out)
}

/// The most bits a value with [`NUMERIC_INTEGER_DIGITS_MAX`] digits can need,
/// one more than `10^131072`'s width less one: it is the one width the two
/// sides of the bound share.
pub(super) const NUMERIC_INTEGER_BITS_MAX: usize = 435_412;

/// An integer's ASCII `digits` in `radix` (2, 8 or 16), written in decimal.
/// Quadratic in their number, which a caller bounds.
pub(super) fn radix_to_decimal(digits: &[u8], radix: u32) -> String {
    const LIMB: u64 = 1_000_000_000;
    // As many digits a step as keep the multiplier at most 2^32.
    let step = (32 / radix.trailing_zeros()) as usize;
    let mut limbs: Vec<u64> = vec![0];
    for chunk in digits.chunks(step) {
        let multiplier = u64::from(radix).pow(chunk.len() as u32);
        let mut carry = chunk.iter().fold(0u64, |n, &d| {
            n * u64::from(radix) + u64::from(char::from(d).to_digit(radix).expect("a digit"))
        });
        for limb in &mut limbs {
            let value = *limb * multiplier + carry;
            *limb = value % LIMB;
            carry = value / LIMB;
        }
        while carry > 0 {
            limbs.push(carry % LIMB);
            carry /= LIMB;
        }
    }
    let mut out = limbs.last().expect("one limb at least").to_string();
    for limb in limbs.iter().rev().skip(1) {
        out.push_str(&format!("{limb:09}"));
    }
    out
}

/// The longest of `numeric_out`'s special spellings, `-Infinity`.
const WORD_MAX: usize = 9;

/// **`numeric_in` read a byte at a time into a `numeric` column**: the
/// grammar this build reads, `[-]digits[.digits]`, its significant digits
/// held only as far as the column's answer reads them — a typmod'd column's
/// to its precision and scale (I51), a bare one's to the storage format's
/// bounds (I63), past which it is refused, and a typmod this build does not
/// read keeping them all, its key being them — and beside it v14's and
/// v16's readings (I82), each holding no more than whether `column` stores
/// what it read needs. Its first bytes are kept for `numeric_out`'s special
/// spellings.
#[derive(Debug, Clone)]
pub(crate) struct NumericIn {
    column: NumericColumn,
    word: [u8; WORD_MAX],
    /// The text's length, saturating.
    len: usize,
    plain: Plain,
    /// v14's reading, then v16's.
    majors: [Spelling; 2],
}

impl NumericIn {
    pub(crate) fn new(column: NumericColumn) -> Self {
        Self {
            column,
            word: [0; WORD_MAX],
            len: 0,
            plain: Plain::new(column),
            majors: [Spelling::new(false), Spelling::new(true)],
        }
    }

    /// The reader of `text`, fed whole: the majors read it only where this
    /// build's grammar does not, which is all they are asked of.
    pub(crate) fn whole(column: NumericColumn, text: &[u8]) -> Self {
        let mut read = Self::new(column);
        read.whole_text(text);
        read
    }

    /// Read `text`, the whole of what this reader, begun afresh, is fed.
    pub(crate) fn whole_text(&mut self, text: &[u8]) {
        for &byte in text {
            self.plain.byte(byte);
            self.keep(byte);
        }
        if !self.plain.read() {
            for &byte in text {
                self.majors.iter_mut().for_each(|major| major.byte(byte, self.column));
            }
        }
    }

    /// Ready the reader for another text.
    pub(crate) fn start(&mut self) {
        *self = Self::new(self.column);
    }

    /// Read the text's next byte.
    pub(crate) fn byte(&mut self, byte: u8) {
        self.plain.byte(byte);
        self.keep(byte);
        self.majors.iter_mut().for_each(|major| major.byte(byte, self.column));
    }

    fn keep(&mut self, byte: u8) {
        if let Some(at) = self.word.get_mut(self.len) {
            *at = byte;
        }
        self.len = self.len.saturating_add(1);
    }

    /// The text, where it is no longer than a special spelling.
    pub(crate) fn word(&self) -> Option<&str> {
        std::str::from_utf8(self.word.get(..self.len)?).ok()
    }

    /// A typmod'd column's field as `COPY` stores it, an unscaled digit
    /// string at the scale ([`typmod_unscaled_digits`]'s answer).
    ///
    /// [`typmod_unscaled_digits`]: super::typmod_unscaled_digits
    // pg-refuses: I51 — a value past the precision once rounded to the scale.
    pub(crate) fn unscaled(&self) -> Read<String> {
        let NumericColumn::Typmod { precision, scale } = self.column else {
            unreachable!("only a typmod'd column's field is put through its typmod")
        };
        let plain = &self.plain;
        if !plain.read() {
            return Err(self.unread());
        }
        // More integer digits than it holds are more than the precision
        // leaves room for at the scale, whatever rounds.
        if plain.int.count > plain.int.held.len() {
            return Err(Unread::Refused);
        }
        typmod_round(
            plain.negative,
            ascii(&plain.int.held),
            ascii(&plain.frac.held),
            precision,
            scale,
        )
    }

    /// A bare column's value, or one's whose typmod this build does not
    /// read, as its key is built from it (`NumericKey::from_parts`): its
    /// sign, its integer's digits from the first nonzero and its fraction's
    /// to the last nonzero.
    // pg-refuses: I63 — a display scale or a weight past the storage format's.
    pub(crate) fn parts(&self) -> Read<(bool, &str, &str)> {
        let plain = &self.plain;
        if !plain.read() {
            return Err(self.unread());
        }
        if self.column == NumericColumn::Bare
            && (plain.int.count > NUMERIC_INTEGER_DIGITS_MAX
                || plain.frac.places > NUMERIC_DSCALE_MAX)
        {
            return Err(Unread::Refused);
        }
        Ok((plain.negative, ascii(&plain.int.held), ascii(&plain.frac.held)))
    }

    /// Why `numeric_in` reads no value of the column here, this build's
    /// grammar having read none: refused only where every major refuses it.
    /// From v16 it reads a non-decimal integer and an underscore between
    /// digits, and before it an exponent `strtol` reads, a blank after the
    /// `e` included, and refuses one at `PG_INT32_MAX / 2` that v16 reads, so
    /// neither grammar holds the other's every text (I82). Each value read is
    /// then held to the column.
    fn unread(&self) -> Unread {
        if self.majors.iter().all(|major| !major.stored(self.column)) {
            Unread::Refused
        } else {
            Unread::Unparsed
        }
    }
}

/// Digits read as ASCII.
fn ascii(digits: &[u8]) -> &str {
    std::str::from_utf8(digits).expect("ASCII digits")
}

/// How many significant integer digits and fraction places `column` reads:
/// past them, a typmod'd column refuses the value (I51) and a bare one too
/// (I63).
fn caps(column: NumericColumn) -> (usize, usize) {
    match column {
        NumericColumn::Bare => (NUMERIC_INTEGER_DIGITS_MAX, NUMERIC_DSCALE_MAX),
        // The digit after the scale rounds; an integer digit more than the
        // precision leaves for the scale is past it.
        NumericColumn::Typmod { precision, scale } => (
            usize::try_from(i32::from(precision) - i32::from(scale) + 1).unwrap_or(0),
            usize::try_from(i32::from(scale) + 1).unwrap_or(0),
        ),
        NumericColumn::UnreadTypmod => (usize::MAX, usize::MAX),
    }
}

/// An integer's digits from its first nonzero one: as many held as a cap
/// allows, and how many there are.
#[derive(Debug, Clone, Default)]
struct Leading {
    held: Vec<u8>,
    count: usize,
}

impl Leading {
    fn digit(&mut self, digit: u8, cap: usize) {
        if self.count == 0 && digit == b'0' {
            return;
        }
        if self.held.len() < cap {
            self.held.push(digit);
        }
        self.count = self.count.saturating_add(1);
    }
}

/// A fraction's digits to its last nonzero one, as many places held as a cap
/// allows, and how many places there are.
#[derive(Debug, Clone, Default)]
struct Trailing {
    held: Vec<u8>,
    places: usize,
}

impl Trailing {
    fn digit(&mut self, digit: u8, cap: usize) {
        self.places = self.places.saturating_add(1);
        if digit != b'0' && self.places <= cap {
            self.held.resize(self.places - 1, b'0');
            self.held.push(digit);
        }
    }
}

/// **The grammar this build reads**: `[-]digits[.digits]`, no exponent, no
/// sign but `-`, at least one digit, a point at either end and leading
/// zeros allowed, which `numeric_out` never writes (`key::form::accepted_form`
/// lists them).
#[derive(Debug, Clone)]
struct Plain {
    stage: PlainStage,
    negative: bool,
    digit: bool,
    int: Leading,
    frac: Trailing,
    caps: (usize, usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlainStage {
    Start,
    Signed,
    Int,
    Frac,
    Refused,
}

impl Plain {
    fn new(column: NumericColumn) -> Self {
        Self {
            stage: PlainStage::Start,
            negative: false,
            digit: false,
            int: Leading::default(),
            frac: Trailing::default(),
            caps: caps(column),
        }
    }

    fn byte(&mut self, byte: u8) {
        self.stage = match (self.stage, byte) {
            (PlainStage::Start, b'-') => {
                self.negative = true;
                PlainStage::Signed
            }
            (PlainStage::Start | PlainStage::Signed | PlainStage::Int, b'0'..=b'9') => {
                self.digit = true;
                self.int.digit(byte, self.caps.0);
                PlainStage::Int
            }
            (PlainStage::Start | PlainStage::Signed | PlainStage::Int, b'.') => PlainStage::Frac,
            (PlainStage::Frac, b'0'..=b'9') => {
                self.digit = true;
                self.frac.digit(byte, self.caps.1);
                PlainStage::Frac
            }
            (PlainStage::Refused, _) => return,
            _ => {
                // What it held answers nothing now.
                self.int = Leading::default();
                self.frac = Trailing::default();
                PlainStage::Refused
            }
        };
    }

    /// Whether the text read is one of the grammar's.
    fn read(&self) -> bool {
        self.digit && matches!(self.stage, PlainStage::Int | PlainStage::Frac)
    }
}

/// What a major's `numeric_in` reads a text as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    NaN,
    Infinite,
    Decimal,
    /// An integer in this radix, from v16.
    Radix(u32),
}

/// **One major's `numeric_in`**, v14's — `set_var_from_str` reading its
/// exponent by `strtol` — or v16's — a non-decimal integer after `0x`, `0o`
/// or `0b`, and an underscore between digits, the exponent's too: what it
/// reads, or that it refuses the text as syntax or as an exponent out of
/// bounds. v13 reads v14's texts but the infinities.
#[derive(Debug, Clone)]
struct Spelling {
    v16: bool,
    stage: Stage,
    form: Form,
    negative: bool,
    /// The mantissa's digits, and those before its point.
    count: i64,
    integer: i64,
    /// Its significant digits, held as far as the column reads them.
    significant: Leading,
    exponent: i64,
    exponent_negative: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Blanks before the text.
    Lead,
    /// Past the sign.
    Signed,
    /// So many letters of `NaN`, or of `Infinity`.
    Word {
        nan: bool,
        read: u8,
    },
    /// A `0` where v16's prefix may open, its next byte deciding.
    Zero,
    /// A point before any digit, which one must follow.
    Point,
    /// The mantissa's digits: past its point where `point`, just past it
    /// where `dot`, and just past an underscore where `underscore`.
    Mantissa {
        point: bool,
        dot: bool,
        underscore: bool,
    },
    /// `e` or `E`, the blanks `strtol` skips after it, and its sign.
    Marker,
    MarkerBlank,
    MarkerSign,
    Exponent {
        underscore: bool,
    },
    /// Past v16's prefix: `first` before any digit or underscore.
    Radix {
        first: bool,
        underscore: bool,
    },
    /// Blanks after the text.
    Trail,
    Refused,
}

/// The most an exponent's magnitude is read to, far past either bound.
const EXPONENT_SATURATED: i64 = 1 << 40;

impl Spelling {
    fn new(v16: bool) -> Self {
        Self {
            v16,
            stage: Stage::Lead,
            form: Form::Decimal,
            negative: false,
            count: 0,
            integer: 0,
            significant: Leading::default(),
            exponent: 0,
            exponent_negative: false,
        }
    }

    /// Read the text's next byte.
    // pg-refuses: I82 — every refusal here is `numeric_in`'s at v14 or from v16.
    fn byte(&mut self, byte: u8, column: NumericColumn) {
        let blank = is_c_space(byte);
        let digit = byte.is_ascii_digit();
        let lower = byte.to_ascii_lowercase();
        self.stage = match self.stage {
            Stage::Refused => return,
            Stage::Lead if blank => Stage::Lead,
            Stage::Lead if lower == b'n' => {
                self.form = Form::NaN;
                Stage::Word { nan: true, read: 1 }
            }
            Stage::Lead if matches!(byte, b'+' | b'-') => {
                self.negative = byte == b'-';
                Stage::Signed
            }
            Stage::Lead | Stage::Signed if lower == b'i' => {
                self.form = Form::Infinite;
                Stage::Word { nan: false, read: 1 }
            }
            Stage::Lead | Stage::Signed if byte == b'.' => Stage::Point,
            Stage::Lead | Stage::Signed if byte == b'0' && self.v16 => Stage::Zero,
            Stage::Lead | Stage::Signed | Stage::Point if digit => {
                self.digit(byte, column, self.stage == Stage::Point)
            }
            Stage::Word { nan, read } => {
                let word: &[u8] = if nan { b"nan" } else { b"infinity" };
                let end = if nan { read == 3 } else { matches!(read, 3 | 8) };
                match word.get(usize::from(read)) {
                    Some(&letter) if lower == letter => Stage::Word { nan, read: read + 1 },
                    _ if blank && end => Stage::Trail,
                    _ => self.refuse(),
                }
            }
            Stage::Zero => {
                let radix = match lower {
                    b'x' => 16,
                    b'o' => 8,
                    b'b' => 2,
                    _ => 0,
                };
                if radix == 0 {
                    // A decimal `0`, and the byte read after it.
                    self.stage = self.digit(b'0', column, false);
                    self.byte(byte, column);
                    return;
                }
                self.form = Form::Radix(radix);
                Stage::Radix { first: true, underscore: false }
            }
            Stage::Mantissa { point, dot, underscore } => match byte {
                b'0'..=b'9' => self.digit(byte, column, point),
                b'.' if point || underscore => self.refuse(),
                b'.' => Stage::Mantissa { point: true, dot: true, underscore: false },
                b'_' if self.v16 && !dot && !underscore => {
                    Stage::Mantissa { point, dot: false, underscore: true }
                }
                b'e' | b'E' if !underscore => Stage::Marker,
                _ if blank && !underscore => Stage::Trail,
                _ => self.refuse(),
            },
            Stage::Marker | Stage::MarkerBlank if blank && !self.v16 => Stage::MarkerBlank,
            Stage::Marker | Stage::MarkerBlank if matches!(byte, b'+' | b'-') => {
                self.exponent_negative = byte == b'-';
                Stage::MarkerSign
            }
            Stage::Marker | Stage::MarkerBlank | Stage::MarkerSign | Stage::Exponent { .. }
                if digit =>
            {
                self.exponent =
                    (self.exponent * 10 + i64::from(byte - b'0')).min(EXPONENT_SATURATED);
                Stage::Exponent { underscore: false }
            }
            Stage::Exponent { underscore: false } if byte == b'_' && self.v16 => {
                Stage::Exponent { underscore: true }
            }
            Stage::Exponent { underscore: false } if blank => Stage::Trail,
            Stage::Radix { first, underscore } => {
                let Form::Radix(radix) = self.form else { unreachable!("a radix is read") };
                if char::from(byte).is_digit(radix) {
                    self.significant.digit(byte, radix_cap(column));
                    Stage::Radix { first: false, underscore: false }
                } else if byte == b'_' && !underscore {
                    Stage::Radix { first: false, underscore: true }
                } else if blank && !first && !underscore {
                    Stage::Trail
                } else {
                    self.refuse()
                }
            }
            Stage::Trail if blank => Stage::Trail,
            _ => self.refuse(),
        };
    }

    /// A mantissa digit, after its point where `point`.
    fn digit(&mut self, byte: u8, column: NumericColumn, point: bool) -> Stage {
        // A typmod'd column rounds at most its precision's digits and one.
        let cap = match column {
            NumericColumn::Typmod { precision, .. } => usize::from(precision) + 1,
            NumericColumn::Bare | NumericColumn::UnreadTypmod => 0,
        };
        self.count = self.count.saturating_add(1);
        self.integer = self.integer.saturating_add(i64::from(!point));
        self.significant.digit(byte, cap);
        Stage::Mantissa { point, dot: false, underscore: false }
    }

    fn refuse(&mut self) -> Stage {
        self.significant = Leading::default();
        Stage::Refused
    }

    /// What the text reads as, once it has ended, or `None` where it is
    /// refused.
    fn read(&self) -> Option<Form> {
        let read = match self.stage {
            Stage::Word { nan: true, read } => read == 3,
            Stage::Word { nan: false, read } => matches!(read, 3 | 8),
            Stage::Zero | Stage::Mantissa { underscore: false, .. } | Stage::Trail => true,
            Stage::Exponent { underscore } => !underscore,
            Stage::Radix { first, underscore } => !first && !underscore,
            _ => false,
        };
        let bound = i64::from(i32::MAX / 2);
        let within = if self.v16 { self.exponent <= bound } else { self.exponent < bound };
        (read && within).then_some(self.form)
    }

    /// Whether `column` stores the value read: within the storage format's
    /// bounds for a bare one (I63), within the precision once rounded to the
    /// scale for a typmod'd one (I51), and an infinity under no typmod (I34).
    // pg-refuses: I63 — a display scale or a weight past the storage format's.
    fn stored(&self, column: NumericColumn) -> bool {
        let Some(form) = self.read() else { return false };
        // A `0` v16's prefix might have followed is a digit.
        let count = self.count + i64::from(self.stage == Stage::Zero);
        let integer = self.integer + i64::from(self.stage == Stage::Zero);
        let significant = &self.significant;
        match (form, column) {
            (Form::NaN, _) | (_, NumericColumn::UnreadTypmod) => true,
            (Form::Infinite, column) => column == NumericColumn::Bare,
            (Form::Decimal, column) => {
                let exponent = if self.exponent_negative { -self.exponent } else { self.exponent };
                let point = integer + exponent;
                let dscale = (count - integer - exponent).max(0);
                if significant.count == 0 {
                    return column != NumericColumn::Bare || dscale <= NUMERIC_DSCALE_MAX as i64;
                }
                let first = count - significant.count as i64;
                let NumericColumn::Typmod { precision, scale } = column else {
                    return dscale <= NUMERIC_DSCALE_MAX as i64
                        && point - first <= NUMERIC_INTEGER_DIGITS_MAX as i64;
                };
                // The digit the scale rounds at, counted from the first
                // significant one: before it, the value rounds to zero, or to
                // one unit of the scale; past the precision, it can only grow
                // by rounding.
                let round = point + i64::from(scale) - first;
                if round <= 0 {
                    return true;
                }
                if round > i64::from(precision) {
                    return false;
                }
                // Within the precision and the scale of the first significant
                // digit, so short; every digit held reaches the one rounded.
                let digits = ascii(&significant.held);
                let within = point - first;
                let zeros = |n: i64| "0".repeat(usize::try_from(n).unwrap_or(0));
                let len = digits.len() as i64;
                let (int, frac) = match within {
                    _ if within <= 0 => (String::new(), format!("{}{digits}", zeros(-within))),
                    _ if within >= len => {
                        (format!("{digits}{}", zeros(within - len)), String::new())
                    }
                    _ => {
                        let (int, frac) = digits.split_at(within as usize);
                        (int.to_string(), frac.to_string())
                    }
                };
                typmod_round(self.negative, &int, &frac, precision, scale).is_ok()
            }
            (Form::Radix(radix), column) => {
                if significant.count == 0 {
                    return true;
                }
                let digits = &significant.held;
                let lead = char::from(digits[0]).to_digit(radix).expect("a digit of the radix");
                let bits = (significant.count - 1)
                    .saturating_mul(radix.trailing_zeros() as usize)
                    .saturating_add((32 - lead.leading_zeros()) as usize);
                let decimal = match column {
                    NumericColumn::Bare if bits > NUMERIC_INTEGER_BITS_MAX => return false,
                    NumericColumn::Bare if bits < NUMERIC_INTEGER_BITS_MAX => return true,
                    NumericColumn::Bare => {
                        return radix_to_decimal(digits, radix).len() <= NUMERIC_INTEGER_DIGITS_MAX;
                    }
                    // Past four bits a digit it is past the precision
                    // whatever the scale rounds.
                    NumericColumn::Typmod { precision, scale }
                        if bits as i64 > 4 * (i64::from(precision) - i64::from(scale) + 1) =>
                    {
                        return false;
                    }
                    NumericColumn::Typmod { .. } => radix_to_decimal(digits, radix),
                    NumericColumn::UnreadTypmod => unreachable!("answered above"),
                };
                let NumericColumn::Typmod { precision, scale } = column else {
                    unreachable!("answered above")
                };
                typmod_round(self.negative, &decimal, "", precision, scale).is_ok()
            }
        }
    }
}

/// How many of a radix spelling's significant digits `column` reads: its
/// first, which its width in bits counts from, and for a bare one every digit
/// up to I63's bound in bits, where only its decimal length decides, and a
/// typmod'd one's up to four bits a decimal digit of its precision less its
/// scale.
fn radix_cap(column: NumericColumn) -> usize {
    match column {
        NumericColumn::Bare => NUMERIC_INTEGER_BITS_MAX,
        NumericColumn::Typmod { precision, scale } => {
            usize::try_from(4 * (i32::from(precision) - i32::from(scale) + 1)).unwrap_or(0).max(1)
        }
        NumericColumn::UnreadTypmod => 1,
    }
}
