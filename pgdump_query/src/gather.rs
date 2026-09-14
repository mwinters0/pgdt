//! The statistics gatherer: [`crate::statistics::BlockObserver`] implemented
//! at L4, where a column's comparison is known
//! (`docs/design/decisions.md`, "D74").
//!
//! [`observer_for`] builds one per `COPY` block from the block's resolved
//! schema. Every tracked column counts its NULLs per group; a column its
//! comparison orders exactly — a `Compared` plan with no divergence — also
//! keeps per-group bounds and the block's row order, under the key a filter
//! orders by ([`ValueKey`]); and a column its comparison equates exactly keeps
//! a dictionary per group.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::copy::{CopyHeader, decode_field, split_fields};
use crate::decode::{decode_bytea, render_bytea};
use crate::pgtype::{CompareKind, ComparisonPlan, NestedPlan};
use crate::preamble::DumpMetadata;
use crate::predicate::ValueKey;
use crate::resolve::{SchemaMode, resolve_columns};
use crate::statistics::{
    BlockObserver, BlockStatistics, Bounds, ColumnBounds, ColumnDictionary, ColumnStatistics,
    DICTIONARY_CAP, RowGroup, STORED_VALUE_CAP, Sortedness, StatisticsRequest,
};

/// The observer for one block, or `None` when `request` tracks nothing in it.
///
/// Resolved against an empty census: the census moves only an array column,
/// and a nested column gets neither bounds nor a dictionary.
pub(crate) fn observer_for(
    request: &StatisticsRequest,
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Option<Box<dyn BlockObserver>> {
    let tracked = request.tracked_columns(header)?;
    let qualified = header.qualified_name();
    let resolved =
        resolve_columns(&qualified, &header.columns, metadata, database, SchemaMode::Typed, &[]);
    let declared = metadata
        .and_then(|m| m.databases.iter().find(|db| db.name.as_deref() == database))
        .and_then(|db| db.tables.get(&qualified));
    let columns: Vec<Option<ColumnGatherer>> = header
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            tracked[i].then(|| {
                let def = declared.and_then(|cols| cols.iter().find(|c| &c.name == name));
                ColumnGatherer::new(
                    def.map(|d| d.declared_type.clone()),
                    def.and_then(|d| d.collation.clone()),
                    &resolved.comparisons[i],
                    &resolved.plans[i],
                )
            })
        })
        .collect();
    Some(Box::new(Gatherer {
        group_size: request.group_size(),
        groups: Vec::new(),
        open: None,
        splits: columns.iter().any(Option::is_some),
        columns,
    }))
}

/// The group a row is being added to.
struct OpenGroup {
    index: u64,
    first_start: u64,
    rows: u64,
}

struct Gatherer {
    group_size: u64,
    groups: Vec<RowGroup>,
    open: Option<OpenGroup>,
    /// Whether any column is tracked, so a row is worth splitting.
    splits: bool,
    columns: Vec<Option<ColumnGatherer>>,
}

impl Gatherer {
    /// Close the open group, its last row's line ending at `end`, and list an
    /// empty group for every index short of `next`.
    fn close_through(&mut self, end: u64, next: u64) {
        let first = match self.open.take() {
            Some(group) => {
                self.groups.push(RowGroup { rows: group.rows, bytes: end - group.first_start });
                for column in self.columns.iter_mut().flatten() {
                    column.close_group();
                }
                group.index + 1
            }
            None => 0,
        };
        for _ in first..next {
            self.groups.push(RowGroup { rows: 0, bytes: 0 });
            for column in self.columns.iter_mut().flatten() {
                column.close_group();
            }
        }
    }
}

impl BlockObserver for Gatherer {
    fn observe_row(&mut self, offset: u64, raw: &[u8]) {
        let index = offset / self.group_size;
        match &mut self.open {
            Some(group) if group.index == index => group.rows += 1,
            _ => {
                self.close_through(offset, index);
                self.open = Some(OpenGroup { index, first_start: offset, rows: 1 });
            }
        }
        if !self.splits {
            return;
        }
        for (field, column) in split_fields(raw).zip(self.columns.iter_mut()) {
            if let Some(column) = column {
                column.observe(field);
            }
        }
    }

    fn finish(mut self: Box<Self>, end: u64) -> BlockStatistics {
        if let Some(group) = &self.open {
            let next = group.index + 1;
            self.close_through(end, next);
        }
        BlockStatistics {
            group_size: self.group_size,
            groups: self.groups,
            columns: self.columns.into_iter().map(|c| c.map(ColumnGatherer::finish)).collect(),
        }
    }
}

struct ColumnGatherer {
    declared_type: Option<String>,
    collation: Option<String>,
    null_counts: Vec<u64>,
    group_nulls: u64,
    bounds: Option<BoundsGatherer>,
    dictionary: Option<DictionaryGatherer>,
}

impl ColumnGatherer {
    fn new(
        declared_type: Option<String>,
        collation: Option<String>,
        comparison: &ComparisonPlan,
        plan: &NestedPlan,
    ) -> Self {
        let (bounds, dictionary) = match comparison {
            ComparisonPlan::Compared { kind, divergence } if *plan == NestedPlan::Scalar => (
                divergence.is_none().then(|| BoundsGatherer::new(kind.clone())),
                divergence
                    .is_none_or(|d| !d.affects_equality())
                    .then(|| DictionaryGatherer::new(kind)),
            ),
            _ => (None, None),
        };
        Self {
            declared_type,
            collation,
            null_counts: Vec::new(),
            group_nulls: 0,
            bounds,
            dictionary,
        }
    }

    fn observe(&mut self, field: &[u8]) {
        match decode_field(field) {
            Ok(None) => self.group_nulls += 1,
            Ok(Some(text)) => {
                if let Some(bounds) = &mut self.bounds {
                    bounds.observe(&text);
                }
                if let Some(dictionary) = &mut self.dictionary {
                    dictionary.observe(&text);
                }
            }
            // Not text at all, so neither a key nor an entry: the group can
            // cover the row with neither, and the block's order is lost.
            Err(_) => {
                if let Some(bounds) = &mut self.bounds {
                    bounds.lose_value();
                }
                if let Some(dictionary) = &mut self.dictionary {
                    dictionary.group = None;
                }
            }
        }
    }

    fn close_group(&mut self) {
        self.null_counts.push(std::mem::take(&mut self.group_nulls));
        if let Some(bounds) = &mut self.bounds {
            bounds.close_group();
        }
        if let Some(dictionary) = &mut self.dictionary {
            dictionary.close_group();
        }
    }

    fn finish(self) -> ColumnStatistics {
        ColumnStatistics {
            declared_type: self.declared_type,
            collation: self.collation,
            null_counts: self.null_counts,
            bounds: self.bounds.map(BoundsGatherer::finish),
            dictionary: self.dictionary.map(DictionaryGatherer::finish),
        }
    }
}

/// How a column's values are ordered while gathering.
enum Order {
    /// By the text itself, bytewise, once put in [`Canonical`] form: never
    /// keyed, since a key copies the whole value and a value may be hundreds
    /// of megabytes, and a bound past [`STORED_VALUE_CAP`] is truncated.
    Bytewise(Canonical),
    /// By [`ValueKey`], the key a filter orders by. A value past
    /// [`STORED_VALUE_CAP`] leaves its group unbounded and its block unordered,
    /// no truncation of it being a bound.
    Keyed(CompareKind),
}

/// The bytewise kinds, each by the text whose bytes order as its key does.
#[derive(Clone, Copy)]
enum Canonical {
    /// `text`, `varchar`, `name` under a bytewise collation: the text.
    Text,
    /// `character`: the text without its trailing blanks, which its comparison
    /// ignores — so a bound is stored unpadded.
    PaddedText,
    /// `bytea`: `\x` and lowercase hex pairs, which order as the bytes do. A
    /// value in any other spelling is not placed.
    Bytea,
}

impl Canonical {
    /// The text whose bytes order `text` among its column's values, `None`
    /// where this kind cannot place it.
    fn of(self, text: &str) -> Option<&str> {
        match self {
            Self::Text => Some(text),
            Self::PaddedText => Some(text.trim_end_matches(' ')),
            Self::Bytea => text
                .strip_prefix("\\x")
                .is_some_and(|hex| {
                    hex.len() % 2 == 0
                        && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
                })
                .then_some(text),
        }
    }
}

/// How much of a bytewise value is kept: past the cap by one character's
/// width, so a kept head longer than the cap says its value is too.
const CLIP_BYTES: usize = STORED_VALUE_CAP + char::MAX.len_utf8();

/// A bytewise value's first [`CLIP_BYTES`], which is all a stored bound is
/// taken from.
struct Clipped {
    head: String,
    /// Whether `head` is the whole value.
    whole: bool,
}

impl Clipped {
    fn of(text: &str) -> Self {
        let head = text_prefix(text, CLIP_BYTES);
        Self { head: head.to_owned(), whole: head.len() == text.len() }
    }

    /// Where `text` orders against the value this was clipped from, `None`
    /// when `text` begins with a head that is not the whole value. Two such
    /// values share every byte a stored bound reads, so either stands for
    /// the other in the group's bounds; only their row order is unknown.
    fn locate(&self, text: &str) -> Option<Ordering> {
        let (head, text) = (self.head.as_bytes(), text.as_bytes());
        let shared = head.len().min(text.len());
        match text[..shared].cmp(&head[..shared]) {
            Ordering::Equal if text.len() > head.len() => self.whole.then_some(Ordering::Greater),
            Ordering::Equal if text.len() == head.len() && self.whole => Some(Ordering::Equal),
            Ordering::Equal => Some(Ordering::Less),
            unequal => Some(unequal),
        }
    }
}

struct BoundsGatherer {
    order: Order,
    groups: Vec<Option<Bounds>>,
    group: GroupBounds,
    never_decreased: bool,
    never_increased: bool,
    /// A value the block's order cannot place was seen.
    order_lost: bool,
}

/// One group's running bounds, and the previous value for the block's order.
enum GroupBounds {
    Bytewise {
        min: Option<Clipped>,
        max: Option<Clipped>,
        previous: Option<Clipped>,
        lost: bool,
    },
    Keyed {
        min: Option<(ValueKey, String)>,
        max: Option<(ValueKey, String)>,
        previous: Option<ValueKey>,
        lost: bool,
    },
}

impl BoundsGatherer {
    fn new(kind: CompareKind) -> Self {
        let order = match kind {
            CompareKind::Text => Order::Bytewise(Canonical::Text),
            CompareKind::PaddedText => Order::Bytewise(Canonical::PaddedText),
            CompareKind::Bytea => Order::Bytewise(Canonical::Bytea),
            kind => Order::Keyed(kind),
        };
        let group = match order {
            Order::Bytewise(_) => {
                GroupBounds::Bytewise { min: None, max: None, previous: None, lost: false }
            }
            Order::Keyed(_) => {
                GroupBounds::Keyed { min: None, max: None, previous: None, lost: false }
            }
        };
        Self {
            order,
            groups: Vec::new(),
            group,
            never_decreased: true,
            never_increased: true,
            order_lost: false,
        }
    }

    /// A value that does not key, or that no stored bound could cover: no
    /// bounds for its group, and no order for its block.
    fn lose_value(&mut self) {
        match &mut self.group {
            GroupBounds::Bytewise { lost, .. } | GroupBounds::Keyed { lost, .. } => *lost = true,
        }
        self.order_lost = true;
    }

    /// Fold one step of the block's row order: `step` is where the value
    /// orders against the previous one, `None` where that is unknown.
    fn step(&mut self, step: Option<Ordering>) {
        match step {
            Some(Ordering::Less) => self.never_decreased = false,
            Some(Ordering::Greater) => self.never_increased = false,
            Some(Ordering::Equal) => {}
            None => self.order_lost = true,
        }
    }

    fn observe(&mut self, text: &str) {
        match &self.order {
            Order::Bytewise(canonical) => {
                let Some(text) = canonical.of(text) else { return self.lose_value() };
                let GroupBounds::Bytewise { min, max, previous, .. } = &mut self.group else {
                    unreachable!("a bytewise order keeps bytewise bounds")
                };
                if min.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Less)) {
                    *min = Some(Clipped::of(text));
                }
                if max.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Greater)) {
                    *max = Some(Clipped::of(text));
                }
                let step = previous.as_ref().map(|p| p.locate(text));
                *previous = Some(Clipped::of(text));
                if let Some(step) = step {
                    self.step(step);
                }
            }
            Order::Keyed(kind) => {
                if text.len() > STORED_VALUE_CAP {
                    return self.lose_value();
                }
                let Some(key) = ValueKey::of(kind, text) else { return self.lose_value() };
                let GroupBounds::Keyed { min, max, previous, .. } = &mut self.group else {
                    unreachable!("a keyed order keeps keyed bounds")
                };
                if min.as_ref().is_none_or(|(m, _)| key.compare(m) == Ordering::Less) {
                    *min = Some((key.clone(), text.to_owned()));
                }
                if max.as_ref().is_none_or(|(m, _)| key.compare(m) == Ordering::Greater) {
                    *max = Some((key.clone(), text.to_owned()));
                }
                let step = previous.as_ref().map(|p| key.compare(p));
                *previous = Some(key);
                if let Some(step) = step {
                    self.step(Some(step));
                }
            }
        }
    }

    fn close_group(&mut self) {
        let bounds = match &mut self.group {
            GroupBounds::Bytewise { min, max, lost, .. } => {
                match (std::mem::take(lost), min.take(), max.take()) {
                    (false, Some(min), Some(max)) => {
                        let Order::Bytewise(canonical) = self.order else { unreachable!() };
                        clipped_bounds(canonical, &min, &max)
                    }
                    _ => None,
                }
            }
            GroupBounds::Keyed { min, max, lost, .. } => {
                match (std::mem::take(lost), min.take(), max.take()) {
                    (false, Some((_, min)), Some((_, max))) => {
                        Some(Bounds { min, max, max_exact: true })
                    }
                    _ => None,
                }
            }
        };
        self.groups.push(bounds);
    }

    fn finish(self) -> ColumnBounds {
        let sortedness = if self.order_lost {
            Sortedness::Unsorted
        } else if self.never_decreased {
            Sortedness::Ascending
        } else if self.never_increased {
            Sortedness::Descending
        } else {
            Sortedness::Unsorted
        };
        ColumnBounds { sortedness, groups: self.groups }
    }
}

/// A bytewise group's bounds as stored: each exact where its value fits the
/// cap, otherwise a prefix below and a successor above.
fn clipped_bounds(canonical: Canonical, min: &Clipped, max: &Clipped) -> Option<Bounds> {
    let fits = |c: &Clipped| c.whole && c.head.len() <= STORED_VALUE_CAP;
    let lower = if fits(min) {
        min.head.clone()
    } else {
        text_prefix(&min.head, STORED_VALUE_CAP).to_owned()
    };
    if fits(max) {
        return Some(Bounds { min: lower, max: max.head.clone(), max_exact: true });
    }
    let upper = match canonical {
        Canonical::Text => text_upper(&max.head, false)?,
        Canonical::PaddedText => text_upper(&max.head, true)?,
        Canonical::Bytea => bytea_upper(&max.head)?,
    };
    Some(Bounds { min: lower, max: upper, max_exact: false })
}

/// The longest prefix of `text` no longer than `cap` bytes that ends on a
/// character boundary — a lower bound on `text` bytewise.
fn text_prefix(text: &str, cap: usize) -> &str {
    let mut end = cap.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// A text no longer than [`STORED_VALUE_CAP`] ordering above `text` bytewise:
/// a prefix with its last character replaced by the next one. UTF-8 orders
/// bytewise as it orders code points, so the successor is above every text
/// the prefix begins. `padded` is `character`, whose comparison drops trailing
/// blanks first: the prefix gives its own up, and a successor that is a blank
/// is skipped. `None` when no character of the prefix has a successor.
fn text_upper(text: &str, padded: bool) -> Option<String> {
    // Room for a successor one byte longer than the character it replaces.
    let mut prefix: String =
        text_prefix(text, STORED_VALUE_CAP - char::MAX.len_utf8() + 1).to_owned();
    if padded {
        prefix.truncate(prefix.trim_end_matches(' ').len());
    }
    while let Some(last) = prefix.pop() {
        let Some(next) = successor(last) else { continue };
        let next = if padded && next == ' ' { '!' } else { next };
        prefix.push(next);
        return Some(prefix);
    }
    None
}

/// The next Unicode scalar value, skipping the surrogate range.
fn successor(c: char) -> Option<char> {
    match c {
        '\u{D7FF}' => Some('\u{E000}'),
        char::MAX => None,
        _ => char::from_u32(c as u32 + 1),
    }
}

/// The most bytes a rendered `\x` bound can carry within the cap.
const BYTEA_CAP_BYTES: usize = (STORED_VALUE_CAP - 2) / 2;

/// A rendered `bytea` above the value: its prefix with trailing `0xFF` bytes
/// dropped and the last byte incremented. `None` when every byte is `0xFF`.
fn bytea_upper(text: &str) -> Option<String> {
    let mut bytes = decode_bytea(text)?;
    bytes.truncate(BYTEA_CAP_BYTES);
    while bytes.last() == Some(&0xFF) {
        bytes.pop();
    }
    *bytes.last_mut()? += 1;
    Some(render_bytea(&bytes))
}

/// A column's dictionary. A `character` entry is its text without the
/// trailing blanks its comparison ignores, deduplicated and measured against
/// [`STORED_VALUE_CAP`] as such (`docs/design/decisions.md`, "D34").
struct DictionaryGatherer {
    /// Whether the column is `character`, whose entries give up their padding.
    padded: bool,
    entries: Vec<String>,
    interned: HashMap<String, u32>,
    groups: Vec<Option<Vec<u32>>>,
    /// The group's distinct texts in first-seen order; `None` once past a cap.
    group: Option<Vec<String>>,
}

impl DictionaryGatherer {
    fn new(kind: &CompareKind) -> Self {
        Self {
            padded: matches!(kind, CompareKind::PaddedText),
            entries: Vec::new(),
            interned: HashMap::new(),
            groups: Vec::new(),
            group: Some(Vec::new()),
        }
    }

    fn observe(&mut self, text: &str) {
        let Some(group) = &mut self.group else { return };
        let text = if self.padded { text.trim_end_matches(' ') } else { text };
        if group.iter().any(|seen| seen == text) {
            return;
        }
        if text.len() > STORED_VALUE_CAP || group.len() == DICTIONARY_CAP {
            self.group = None;
            return;
        }
        group.push(text.to_owned());
    }

    fn close_group(&mut self) {
        let group = self.group.replace(Vec::new());
        let indices = group.map(|texts| {
            texts
                .into_iter()
                .map(|text| match self.interned.get(&text) {
                    Some(&id) => id,
                    None => {
                        let id = self.entries.len() as u32;
                        self.entries.push(text.clone());
                        self.interned.insert(text, id);
                        id
                    }
                })
                .collect()
        });
        self.groups.push(indices);
    }

    fn finish(self) -> ColumnDictionary {
        ColumnDictionary { entries: self.entries, groups: self.groups }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SplitMix64, seeded, so a failing case reproduces.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// A value of `kind` drawn to collide: short and long, sharing long
    /// prefixes, padded, and — for text — past the ASCII range.
    fn value(rng: &mut Rng, kind: &CompareKind) -> String {
        let length = match rng.below(4) {
            0 => rng.below(4) as usize,
            1 => STORED_VALUE_CAP - 2 + rng.below(8) as usize,
            _ => 250 + rng.below(40) as usize,
        };
        let alphabet: &[char] = &['a', 'b', ' ', 'é', '\u{10FFFF}', '\u{1f}'];
        match kind {
            CompareKind::Int => (rng.below(20) as i64 - 10).to_string(),
            CompareKind::Bytea => {
                let bytes: Vec<u8> = (0..length / 2)
                    .map(|_| [0x00, 0x7f, 0xfe, 0xff][rng.below(4) as usize])
                    .collect();
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

    /// **Every stored bound is on the right side of every value in its group,
    /// under the key a filter orders by, and a stated block order holds row by
    /// row** — over random groups of the three bytewise kinds and a keyed one,
    /// whose values collide on long shared prefixes past the cap.
    #[test]
    fn stored_bounds_and_order_hold_every_value_under_its_key() {
        let mut rng = Rng(0x5eed);
        let kinds =
            [CompareKind::Text, CompareKind::PaddedText, CompareKind::Bytea, CompareKind::Int];
        let (mut bounded, mut ordered) = (0usize, 0usize);
        for round in 0..400 {
            let kind = &kinds[round % kinds.len()];
            let mut gatherer = BoundsGatherer::new(kind.clone());
            let base = value(&mut rng, kind);
            let groups: Vec<Vec<String>> = (0..1 + rng.below(4))
                .map(|_| {
                    (0..rng.below(6))
                        .map(|_| {
                            let v = value(&mut rng, kind);
                            // Half share a long prefix with one base value.
                            if rng.below(2) == 0
                                && !matches!(kind, CompareKind::Int | CompareKind::Bytea)
                            {
                                format!("{base}{v}")
                            } else {
                                v
                            }
                        })
                        .collect()
                })
                .collect();
            if round % 3 == 0 {
                // Sorted input, so a stated order is sometimes Ascending.
                let mut all: Vec<String> = groups.concat();
                all.sort_by(|a, b| {
                    ValueKey::of(kind, a).unwrap().compare(&ValueKey::of(kind, b).unwrap())
                });
                for v in &all {
                    gatherer.observe(v);
                }
                gatherer.close_group();
                let column = gatherer.finish();
                assert_ne!(column.sortedness, Sortedness::Descending, "{kind:?}: sorted input");
                continue;
            }
            for group in &groups {
                for v in group {
                    gatherer.observe(v);
                }
                gatherer.close_group();
            }
            let all: Vec<&String> = groups.iter().flatten().collect();
            let column = gatherer.finish();
            for (group, stored) in groups.iter().zip(&column.groups) {
                let Some(b) = stored else {
                    assert!(
                        group.is_empty() || matches!(kind, CompareKind::Int),
                        "{kind:?}: a group of {} values lost its bounds",
                        group.len()
                    );
                    continue;
                };
                bounded += 1;
                assert!(b.min.len() <= STORED_VALUE_CAP && b.max.len() <= STORED_VALUE_CAP);
                let low = ValueKey::of(kind, &b.min).expect("a stored min keys");
                let high = ValueKey::of(kind, &b.max).expect("a stored max keys");
                for v in group {
                    let key = ValueKey::of(kind, v).unwrap();
                    assert_ne!(
                        low.compare(&key),
                        Ordering::Greater,
                        "{kind:?}: min {:?} above {v:?}",
                        b.min
                    );
                    assert_ne!(
                        high.compare(&key),
                        Ordering::Less,
                        "{kind:?}: max {:?} below {v:?}",
                        b.max
                    );
                }
                if b.max_exact {
                    assert!(
                        group
                            .iter()
                            .any(|v| ValueKey::of(kind, v).unwrap().compare(&high)
                                == Ordering::Equal)
                    );
                }
            }
            let keys: Vec<ValueKey> = all.iter().map(|v| ValueKey::of(kind, v).unwrap()).collect();
            let pairs = keys.windows(2).map(|w| w[1].compare(&w[0]));
            match column.sortedness {
                Sortedness::Ascending => {
                    ordered += 1;
                    assert!(pairs.clone().all(Ordering::is_ge), "{kind:?}: not ascending");
                }
                Sortedness::Descending => {
                    ordered += 1;
                    assert!(pairs.clone().all(Ordering::is_le), "{kind:?}: not descending");
                }
                Sortedness::Unsorted => {}
            }
        }
        assert!(bounded > 300, "only {bounded} groups bounded");
        assert!(ordered > 50, "only {ordered} blocks ordered");
    }

    fn cmp_text(a: &str, b: &str) -> Ordering {
        a.as_bytes().cmp(b.as_bytes())
    }

    #[test]
    fn a_long_texts_stored_bounds_are_on_the_right_side_of_it() {
        let long = format!("{}é{}", "a".repeat(250), "z".repeat(40));
        let lower = text_prefix(&long, STORED_VALUE_CAP);
        assert!(lower.len() <= STORED_VALUE_CAP);
        assert_ne!(cmp_text(lower, &long), Ordering::Greater);
        let upper = text_upper(&long, false).unwrap();
        assert!(upper.len() <= STORED_VALUE_CAP);
        assert_eq!(cmp_text(&upper, &long), Ordering::Greater);

        // The last character has no successor, so the one before it moves.
        let top = format!("{}{}", "b".repeat(300), char::MAX);
        let upper = text_upper(&top, false).unwrap();
        assert_eq!(cmp_text(&upper, &top), Ordering::Greater);
        assert!(text_upper(&char::MAX.to_string().repeat(100), false).is_none());
    }

    #[test]
    fn a_padded_upper_bound_is_above_the_value_once_blanks_are_dropped() {
        let long = format!("{}\u{1f}{}", "a".repeat(252), " x".repeat(40));
        let upper = text_upper(&long, true).unwrap();
        let key = |t: &str| t.trim_end_matches(' ').to_owned();
        assert_eq!(cmp_text(&key(&upper), &key(&long)), Ordering::Greater);
        let blanks = format!("{}{}", "a".repeat(10), " ".repeat(300));
        let upper = text_upper(&blanks, true).unwrap();
        assert_eq!(cmp_text(&key(&upper), &key(&blanks)), Ordering::Greater);
    }

    #[test]
    fn a_long_byteas_stored_bounds_are_on_the_right_side_of_it() {
        let mut bytes = vec![0x10u8; 200];
        bytes.extend([0xFF; 3]);
        let text = render_bytea(&bytes);
        let value = decode_bytea(&text).unwrap();
        let lower = text_prefix(&text, STORED_VALUE_CAP);
        assert!(lower.len() <= STORED_VALUE_CAP);
        assert!(decode_bytea(lower).unwrap() <= value);
        let upper = bytea_upper(&text).unwrap();
        assert!(upper.len() <= STORED_VALUE_CAP);
        assert!(decode_bytea(&upper).unwrap() > value);
        assert!(bytea_upper(&render_bytea(&[0xFF; 300])).is_none());
    }
}
