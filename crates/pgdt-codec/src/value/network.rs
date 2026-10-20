//! Whether `inet`, `cidr`, `macaddr` and `macaddr8` input reads a text: ports
//! of `inet_net_pton.c` and the `macaddr` scanners, read or refused. Not
//! `decode`'s API; a strict parse and a filter's literal call them. And an
//! `inet` or `cidr` value's order key ([`NetworkKey`]), and a `macaddr`'s
//! octets as `macaddr_out` writes them, read and rendered.

use std::cmp::Ordering;

use crate::decode::Unread;
use crate::value::digits::HEX_NIBBLE;
use crate::value::libc::is_c_space;

/// An `inet` or `cidr` value as `network_in` reads it: its family (`true`
/// for IPv6), its netmask's bits, and its address left-aligned in sixteen
/// bytes. `None` where the server refuses the text.
///
/// The family is IPv6 wherever the text holds a `:`. An IPv4 `inet` is
/// dotted decimal, `0` to `255` an octet, leading zeros and a trailing dot
/// read, and a netmask past the octets written only where they reach it
/// (`10.1.2/24`); a `cidr` reads an abbreviated or hex network besides,
/// with a classful netmask where none is written (`10` is `10.0.0.0/8`).
/// An IPv6 value of either takes an IPv4 tail and a netmask written with no
/// leading zero. A `cidr` refuses a bit set below its netmask.
///
/// A port of `inet_net_pton.c`, its quirks included: a netmask is
/// accumulated in a wrapping `int`, as `-fwrapv` builds the server
/// (`1.2.3.4/4294967304` is a `/8`), and an IPv4 tail may be short or hold
/// an empty octet (`::1..2.3` is `::1.0.2.3`).
// pg-refuses: I67 — every refusal here is `network_in`'s.
pub(crate) fn network_in(text: &str, cidr: bool) -> Option<(bool, u8, [u8; 16])> {
    let b = text.as_bytes();
    let v6 = b.contains(&b':');
    let mut addr = [0u8; 16];
    let bits = if v6 {
        inet_pton_ipv6(b, &mut addr)?
    } else if cidr {
        cidr_pton_ipv4(b, &mut addr)?
    } else {
        inet_pton_ipv4(b, &mut addr)?
    };
    let maxbits = if v6 { 128 } else { 32 };
    let bits = u8::try_from(bits).ok().filter(|&bits| bits <= maxbits)?;
    // `addressOK`: no bit set below the netmask.
    if cidr && (bits..maxbits).any(|bit| addr[usize::from(bit / 8)] & (0x80 >> (bit % 8)) != 0) {
        return None;
    }
    Some((v6, bits, addr))
}

/// The byte at `at`, `0` past the end, as a C string reads its terminator.
fn c_byte(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

/// A netmask's digits from `*at`, the first already known to be one,
/// accumulated as the server's `int` accumulates them, wrapping.
fn wrapping_bits(b: &[u8], at: &mut usize) -> i32 {
    let mut bits = 0i32;
    while c_byte(b, *at).is_ascii_digit() {
        bits = bits.wrapping_mul(10).wrapping_add(i32::from(b[*at] - b'0'));
        *at += 1;
    }
    bits
}

/// `inet_net_pton_ipv4`: an `inet`'s IPv4 address, `None` on `ENOENT` or
/// `EMSGSIZE`.
fn inet_pton_ipv4(b: &[u8], dst: &mut [u8; 16]) -> Option<i32> {
    let mut at = 0;
    let mut len = 0;
    let mut ch = c_byte(b, at);
    while ch.is_ascii_digit() {
        let mut octet = 0u32;
        while ch.is_ascii_digit() {
            octet = octet * 10 + u32::from(ch - b'0');
            if octet > 255 {
                return None;
            }
            at += 1;
            ch = c_byte(b, at);
        }
        if len == 4 {
            return None;
        }
        dst[len] = octet as u8;
        len += 1;
        if ch == 0 || ch == b'/' {
            break;
        }
        if ch != b'.' {
            return None;
        }
        at += 1;
        ch = c_byte(b, at);
    }
    let mut bits = -1;
    if ch == b'/' && c_byte(b, at + 1).is_ascii_digit() && len > 0 {
        at += 1;
        bits = wrapping_bits(b, &mut at);
        ch = c_byte(b, at);
        if ch != 0 || bits > 32 {
            return None;
        }
    }
    if ch != 0 || len == 0 {
        return None;
    }
    if bits == -1 {
        if len != 4 {
            return None;
        }
        bits = 32;
    }
    // A netmask may not reach past the octets written; C's division
    // truncates a wrapped negative one toward zero, so it passes here.
    if bits / 8 > len as i32 {
        return None;
    }
    Some(bits)
}

/// `inet_cidr_pton_ipv4` at four bytes: a `cidr`'s IPv4 network, `None` on
/// `ENOENT` or `EMSGSIZE`.
fn cidr_pton_ipv4(b: &[u8], dst: &mut [u8; 16]) -> Option<i32> {
    let mut len = 0;
    let mut at = 1;
    let mut ch = c_byte(b, 0);
    if ch == b'0' && matches!(c_byte(b, 1), b'x' | b'X') && c_byte(b, 2).is_ascii_hexdigit() {
        // A string of nibbles, an odd last one the high half of its byte.
        at = 2;
        let mut nibbles = 0;
        let mut byte = 0u8;
        loop {
            ch = c_byte(b, at);
            at += 1;
            if !ch.is_ascii_hexdigit() {
                break;
            }
            let nibble = HEX_NIBBLE[usize::from(ch)];
            byte = if nibbles == 0 { nibble } else { byte << 4 | nibble };
            nibbles += 1;
            if nibbles == 2 {
                push_octet(dst, &mut len, byte)?;
                nibbles = 0;
            }
        }
        if nibbles == 1 {
            push_octet(dst, &mut len, byte << 4)?;
        }
    } else if ch.is_ascii_digit() {
        loop {
            let mut octet = 0u32;
            loop {
                octet = octet * 10 + u32::from(ch - b'0');
                if octet > 255 {
                    return None;
                }
                ch = c_byte(b, at);
                at += 1;
                if !ch.is_ascii_digit() {
                    break;
                }
            }
            push_octet(dst, &mut len, octet as u8)?;
            if ch == 0 || ch == b'/' {
                break;
            }
            if ch != b'.' {
                return None;
            }
            ch = c_byte(b, at);
            at += 1;
            if !ch.is_ascii_digit() {
                return None;
            }
        }
    } else {
        return None;
    }
    // `at` is one past `ch`, as the C's `src` is.
    let mut bits = -1;
    if ch == b'/' && c_byte(b, at).is_ascii_digit() && len > 0 {
        bits = wrapping_bits(b, &mut at);
        if c_byte(b, at) != 0 {
            return None;
        }
        if bits > 32 {
            return None;
        }
        ch = 0;
    }
    if ch != 0 || len == 0 {
        return None;
    }
    if bits == -1 {
        // The class's netmask, widened to the octets written.
        bits = match dst[0] {
            240.. => 32,
            224.. => 8,
            192.. => 24,
            128.. => 16,
            _ => 8,
        };
        bits = bits.max(len as i32 * 8);
        if bits == 8 && dst[0] == 224 {
            bits = 4;
        }
    }
    // The network extended with zero bytes to cover its netmask, which
    // four bytes must hold.
    while bits > len as i32 * 8 {
        push_octet(dst, &mut len, 0)?;
    }
    Some(bits)
}

/// One more of an IPv4 network's four bytes, `None` past the fourth
/// (`EMSGSIZE`).
fn push_octet(dst: &mut [u8; 16], len: &mut usize, byte: u8) -> Option<()> {
    if *len == 4 {
        return None;
    }
    dst[*len] = byte;
    *len += 1;
    Some(())
}

/// `inet_cidr_pton_ipv6` at sixteen bytes, which `inet` and `cidr` share:
/// `None` on `ENOENT`.
fn inet_pton_ipv6(b: &[u8], dst: &mut [u8; 16]) -> Option<i32> {
    let mut tmp = [0u8; 16];
    let mut tp = 0;
    let mut colonp = None;
    let mut at = 0;
    if c_byte(b, 0) == b':' {
        if c_byte(b, 1) != b':' {
            return None;
        }
        at = 1;
    }
    let mut curtok = at;
    let mut saw_xdigit = false;
    let mut val = 0u32;
    let mut digits = 0;
    let mut bits = -1;
    loop {
        let ch = c_byte(b, at);
        at += 1;
        if ch == 0 {
            break;
        }
        if ch.is_ascii_hexdigit() {
            val = val << 4 | u32::from(HEX_NIBBLE[usize::from(ch)]);
            digits += 1;
            if digits > 4 {
                return None;
            }
            saw_xdigit = true;
            continue;
        }
        if ch == b':' {
            curtok = at;
            if !saw_xdigit {
                if colonp.is_some() {
                    return None;
                }
                colonp = Some(tp);
                continue;
            }
            if c_byte(b, at) == 0 || tp + 2 > 16 {
                return None;
            }
            tmp[tp] = (val >> 8) as u8;
            tmp[tp + 1] = val as u8;
            tp += 2;
            saw_xdigit = false;
            digits = 0;
            val = 0;
            continue;
        }
        if ch == b'.' && tp + 4 <= 16 && getv4(&b[curtok..], &mut tmp[tp..tp + 4], &mut bits) {
            tp += 4;
            saw_xdigit = false;
            break;
        }
        if ch == b'/' && getbits(&b[at..], &mut bits) {
            break;
        }
        return None;
    }
    if saw_xdigit {
        if tp + 2 > 16 {
            return None;
        }
        tmp[tp] = (val >> 8) as u8;
        tmp[tp + 1] = val as u8;
        tp += 2;
    }
    if bits == -1 {
        bits = 128;
    }
    if let Some(colon) = colonp {
        // The groups after `::` moved to the end, zeros in their place.
        if tp == 16 {
            return None;
        }
        let n = tp - colon;
        for i in 1..=n {
            tmp[16 - i] = tmp[colon + n - i];
            tmp[colon + n - i] = 0;
        }
        tp = 16;
    }
    if tp != 16 {
        return None;
    }
    *dst = tmp;
    Some(bits)
}

/// `getbits`: an IPv6 netmask, `0` to `128` with no leading zero.
fn getbits(b: &[u8], bits: &mut i32) -> bool {
    let mut val = 0;
    let mut n = 0;
    for &ch in b {
        if !ch.is_ascii_digit() {
            return false;
        }
        if n != 0 && val == 0 {
            return false;
        }
        n += 1;
        val = val * 10 + i32::from(ch - b'0');
        if val > 128 {
            return false;
        }
    }
    if n == 0 {
        return false;
    }
    *bits = val;
    true
}

/// `getv4`: an IPv6 value's IPv4 tail into the four bytes `dst`, octets with
/// no leading zero, at most four, and a netmask after it by [`getbits`].
fn getv4(b: &[u8], dst: &mut [u8], bits: &mut i32) -> bool {
    let mut val = 0u32;
    let mut n = 0;
    let mut len = 0;
    for (i, &ch) in b.iter().enumerate() {
        if ch.is_ascii_digit() {
            if n != 0 && val == 0 {
                return false;
            }
            n += 1;
            val = val * 10 + u32::from(ch - b'0');
            if val > 255 {
                return false;
            }
            continue;
        }
        if ch == b'.' || ch == b'/' {
            if len > 3 {
                return false;
            }
            dst[len] = val as u8;
            len += 1;
            if ch == b'/' {
                return getbits(&b[i + 1..], bits);
            }
            val = 0;
            n = 0;
            continue;
        }
        return false;
    }
    if n == 0 || len > 3 {
        return false;
    }
    dst[len] = val as u8;
    true
}

/// A `macaddr`/`macaddr8` value's octets: `octets` hex pairs of either case
/// joined by colons, `macaddr_out` and `macaddr8_out` writing lowercase (I40).
/// The server's input function takes other separator conventions and this
/// takes none of them (`docs/design/decisions.md`, "D55").
pub(crate) fn macaddr_octets(text: &str, octets: usize) -> Option<Vec<u8>> {
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
    (bytes.len() == octets).then_some(bytes)
}

/// A `macaddr`/`macaddr8` literal in `macaddr_out`'s own spelling: `octets`
/// lowercase hex pairs joined by colons (I40). Those two rules are the whole
/// of that output function, which is why this type canonicalizes where the
/// two beside it decode per row — see [`literal_equality`](crate::key::equality::literal_equality).
pub fn render_macaddr(text: &str, octets: usize) -> Option<String> {
    let bytes = macaddr_octets(text, octets)?;
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"))
}

/// Why a `macaddr` this build does not read is no value: `macaddr_in`
/// refuses it, or reads it — six octets in one of its seven `sscanf`
/// layouts, separated by `:` or `-`, grouped by `.` or `-` in fours or `:`
/// or `-` in sixes, or not at all, a digit run as short as one and blanks
/// around them — where this build reads `macaddr_out`'s colon-separated
/// pairs (`docs/design/decisions.md`, "D55").
///
/// `%x` is modelled for the hex digits it takes and the blanks it skips. An
/// octet past eight significant digits, which glibc truncates, is left
/// unvalued.
// deficiency: KD85 — a run opening with a sign or a `0x` is read by glibc's
// `%x`, which this does not model, so a text in which any layout tried meets
// one is counted read and never refused, `08:00:2b:01:02:-1` and
// `0x0800.2b01.0203` among those the server refuses (I68) — and so is a
// malformed dash-separated one, `222:222` meeting its `-` where `%2x` reads.
pub(crate) fn macaddr_unread(s: &str) -> Unread {
    // `x` is `%x`, `2` is `%2x`, anything else a byte the text must hold.
    const LAYOUTS: [&[u8]; 7] = [
        b"x:x:x:x:x:x",
        b"x-x-x-x-x-x",
        b"222:222",
        b"222-222",
        b"22.22.22",
        b"22-22-22",
        b"222222",
    ];
    for layout in LAYOUTS {
        match macaddr_scan(s.as_bytes(), layout) {
            MacScan::Unmodelled => return Unread::Unparsed,
            // The first layout taking six octets decides.
            // pg-refuses: I68 — an octet past 255.
            MacScan::Six(octets) => {
                return if octets.iter().all(|o| o.is_none_or(|o| o <= 255)) {
                    Unread::Unparsed
                } else {
                    Unread::Refused
                };
            }
            MacScan::Short => {}
        }
    }
    // pg-refuses: I68 — no layout takes six octets.
    Unread::Refused
}

/// What one `sscanf` layout of [`macaddr_unread`] makes of a text.
enum MacScan {
    /// Six octets and nothing after them but blanks, `None` for a run past
    /// eight significant digits, which glibc truncates to an `unsigned int`.
    Six([Option<u32>; 6]),
    /// Fewer than six, or a seventh field after them.
    Short,
    /// A sign or a `0x`, which glibc reads by rules the model does not
    /// follow.
    Unmodelled,
}

fn macaddr_scan(b: &[u8], layout: &[u8]) -> MacScan {
    let mut octets = [None; 6];
    let mut count = 0;
    let mut at = 0;
    for &field in layout {
        if !matches!(field, b'x' | b'2') {
            if b.get(at) != Some(&field) {
                return MacScan::Short;
            }
            at += 1;
            continue;
        }
        at += b[at..].iter().take_while(|&&c| is_c_space(c)).count();
        if let Some([b'+' | b'-', ..] | [b'0', b'x' | b'X', ..]) = b.get(at..) {
            return MacScan::Unmodelled;
        }
        let width = if field == b'2' { 2 } else { usize::MAX };
        let run = b[at..].iter().take(width).take_while(|c| c.is_ascii_hexdigit()).count();
        if run == 0 {
            return MacScan::Short;
        }
        let digits = &b[at..at + run];
        at += run;
        let significant = &digits[digits.iter().take_while(|&&c| c == b'0').count()..];
        octets[count] = (significant.len() <= 8).then(|| {
            significant
                .iter()
                .fold(0, |value, &c| value << 4 | u32::from(HEX_NIBBLE[usize::from(c)]))
        });
        count += 1;
    }
    // `%1s`: a seventh field wherever anything but blanks follows.
    if b[at..].iter().all(|&c| is_c_space(c)) { MacScan::Six(octets) } else { MacScan::Short }
}

/// `macaddr8_in`: the eight bytes the server reads, a six-byte address
/// widened by `FF:FE` in its middle, `None` where it refuses the text. Hex
/// pairs, each separated from the next by `:`, `-` or `.` — one of them
/// throughout — or by nothing, blanks around the whole, and its quirks: a
/// separator after the last pair, and a single stray character after six or
/// eight pairs, are read.
// pg-refuses: I68 — every refusal here is `macaddr8_in`'s.
pub(crate) fn macaddr8_in(s: &str) -> Option<[u8; 8]> {
    let b = s.as_bytes();
    let mut at = b.iter().take_while(|&&c| is_c_space(c)).count();
    let mut bytes = [0u8; 8];
    let mut count = 0;
    let mut spacer = None;
    while at + 1 < b.len() {
        if count == 8 {
            return None;
        }
        let (hi, lo) = (HEX_NIBBLE[usize::from(b[at])], HEX_NIBBLE[usize::from(b[at + 1])]);
        if (hi | lo) & 0xF0 != 0 {
            return None;
        }
        bytes[count] = hi << 4 | lo;
        count += 1;
        at += 2;
        if let Some(&c @ (b':' | b'-' | b'.')) = b.get(at) {
            if *spacer.get_or_insert(c) != c {
                return None;
            }
            at += 1;
        }
        if (count == 6 || count == 8) && b.get(at).is_some_and(|&c| is_c_space(c)) {
            if !b[at..].iter().all(|&c| is_c_space(c)) {
                return None;
            }
            at = b.len();
        }
    }
    match count {
        8 => Some(bytes),
        6 => Some([bytes[0], bytes[1], bytes[2], 0xFF, 0xFE, bytes[3], bytes[4], bytes[5]]),
        _ => None,
    }
}

/// One side of an `inet`/`cidr` comparison: the family, the netmask length,
/// and the address left-aligned in sixteen bytes.
///
/// Not a byte key, and it cannot be made into one: `network_cmp_internal`
/// compares the *shorter* netmask's worth of address bits first (I40), so how
/// many bits are significant depends on the value being compared against.
/// The pair is compared, not two independently sortable keys.
///
/// `v6` is the family; `PGSQL_AF_INET6` is `PGSQL_AF_INET + 1`, so an IPv4
/// address sorts below every IPv6 one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkKey {
    pub(crate) v6: bool,
    pub(crate) bits: u8,
    /// The address, left-aligned: an IPv4 address occupies the first four
    /// bytes and the rest are zero, which is what `bitncmp` reads, never
    /// looking past `maxbits`.
    pub(crate) addr: [u8; 16],
}

impl NetworkKey {
    /// `ip_maxbits` — the family's address width.
    fn maxbits(&self) -> u8 {
        if self.v6 { 128 } else { 32 }
    }

    /// `network_cmp_internal` (I40): family, then the shorter netmask's
    /// worth of address bits, then the netmask length, then the whole
    /// address.
    pub(crate) fn cmp(&self, other: &Self) -> Ordering {
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
/// significant first. Whole bytes by `memcmp`, then the straddling byte's
/// remaining bits under a high-bit mask — the same answer as `bitncmp`'s
/// bit-at-a-time loop, which stops at the first differing bit.
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

#[cfg(test)]
mod tests;
