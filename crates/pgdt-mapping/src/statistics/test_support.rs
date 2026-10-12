//! What more than one statistics module's tests build: a seeded generator,
//! values drawn to collide and the type a kind's column is read as, and the
//! density rule's reference model, the ladder of sizes
//! `scripts/row_density.py` walks.

use arrow_schema::DataType;
use pgdt_codec::compare::CompareKind;
use pgdt_codec::decode::render_bytea;
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
