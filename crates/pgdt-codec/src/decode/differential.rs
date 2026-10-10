use super::digits::push_integer;
use super::*;

/// A deterministic 64-bit LCG, so a failure is reproducible from the
/// seed alone and the corpus does not have to be committed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 =
            self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick(&mut self, from: &[u8]) -> char {
        char::from(from[self.below(from.len())])
    }
}

/// Bytes that reach a hex or digit loop: the valid ones, the ones that
/// sit just outside each accepted range, and the separators the grammars
/// use; `fuzz` and `shaped` add one multi-byte character.
const ALPHABET: &[u8] = b"0123456789abcdefABCDEFgGxX-.:+ \\/`z@\x7f";

fn fuzz(seed: u64, len: usize, f: impl Fn(&str)) {
    let mut rng = Rng(seed);
    for _ in 0..20_000 {
        let n = rng.below(len) + 1;
        let mut s = String::new();
        for _ in 0..n {
            if rng.below(32) == 0 {
                s.push('é');
            } else {
                s.push(rng.pick(ALPHABET));
            }
        }
        f(&s);
    }
}

/// The free-form corpus above almost never lands on a *well-formed*
/// literal — one in twenty thousand, for a time of day — so each decoder
/// also gets a corpus built to its own grammar and then perturbed, which
/// is where the accepting branches actually get exercised.
fn shaped(seed: u64, build: impl Fn(&mut Rng) -> String, f: impl Fn(&str)) {
    let mut rng = Rng(seed);
    for _ in 0..20_000 {
        let s = build(&mut rng);
        // Two thirds are left well-formed; the rest are damaged at one
        // position, which is what puts a rejecting branch beside every
        // accepting one.
        let s = if rng.below(3) == 0 && !s.is_empty() {
            let mut chars: Vec<char> = s.chars().collect();
            let at = rng.below(chars.len());
            chars[at] = if rng.below(16) == 0 { 'é' } else { rng.pick(ALPHABET) };
            chars.into_iter().collect()
        } else {
            s
        };
        f(&s);
    }
}

fn digits(rng: &mut Rng, count: usize) -> String {
    (0..count).map(|_| rng.pick(b"0123456789")).collect()
}

/// Whether `time_overflows` or a signed part refuses `s`, which the
/// shape [`parse_time_of_day`] replaced read: an hour past 24, a minute
/// past 59, a second past 60 or a whole past `24:00:00`, or a part
/// carrying a sign, the server's zone (I61).
fn past_time_overflows(s: &str) -> bool {
    let (hms, frac) = s.split_once('.').unwrap_or((s, ""));
    let parts: Vec<&str> = hms.split(':').collect();
    if parts.iter().any(|p| !p.bytes().all(|b| b.is_ascii_digit())) {
        return true;
    }
    let [h, mi, se] = [0, 1, 2].map(|i| parts[i].parse::<i64>().unwrap());
    let fraction = !frac.is_empty() && frac.bytes().any(|b| b != b'0');
    h > 24 || mi > 59 || se > 60 || (h * 3600 + mi * 60 + se, fraction) > (86_400, false)
}

/// **The rewrite reads what the prior shape read, inside the bounds the
/// server keeps**: past them the prior shape read on where this refuses.
#[test]
fn time_of_day_agrees_with_the_shape_it_replaced() {
    let check = |s: &str| match (parse_time_of_day(s), prior_shape::parse_time_of_day(s)) {
        (Err(Unread::Refused), Some(_)) => assert!(past_time_overflows(s), "{s:?}"),
        (Err(Unread::Refused), None) => {}
        // A signed part, which the server reads as a zone (I61).
        (Err(Unread::Unparsed), Some(_)) => assert!(s.contains(['+', '-']), "{s:?}"),
        (got, prior) => assert_eq!(got.ok(), prior, "{s:?}"),
    };
    fuzz(1, 20, check);
    shaped(
        11,
        |rng| {
            let widths = [rng.below(3) + 1, rng.below(3) + 1, rng.below(3) + 1];
            let h = digits(rng, widths[0]);
            let mi = digits(rng, widths[1]);
            let se = digits(rng, widths[2]);
            let mut s = format!("{h}:{mi}:{se}");
            // Zero fractional digits through eight: six is the limit, so
            // the widths either side of it are both in the corpus.
            let frac = rng.below(9);
            if frac > 0 {
                s.push('.');
                s.push_str(&digits(rng, frac));
            }
            s
        },
        check,
    );
    // Every fractional width, which is what the multiply stands in for.
    for (text, want) in [
        ("00:00:00", 0),
        ("00:00:00.5", 500_000),
        ("00:00:00.05", 50_000),
        ("00:00:00.005", 5_000),
        ("00:00:00.0005", 500),
        ("00:00:00.00005", 50),
        ("00:00:00.000005", 5),
        ("00:00:00.000000", 0),
    ] {
        assert_eq!(parse_time_of_day(text), Ok((0, want)), "{text}");
    }
    assert_eq!(parse_time_of_day("00:00:00.0000005"), Err(Unread::Unparsed));
}

/// Whether a hyphen in `s` falls where `uuid_in` refuses one: anywhere
/// but after a group of four hex digits, the last group excepted, or
/// twice in a row.
fn hyphen_misplaced(s: &str) -> bool {
    let mut digits = 0;
    let mut after_hyphen = false;
    for b in s.bytes() {
        if b == b'-' {
            if digits == 0 || digits % 4 != 0 || digits >= 32 || after_hyphen {
                return true;
            }
            after_hyphen = true;
        } else {
            digits += 1;
            after_hyphen = false;
        }
    }
    false
}

/// **The rewrite reads what the prior shape read, with the hyphens
/// `uuid_in` places**: where the prior shape dropped a hyphen wherever it
/// fell this refuses one the server refuses, and it reads a value in
/// braces as the prior shape read the value inside them.
#[test]
fn uuid_agrees_with_the_shape_it_replaced() {
    let check = |s: &str| match (decode_uuid(s), prior_shape::decode_uuid(s)) {
        (None, Some(_)) => assert!(hyphen_misplaced(s), "{s:?}"),
        (Some(got), None) => {
            let inner = s.strip_prefix('{').and_then(|s| s.strip_suffix('}')).expect(s);
            assert_eq!(Some(got), prior_shape::decode_uuid(inner), "{s:?}");
        }
        (got, prior) => assert_eq!(got, prior, "{s:?}"),
    };
    fuzz(2, 40, check);
    shaped(
        22,
        |rng| {
            // 31, 32 or 33 hex digits, a hyphen after a group of four
            // often and anywhere else seldom, and braces now and then.
            let count = 31 + rng.below(3);
            let mut s = String::new();
            for at in 0..count {
                let odds = if at > 0 && at % 4 == 0 { 2 } else { 12 };
                if rng.below(odds) == 0 {
                    s.push('-');
                }
                s.push(rng.pick(b"0123456789abcdefABCDEF"));
            }
            if rng.below(4) == 0 {
                s = format!("{{{s}}}");
            }
            s
        },
        check,
    );
    // A canonical value, and the two ways to be the wrong length.
    let canonical = "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11";
    assert!(decode_uuid(canonical).is_some());
    assert_eq!(decode_uuid(&canonical[..35]), None);
    assert_eq!(decode_uuid(&format!("{canonical}0")), None);
}

/// **`uuid_in`'s grammar, read and refused as the server reads and refuses
/// it** (I64): each spelling below was cast on PostgreSQL 16. The first
/// six refused once read, a hyphen having been dropped wherever it fell,
/// and each read in braces was refused.
#[test]
fn a_uuid_takes_a_hyphen_only_after_a_group_of_four() {
    let canonical = decode_uuid("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11").unwrap();
    for read in [
        "a0eebc999c0b4ef8bb6d6bb9bd380a11",
        "a0ee-bc99-9c0b-4ef8-bb6d-6bb9-bd38-0a11",
        "{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}",
        "{a0eebc999c0b4ef8bb6d6bb9bd380a11}",
        "A0EEBC99-9C0B-4EF8-BB6D-6BB9BD380A11",
    ] {
        assert_eq!(decode_uuid(read), Some(canonical), "{read:?}");
    }
    for refused in [
        "-a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11-",
        "a0eebc99--9c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eebc9-99c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eeb-c999c0b4ef8bb6d6bb9bd380a11",
        "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a1-1",
        "{-a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}",
        "{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11-}",
        "{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}",
        "{{a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11}}",
        " a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11",
        "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11 ",
        "{}",
        "{",
    ] {
        assert_eq!(decode_uuid(refused), None, "{refused:?}");
    }
}

#[test]
fn bytea_agrees_with_the_shape_it_replaced() {
    let check = |s: &str| {
        let escaped = format!("\\x{s}");
        // A text without the prefix is the `escape` form, which the
        // shape replaced did not read.
        assert_eq!(decode_bytea(&escaped), prior_shape::decode_bytea(&escaped), "{escaped:?}");
    };
    fuzz(3, 24, check);
    // Both parities of length, including the empty `\x`.
    shaped(
        33,
        |rng| {
            let count = rng.below(17);
            (0..count).map(|_| rng.pick(b"0123456789abcdefABCDEF")).collect()
        },
        check,
    );
}

/// The two hex renderers take bytes rather than text, so their corpus is
/// enumerated rather than fuzzed: every entry of the pair table is
/// reachable, and for the `uuid` every entry at every one of the sixteen
/// positions the hyphens are interleaved into.
#[test]
fn hex_render_agrees_with_the_shape_it_replaced() {
    let every_byte: Vec<u8> = (0..=u8::MAX).collect();
    assert_eq!(render_bytea(&every_byte), prior_shape::render_bytea(&every_byte));
    for b in 0..=u8::MAX {
        assert_eq!(render_bytea(&[b]), prior_shape::render_bytea(&[b]), "{b}");
    }
    // The empty value (`\x` alone) and every length either side of the
    // pair-chunking the decoder's inverse does.
    for len in 0..=33usize {
        let v: Vec<u8> = (0..len).map(|i| every_byte[(i * 7 + 3) % 256]).collect();
        assert_eq!(render_bytea(&v), prior_shape::render_bytea(&v), "{v:?}");
    }

    for at in 0..16 {
        for b in 0..=u8::MAX {
            let mut bytes = [0u8; 16];
            bytes[at] = b;
            assert_eq!(render_uuid(&bytes), prior_shape::render_uuid(&bytes), "{at} {b}");
        }
    }
    let mut rng = Rng(5);
    for _ in 0..20_000 {
        let mut bytes = [0u8; 16];
        for b in &mut bytes {
            *b = rng.next() as u8;
        }
        assert_eq!(render_uuid(&bytes), prior_shape::render_uuid(&bytes), "{bytes:?}");
    }
}

/// The three date/time renderers take integers rather than text, so their
/// corpus is enumerated and swept rather than fuzzed. Three regions
/// matter and each is covered on its own: the dense one every real value
/// lands in, a stride across the whole width of the argument type, and
/// the values only a caller-assembled Arrow array can hold — a negative
/// `Time64`, an hour past 99, a year past four digits — which are what
/// the two-digit and four-digit fast paths have to fall out of rather
/// than panic on.
#[test]
fn date_and_time_render_agrees_with_the_shape_it_replaced() {
    // Every day of a six-century window around the epoch, then a stride
    // over the whole of `i32` — which reaches years either side of five
    // million and so the wide-year path — and both ends exactly.
    for days in -200_000..=200_000i32 {
        assert_eq!(render_date32(days), prior_shape::render_date32(days), "{days}");
    }
    let mut days = i32::MIN;
    loop {
        assert_eq!(render_date32(days), prior_shape::render_date32(days), "{days}");
        match days.checked_add(100_003) {
            Some(next) => days = next,
            None => break,
        }
    }
    for days in [i32::MIN, i32::MAX, 0, -1, 1, 719_468] {
        assert_eq!(render_date32(days), prior_shape::render_date32(days), "{days}");
    }

    // Every fractional width the trim can stop at, against a stride over
    // the whole day — the `.5`/`.000001`/no-fraction branches.
    let fractions = [0i64, 1, 10, 100, 1_000, 10_000, 100_000, 500_000, 120_000, 123_456, 999_999];
    for second in (0..86_401i64).step_by(97) {
        for frac in fractions {
            let v = second * 1_000_000 + frac;
            assert_eq!(render_time64_micros(v), prior_shape::render_time64_micros(v), "{v}");
        }
    }
    for v in [0i64, -1, 1, 86_400_000_000, 360_000_000_000, -360_000_000_000, i64::MIN, i64::MAX] {
        assert_eq!(render_time64_micros(v), prior_shape::render_time64_micros(v), "{v}");
    }

    // Timestamps in the range a dump actually holds, at both spellings,
    // then random `i64`s — which reach BC years, six-digit years and the
    // negative time-of-day the `rem_euclid` cannot produce but
    // `render_time64_micros` can.
    for day in (-800_000..=800_000i64).step_by(4_001) {
        for frac in fractions {
            for with_tz in [false, true] {
                let v = day * 86_400_000_000 + frac;
                assert_eq!(
                    render_timestamp_micros(v, with_tz),
                    prior_shape::render_timestamp_micros(v, with_tz),
                    "{v} {with_tz}"
                );
            }
        }
    }
    let mut rng = Rng(7);
    for _ in 0..20_000 {
        let v = rng.next() as i64;
        for with_tz in [false, true] {
            assert_eq!(
                render_timestamp_micros(v, with_tz),
                prior_shape::render_timestamp_micros(v, with_tz),
                "{v} {with_tz}"
            );
            assert_eq!(render_time64_micros(v), prior_shape::render_time64_micros(v), "{v}");
        }
    }
    for v in [i64::MIN, i64::MAX, 0, -1, 1, -86_400_000_000] {
        for with_tz in [false, true] {
            assert_eq!(
                render_timestamp_micros(v, with_tz),
                prior_shape::render_timestamp_micros(v, with_tz),
                "{v} {with_tz}"
            );
        }
    }
}

/// [`push_integer`] is asked to be `i64::to_string`, so it is checked
/// against it: every value of a two-digit and three-digit width, where the
/// pair loop's odd/even tail lives, both ends of every integer type a
/// column can resolve to, and a random sweep of the whole `i64` range.
#[test]
fn integer_render_agrees_with_to_string() {
    let check = |v: i64| {
        let mut out = String::from("|");
        push_integer(&mut out, v);
        assert_eq!(out, format!("|{v}"), "{v}");
    };
    for v in -1_000..=1_000i64 {
        check(v);
    }
    for v in [
        0,
        i64::from(i16::MIN),
        i64::from(i16::MAX),
        i64::from(i32::MIN),
        i64::from(i32::MAX),
        i64::from(u32::MAX),
        i64::MIN,
        i64::MAX,
        -9,
        -10,
        -99,
        -100,
        9,
        10,
        99,
        100,
        999,
        1_000,
    ] {
        check(v);
    }
    let mut rng = Rng(9);
    for _ in 0..50_000 {
        let v = rng.next() as i64;
        check(v);
        check(v % 1_000_000);
        check(v >> 32);
    }
}

/// The `_into` forms are what the owned ones call, so the property worth
/// asserting is the one that would break if that ever stopped being true:
/// appending to a buffer that already holds text leaves what was there
/// alone and appends exactly the owned form.
#[test]
fn the_sink_forms_append_exactly_what_the_owned_forms_return() {
    let mut out = String::from("head\t");
    render_date32_into(19_889, &mut out);
    out.push('\t');
    render_time64_micros_into(49_530_123_456, &mut out);
    out.push('\t');
    render_timestamp_micros_into(1_718_000_000_000_000, false, &mut out);
    out.push('\t');
    render_timestamp_micros_into(1_718_000_000_000_000, true, &mut out);
    assert_eq!(
        out,
        format!(
            "head\t{}\t{}\t{}\t{}",
            render_date32(19_889),
            render_time64_micros(49_530_123_456),
            render_timestamp_micros(1_718_000_000_000_000, false),
            render_timestamp_micros(1_718_000_000_000_000, true),
        )
    );
    // A trimmed fraction truncates back to its own last non-zero digit
    // and never into the caller's text, even when that text ends in one.
    let mut zero_tailed = String::from("00");
    render_time64_micros_into(500_000, &mut zero_tailed);
    assert_eq!(zero_tailed, "0000:00:00.5");
}

#[test]
fn numeric_agrees_with_the_shape_it_replaced() {
    for scale in [-3i8, -1, 0, 1, 2, 6, 10] {
        let check = |s: &str| {
            assert_eq!(
                decimal_unscaled_digits(s, scale),
                prior_shape::decimal_unscaled_digits(s, scale),
                "{s:?} at scale {scale}"
            );
        };
        fuzz(u64::from(scale.unsigned_abs()) + 4, 16, check);
        shaped(
            u64::from(scale.unsigned_abs()) + 44,
            |rng| {
                // Leading zeros, all-zero values and both signs, since
                // the trim, the collapse to `"0"` and the sign are the
                // three places the two forms could disagree.
                let mut s = String::new();
                if rng.below(3) == 0 {
                    s.push('-');
                }
                let int = rng.below(6);
                let frac = rng.below(12);
                if rng.below(4) == 0 {
                    s.push_str(&"0".repeat(int + frac));
                    if frac > 0 {
                        s.insert(s.len() - frac, '.');
                    }
                    return s;
                }
                s.push_str(&digits(rng, int));
                if frac > 0 || rng.below(2) == 0 {
                    s.push('.');
                    s.push_str(&digits(rng, frac));
                }
                s
            },
            check,
        );
    }
}
