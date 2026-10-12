//! The C library calls PostgreSQL's input functions make, as glibc answers
//! them: `isspace` and `strtod`. Shared by the families whose input functions
//! call them.

/// `b[at]` as C reads it, `0` past the end.
pub(crate) fn c_at(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

/// The run of ASCII digits from `at`, as its length.
fn digit_run(b: &[u8], at: usize) -> usize {
    b.get(at..).map_or(0, |rest| rest.iter().take_while(|c| c.is_ascii_digit()).count())
}

/// The longest prefix of `b[at..]` glibc's `strtod` reads in the C locale,
/// which is the server's `LC_NUMERIC`: a sign, then `inf` or `infinity`,
/// `nan` with an optional parenthesized run of letters, digits and `_`,
/// hexadecimal digits after `0x` ([`hex_float`]), or decimal ones with an
/// optional point and exponent. Its value, where it ends, and whether it lies
/// past `double precision`'s range — overflowed to an infinity, or a nonzero
/// spelling read as zero, where `strtod` sets `ERANGE`. `None` where it reads
/// nothing.
pub(crate) fn strtod(b: &[u8], at: usize) -> Option<(f64, usize, bool)> {
    let negative = c_at(b, at) == b'-';
    let i = at + usize::from(matches!(c_at(b, at), b'+' | b'-'));
    let signed = |v: f64| if negative { -v } else { v };
    let word = |w: &[u8]| b.get(i..i + w.len()).is_some_and(|s| s.eq_ignore_ascii_case(w));
    if word(b"inf") {
        let end = if word(b"infinity") { i + 8 } else { i + 3 };
        return Some((signed(f64::INFINITY), end, false));
    }
    if word(b"nan") {
        let mut end = i + 3;
        if c_at(b, end) == b'(' {
            let run = b[end + 1..]
                .iter()
                .take_while(|&&c| c.is_ascii_alphanumeric() || c == b'_')
                .count();
            if c_at(b, end + 1 + run) == b')' {
                end += run + 2;
            }
        }
        return Some((f64::NAN, end, false));
    }
    if c_at(b, i) == b'0' && matches!(c_at(b, i + 1), b'x' | b'X') {
        return Some(match hex_float(b, i + 2) {
            Some((value, end, out_of_range)) => (signed(value), end, out_of_range),
            // `0x` with no digit after it is the number `0`, the `x` unread.
            None => (signed(0.0), i + 1, false),
        });
    }
    let whole = digit_run(b, i);
    let mut end = i + whole;
    let mut fraction = 0;
    if c_at(b, end) == b'.' {
        fraction = digit_run(b, end + 1);
        end += 1 + fraction;
    }
    if whole + fraction == 0 {
        return None;
    }
    let mantissa_end = end;
    if matches!(c_at(b, end), b'e' | b'E') {
        let sign = usize::from(matches!(c_at(b, end + 1), b'+' | b'-'));
        let digits = digit_run(b, end + 1 + sign);
        if digits > 0 {
            end += 1 + sign + digits;
        }
    }
    // Every byte read is ASCII, and Rust's parse rounds to nearest as glibc's
    // does (I59), so the two agree on the value and on where it overflows.
    let value: f64 = std::str::from_utf8(&b[at..end]).ok()?.parse().ok()?;
    let out_of_range = value.is_infinite()
        || (value == 0.0 && b[i..mantissa_end].iter().any(|c| matches!(c, b'1'..=b'9')));
    Some((value, end, out_of_range))
}

/// The hexadecimal number `strtod` reads from `at`, just past `0x`: hex
/// digits with an optional point, at least one of them, then an optional `p`
/// and a decimal binary exponent; its magnitude rounded to nearest, ties to
/// even, as glibc rounds it, where it ends, and whether it lies past
/// `double precision`'s range. `None` where no digit follows.
fn hex_float(b: &[u8], at: usize) -> Option<(f64, usize, bool)> {
    let mut i = at;
    // The leading significant digits, at most fifteen — sixty bits, past the
    // fifty-three a double keeps plus a rounding bit — and whether any digit
    // past them is nonzero; `exponent` is the power of two of
    // `mantissa`'s last bit.
    let (mut mantissa, mut kept, mut sticky) = (0u64, 0u32, false);
    let (mut exponent, mut digits, mut point) = (0i64, 0usize, false);
    loop {
        let c = c_at(b, i);
        if c == b'.' && !point {
            point = true;
            i += 1;
            continue;
        }
        let Some(digit) = char::from(c).to_digit(16) else { break };
        digits += 1;
        i += 1;
        if mantissa == 0 && digit == 0 {
            exponent -= if point { 4 } else { 0 };
        } else if kept < 15 {
            mantissa = mantissa << 4 | u64::from(digit);
            kept += 1;
            exponent -= if point { 4 } else { 0 };
        } else {
            sticky |= digit != 0;
            exponent += if point { 0 } else { 4 };
        }
    }
    if digits == 0 {
        return None;
    }
    let mut end = i;
    if matches!(c_at(b, i), b'p' | b'P') {
        let sign = usize::from(matches!(c_at(b, i + 1), b'+' | b'-'));
        let run = digit_run(b, i + 1 + sign);
        if run > 0 {
            // Saturated far past any exponent a double reaches either way.
            let power = b[i + 1 + sign..i + 1 + sign + run]
                .iter()
                .fold(0i64, |n, &d| (n * 10 + i64::from(d - b'0')).min(1 << 40));
            exponent += if c_at(b, i + 1) == b'-' { -power } else { power };
            end = i + 1 + sign + run;
        }
    }
    if mantissa == 0 {
        return Some((0.0, end, false));
    }
    let bits = i64::from(64 - mantissa.leading_zeros());
    let top = exponent + bits - 1;
    if top > 1023 {
        return Some((f64::INFINITY, end, true));
    }
    // The bits the result keeps: fifty-three for a normal double, fewer the
    // further a subnormal's leading bit sits below 2^-1022.
    let precision = if top >= -1022 { 53 } else { 53 - (-1022 - top) };
    if precision < 0 {
        return Some((0.0, end, true));
    }
    let shift = bits - precision;
    let rounded = if shift > 0 {
        let shift = shift as u32;
        let (kept, rest, half) =
            (mantissa >> shift, mantissa & ((1 << shift) - 1), 1u64 << (shift - 1));
        kept + u64::from(rest > half || (rest == half && (sticky || kept & 1 == 1)))
    } else {
        mantissa << (-shift) as u32
    };
    if precision < 53 {
        // `rounded` counts multiples of 2^-1074, a carry into bit 52 being
        // the least normal double, which its bit pattern reads as.
        let value = f64::from_bits(rounded);
        return Some((value, end, value == 0.0));
    }
    let (fraction, top) = if rounded == 1 << 53 { (0, top + 1) } else { (rounded, top) };
    if top > 1023 {
        return Some((f64::INFINITY, end, true));
    }
    let biased = u64::try_from(top + 1023).ok()?;
    Some((f64::from_bits(biased << 52 | (fraction & ((1 << 52) - 1))), end, false))
}

/// C's `isspace` in the server's locale over a byte below `0x80`, which is
/// what the input functions below skip: a byte past it is a blank in no
/// locale a server runs in.
pub(crate) fn is_c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0B | 0x0C | b'\r')
}

/// What [`strtod_token`] found.
pub(crate) enum StrtodToken {
    Decimal,
    /// `inf`, `infinity` or `nan`, any tag included.
    Word,
    /// A hexadecimal number: where its digits and any point among them lie
    /// in the text, how many digits precede the point, and its binary
    /// exponent, saturated far past any type's range.
    Hex {
        digits: std::ops::Range<usize>,
        integer: usize,
        exponent: i64,
    },
}

/// The longest prefix of `b` glibc's `strtod` converts in the C locale, and
/// what it is: `None` where it converts none. A sign, then a word, a
/// hexadecimal number — `0x`, hex digits with at most one point among them,
/// at least one digit, then a `p` exponent only where a digit follows it — or
/// a decimal one, shaped alike with an `e` exponent. `0x` with no digit after
/// it is the decimal `0`.
pub(crate) fn strtod_token(b: &[u8]) -> Option<(usize, StrtodToken)> {
    let mut at = usize::from(matches!(b.first(), Some(b'+' | b'-')));
    let word = |at: usize, word: &[u8]| {
        b.get(at..at + word.len()).is_some_and(|s| s.eq_ignore_ascii_case(word))
    };
    if word(at, b"inf") {
        at += 3;
        if word(at, b"inity") {
            at += 5;
        }
        return Some((at, StrtodToken::Word));
    }
    if word(at, b"nan") {
        at += 3;
        if b.get(at) == Some(&b'(') {
            let tag =
                b[at + 1..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
            if b.get(at + 1 + tag) == Some(&b')') {
                at += tag + 2;
            }
        }
        return Some((at, StrtodToken::Word));
    }
    // The digits of `radix` from `at` on.
    let run = |at: usize, radix: u32| {
        b.get(at..)
            .map_or(0, |rest| rest.iter().take_while(|c| char::from(**c).is_digit(radix)).count())
    };
    // A `marker` exponent at `at`: its length, and its value, saturated.
    let exponent = |at: usize, marker: u8| {
        if !b.get(at).is_some_and(|c| c.to_ascii_lowercase() == marker) {
            return (0, 0);
        }
        let signed = usize::from(matches!(b.get(at + 1), Some(b'+' | b'-')));
        let digits = run(at + 1 + signed, 10);
        if digits == 0 {
            return (0, 0);
        }
        let magnitude = b[at + 1 + signed..at + 1 + signed + digits]
            .iter()
            .fold(0i64, |n, d| (n * 10 + i64::from(d - b'0')).min(1 << 40));
        let value = if b[at + 1] == b'-' { -magnitude } else { magnitude };
        (1 + signed + digits, value)
    };
    // A mantissa of `radix` digits from `at`: where its point is, and its
    // length, the point consumed only beside a digit.
    let mantissa = |at: usize, radix: u32| {
        let integer = run(at, radix);
        let fraction =
            if b.get(at + integer) == Some(&b'.') { run(at + integer + 1, radix) } else { 0 };
        let point = usize::from(b.get(at + integer) == Some(&b'.'));
        (integer, fraction, if integer + fraction == 0 { 0 } else { integer + point + fraction })
    };
    if matches!(b.get(at..at + 2), Some([b'0', b'x' | b'X'])) {
        let (integer, _, len) = mantissa(at + 2, 16);
        if len > 0 {
            let first = at + 2;
            let (exponent_len, exponent) = exponent(first + len, b'p');
            let digits = first..first + len;
            return Some((
                first + len + exponent_len,
                StrtodToken::Hex { digits, integer, exponent },
            ));
        }
    }
    let (_, _, len) = mantissa(at, 10);
    if len == 0 {
        return None;
    }
    let (exponent_len, _) = exponent(at + len, b'e');
    Some((at + len + exponent_len, StrtodToken::Decimal))
}
