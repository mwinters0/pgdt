//! What more than one statistics module's tests build: a seeded generator,
//! values drawn to collide and the type a kind's column is read as, and the
//! density rule's reference model, the ladder of sizes
//! `scripts/row_density.py` walks.

use arrow_schema::DataType;
use pgdt_codec::compare::CompareKind;
use pgdt_codec::decode::{render_bytea, render_bytea_escape};
use pgdt_model::DICTIONARY_ENTRY_MAX_BYTES;

use super::density::{max_rows_group, min_rows_group};

/// SplitMix64, seeded, so a failing case reproduces.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    pub(super) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A value of `kind` drawn to collide: short and long, sharing long
/// prefixes, padded, and — for text — past the ASCII range.
pub(super) fn value(rng: &mut Rng, kind: &CompareKind) -> String {
    let length = match rng.below(4) {
        0 => rng.below(4) as usize,
        1 => DICTIONARY_ENTRY_MAX_BYTES - 2 + rng.below(8) as usize,
        _ => 250 + rng.below(40) as usize,
    };
    let alphabet: &[char] = &['a', 'b', ' ', 'é', '\u{10FFFF}', '\u{1f}'];
    match kind {
        CompareKind::Int { .. } => (rng.below(20) as i64 - 10).to_string(),
        // Finite dates either side of the calendar's end, and the
        // infinities, which no format holds.
        CompareKind::Date => match rng.below(10) {
            0 => "infinity".into(),
            1 => "-infinity".into(),
            2 => format!("{}-01-0{}", 262142 + rng.below(3), 1 + rng.below(9)),
            _ => format!("20{:02}-0{}-1{}", rng.below(30), 1 + rng.below(9), rng.below(10)),
        },
        // As a date, and past `i64` microseconds from 1970 too, which
        // keys from PostgreSQL's epoch (I49).
        CompareKind::Timestamp { .. } => match rng.below(12) {
            0 => "infinity".into(),
            1 => "-infinity".into(),
            2 => format!("262143-01-0{} 00:00:00", 1 + rng.below(9)),
            3 => format!("29427{}-12-31 23:59:59", 1 + rng.below(5)),
            _ => format!("20{:02}-01-01 0{}:00:00", rng.below(30), rng.below(10)),
        },
        CompareKind::Bytea => {
            let bytes: Vec<u8> =
                (0..length / 2).map(|_| [0x00, 0x7f, 0xfe, 0xff][rng.below(4) as usize]).collect();
            render_bytea(&bytes)
        }
        _ => {
            let mut text = String::new();
            while text.len() < length {
                text.push(alphabet[rng.below(alphabet.len() as u64) as usize]);
            }
            text
        }
    }
}

/// A value of `kind` to gather off a long field's bytes
/// (`docs/design/decisions.md`, "D142"): short ones, and long ones sharing a
/// long head, past the ASCII range, around a typmod of 1500 characters, and
/// long only by trailing blanks; a `bytea` in either form, lowercase hex or
/// not, refused or not; an integer long only by its leading zeros.
pub(super) fn long_value(rng: &mut Rng, kind: &CompareKind) -> String {
    let bytes = |rng: &mut Rng| -> Vec<u8> {
        (0..rng.below(3000))
            .map(|_| [0x00, 0x41, 0x5c, 0x7f, 0xfe][rng.below(5) as usize])
            .collect()
    };
    match kind {
        CompareKind::Bytea => match rng.below(6) {
            0 | 1 => render_bytea(&bytes(rng)),
            2 => format!("\\x{}", render_bytea(&bytes(rng))[2..].to_uppercase()),
            3 => render_bytea_escape(&bytes(rng)),
            4 => format!("{}\\9", render_bytea_escape(&bytes(rng))),
            _ => format!("{}0", render_bytea(&bytes(rng))),
        },
        CompareKind::Int { .. } => {
            format!("{}{}", "0".repeat(rng.below(2000) as usize), rng.below(100))
        }
        _ => {
            let repeat = |text: &str, base: u64, spread: u64, rng: &mut Rng| {
                text.repeat((base + rng.below(spread)) as usize)
            };
            match rng.below(7) {
                0 => format!("v{}", rng.below(40)),
                1 => repeat("\u{e9}", 400, 800, rng) + &repeat("x", 0, 600, rng),
                2 => format!("{}{}", "q".repeat(1000), rng.below(1000)),
                3 => format!("w{}{}", rng.below(9), repeat(" ", 300, 1500, rng)),
                4 => repeat("b", 1490, 20, rng) + &repeat(" ", 0, 300, rng),
                5 => repeat("\u{10FFFF}", 300, 200, rng) + &repeat(" ", 0, 3, rng),
                _ => repeat("z", 200, 3000, rng),
            }
        }
    }
}

/// `text` as `COPY` writes it, every fifth character but one past ASCII
/// spelled as an octal escape, so a field of any length is escaped.
pub(super) fn copy_escaped(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() * 2);
    for (i, c) in text.chars().enumerate() {
        match c {
            '\\' => out.extend_from_slice(b"\\\\"),
            c if c.is_ascii() && i % 5 == 0 => {
                out.extend_from_slice(format!("\\{:03o}", c as u32).as_bytes());
            }
            c => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
        }
    }
    out
}

/// The type the typed read emits a column compared by `kind` as.
pub(super) fn join_type(kind: &CompareKind) -> DataType {
    match kind {
        CompareKind::Int { .. } => DataType::Int64,
        CompareKind::Bytea => DataType::Binary,
        CompareKind::Date => DataType::Date32,
        CompareKind::Timestamp { .. } => {
            DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None)
        }
        _ => DataType::Utf8View,
    }
}

/// The size [`density_merges`](super::density::density_merges) must choose, written the way
/// `scripts/row_density.py` chooses it: every size from the gathered one
/// to a single group, and the first whose upper middle group holds the
/// minimum, else the single group.
pub(super) fn chosen_merges(rows: &[u64], min_rows: u64) -> u32 {
    if rows.is_empty() {
        return 0;
    }
    let mut ladder = vec![rows.to_vec()];
    while ladder.last().unwrap().len() > 1 {
        let last = ladder.last().unwrap();
        let coarser =
            (0..last.len()).step_by(2).map(|i| last[i..last.len().min(i + 2)].iter().sum());
        ladder.push(coarser.collect());
    }
    let median = |level: &Vec<u64>| {
        let mut sorted = level.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    };
    let reaches = ladder.iter().position(|level| median(level) >= min_rows);
    reaches.unwrap_or(ladder.len() - 1) as u32
}

/// The size [`density_merges`](super::density::density_merges) must choose under a maximum as well as a
/// minimum: the ladder walked as [`chosen_merges`] walks it, stopping at
/// the last size whose 90th-percentile group is within the maximum.
pub(super) fn chosen_merges_between(rows: &[u64], min_rows: u64, max_rows: u64) -> u32 {
    let mut level = rows.to_vec();
    let mut merges = 0;
    while level.len() > 1 && min_rows_group(&level) < min_rows {
        let coarser: Vec<u64> = level.chunks(2).map(|pair| pair.iter().sum()).collect();
        if max_rows_group(&coarser) > max_rows {
            break;
        }
        (level, merges) = (coarser, merges + 1);
    }
    merges
}
