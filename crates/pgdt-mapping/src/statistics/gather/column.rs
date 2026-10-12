//! One column's gathering: its NULL counts, sums and unrepresentable counts
//! per group, beside the bounds and the row order it keeps, and its
//! per-group dictionary of the text it emits (`docs/design/decisions.md`,
//! "D34") — the group's texts, kept as the group keeps them.

use std::collections::HashMap;
use std::mem::{self, size_of};

use arrow_schema::DataType;

use pgdt_codec::compare::{CompareKind, ComparisonPlan, ComparisonSemantics};
use pgdt_codec::internal::ColumnTier;
use pgdt_codec::pgtype::NestedPlan;
use pgdt_core::decode_field;
use pgdt_model::internal::UnrepresentableExt;
use pgdt_model::{
    ColumnDictionary, ColumnStatistics, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES,
    PostgresInvalidValues, Unrepresentable,
    internal::{text_heap, vec_heap},
};

use super::bounds::{BoundsGatherer, GroupBounds, Refused, merge_adjacent};
use super::charge::{grown_map_heap, map_heap, push_charged, reserve_charged};
use crate::statistics::account::{Charge, STATISTICS_ACCOUNT_CHARGE_STEP};
use crate::statistics::stored::Summand;

/// What [`ColumnGatherer::observe`] answers of a field it read.
pub(super) struct Observed {
    /// What the column's heap grew by.
    pub(super) grew: i64,
    /// Whether the field is one its type's `*_in` refuses, kept out of the
    /// group's statistics rather than stopping the block.
    pub(super) ignored: bool,
}

pub(super) struct ColumnGatherer {
    pub(super) declared_type: Option<String>,
    collation: Option<String>,
    null_counts: Vec<u64>,
    /// How a value is summed, for a column keeping sums.
    summand: Option<Summand>,
    /// Each closed group's sum, `None` for a column keeping none and once a
    /// value has failed to decode ([`ColumnStatistics::sums`]).
    sums: Option<Vec<i128>>,
    /// Each closed group's text bytes ([`ColumnStatistics::value_bytes`]).
    value_bytes: Vec<u64>,
    /// Which tier a value is past, for a column whose type cannot hold every
    /// value its declared type can (`pgdt_codec::unrepresentable`).
    tier: Option<ColumnTier>,
    /// Each closed group's count of such values, for a column `tier` tests
    /// ([`ColumnStatistics::unrepresentable`]).
    unrepresentable: Option<Vec<Unrepresentable>>,
    /// One per [`pgdt_model::BoundsSet`], in its order.
    pub(super) bounds: [Option<BoundsGatherer>; 2],
    dictionary: Option<DictionaryGatherer>,
    /// The column over the open group.
    pub(super) group: GroupState,
    /// The column over a piece's held first group ([`Gatherer::head`](super::gatherer::Gatherer::head)).
    pub(super) head: Option<GroupState>,
    /// Whether a field its type's `*_in` refuses stops the block, or is kept
    /// out of the group's statistics ([`Self::observe`]).
    invalid: PostgresInvalidValues,
    /// Whether the column's declaration makes it `NOT NULL`, so a NULL is
    /// refused as such a field is (I76).
    pub(super) not_null: bool,
}

impl ColumnGatherer {
    /// A set of bounds under each of `bounds`, the kinds some semantics
    /// orders the column by exactly (`pgdt_codec::ResolvedSchema::bounds_kinds`),
    /// and a dictionary where the comparison equates exactly
    /// (`docs/design/decisions.md`, "D79"); a sum per group where the typed
    /// read emits `data_type` as an integer or a `Decimal128`; a count per
    /// group of the values past `tier`, where the type cannot hold every one;
    /// and, as for every tracked column, its NULLs and text bytes per group
    /// ([`ColumnStatistics`]).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        declared_type: Option<String>,
        collation: Option<String>,
        bounds: [Option<CompareKind>; 2],
        comparison: &ComparisonPlan,
        plan: &NestedPlan,
        data_type: &DataType,
        tier: Option<ColumnTier>,
        invalid: PostgresInvalidValues,
    ) -> Self {
        let postgres = ComparisonSemantics::Postgres;
        let dictionary = match comparison {
            ComparisonPlan::Compared { kind, .. } if plan.is_scalar() => {
                comparison.dictionary_answers_in(postgres).then(|| DictionaryGatherer::new(kind))
            }
            _ => None,
        };
        let bounds = bounds.map(|kind| kind.map(BoundsGatherer::new));
        let summand = Summand::of(data_type, plan);
        Self::with(declared_type, collation, bounds, dictionary, summand, tier, invalid)
    }

    fn with(
        declared_type: Option<String>,
        collation: Option<String>,
        bounds: [Option<BoundsGatherer>; 2],
        dictionary: Option<DictionaryGatherer>,
        summand: Option<Summand>,
        tier: Option<ColumnTier>,
        invalid: PostgresInvalidValues,
    ) -> Self {
        let group = GroupState::fresh(&bounds);
        Self {
            declared_type,
            collation,
            null_counts: Vec::new(),
            summand,
            sums: summand.map(|_| Vec::new()),
            value_bytes: Vec::new(),
            unrepresentable: tier.as_ref().map(|_| Vec::new()),
            tier,
            bounds,
            dictionary,
            group,
            head: None,
            invalid,
            not_null: false,
        }
    }

    /// The heap this column holds, as its structure and its interning map,
    /// its open group and a held head included. O(1) but for a keyed bound's
    /// key, which [`DICTIONARY_ENTRY_MAX_BYTES`](pgdt_model::DICTIONARY_ENTRY_MAX_BYTES) bounds.
    pub(super) fn held(&self) -> (u64, u64) {
        let named = self.declared_type.iter().chain(&self.collation).map(text_heap).sum::<u64>();
        let head = self.head.as_ref().map_or(0, GroupState::heap_bytes);
        let per_group = vec_heap(&self.null_counts)
            + self.sums.as_ref().map_or(0, vec_heap)
            + vec_heap(&self.value_bytes)
            + self.unrepresentable.as_ref().map_or(0, vec_heap);
        let mut structure = named + per_group + self.group.heap_bytes() + head;
        for bounds in self.bounds.iter().flatten() {
            structure += vec_heap(&bounds.groups)
                + vec_heap(&bounds.flags)
                + vec_heap(&bounds.tiers)
                + bounds.stored
                + bounds.rows.heap_bytes();
        }
        let mut interned = 0;
        if let Some(dictionary) = &self.dictionary {
            let (entries, map) = dictionary.held();
            structure += entries;
            interned = map;
        }
        (structure, interned)
    }

    /// The heap a row can change: the open group and the row order.
    fn open_heap(&self) -> u64 {
        self.group.heap_bytes()
            + self.bounds.iter().flatten().map(|b| b.rows.heap_bytes()).sum::<u64>()
    }

    /// A column of this one's kind that has gathered nothing: what a piece
    /// starts from.
    pub(super) fn fresh(&self) -> Self {
        let fresh = Self::with(
            self.declared_type.clone(),
            self.collation.clone(),
            self.bounds.each_ref().map(|b| b.as_ref().map(BoundsGatherer::fresh)),
            self.dictionary.as_ref().map(DictionaryGatherer::fresh),
            self.summand,
            self.tier.clone(),
            self.invalid,
        );
        Self { not_null: self.not_null, ..fresh }
    }

    /// The open group's state, leaving a fresh one open.
    pub(super) fn take_group(&mut self) -> GroupState {
        let fresh = GroupState::fresh(&self.bounds);
        mem::replace(&mut self.group, fresh)
    }

    pub(super) fn hold_head(&mut self) {
        self.head = Some(self.take_group());
    }

    /// Observe one field, answering what the column's heap grew by and
    /// whether it went past a value PostgreSQL refuses, or [`Refused`] where
    /// keying it found one, or it is a NULL the column's `NOT NULL` refuses,
    /// and [`Self::invalid`] refuses it.
    pub(super) fn observe(&mut self, field: &[u8]) -> Result<Observed, Refused> {
        let before = self.open_heap();
        let mut ignored = false;
        match decode_field(field) {
            // A NULL its `NOT NULL` refuses and that is gone past is counted,
            // the read taking it as the NULL it is: no statistic contradicts
            // it (D103).
            Ok(None) => {
                if self.not_null {
                    if self.invalid != PostgresInvalidValues::Ignore {
                        return Err(Refused);
                    }
                    ignored = true;
                }
                self.group.nulls += 1;
                return Ok(Observed { grew: 0, ignored });
            }
            Ok(Some(text)) => {
                let tier = self.tier.as_ref().and_then(|tier| tier.of(&text));
                if let Some(tier) = tier {
                    self.group.unrepresentable.add(tier);
                }
                self.group.value_bytes += text.len() as u64;
                // A value the type cannot hold is left out, as a NULL is: the
                // sum is of the values the type holds, read only in a view
                // taking the rest as NULL (`docs/design/decisions.md`, "D98").
                if let Some(summand) = self.summand.filter(|_| tier.is_none()) {
                    self.group.sum = self
                        .group
                        .sum
                        .zip(summand.value(&text))
                        .map(|(sum, value)| sum.wrapping_add(value));
                }
                let mut refused = false;
                for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                    if let (Some(bounds), Some(group)) = (bounds, group) {
                        refused |= bounds.observe(group, &text, tier).is_err();
                    }
                }
                if refused {
                    if self.invalid != PostgresInvalidValues::Ignore {
                        return Err(Refused);
                    }
                    // Ignored: no statistic a read could contradict (D103).
                    ignored = true;
                    self.group.sum = None;
                    for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                        if let (Some(bounds), Some(group)) = (bounds, group) {
                            bounds.lose_value(group, tier);
                        }
                    }
                    self.group.lose_texts();
                } else if let Some(dictionary) = &self.dictionary {
                    dictionary.observe(&mut self.group, &text);
                }
            }
            // Not text at all, so neither a key nor an entry: the group can
            // cover the row with neither, and the block's order is lost. No
            // decoding is longer than the escaped field, and nothing sums it.
            Err(_) => {
                self.group.value_bytes += field.len() as u64;
                self.group.sum = None;
                for (bounds, group) in self.bounds.iter_mut().zip(&mut self.group.bounds) {
                    if let (Some(bounds), Some(group)) = (bounds, group) {
                        bounds.lose_value(group, None);
                    }
                }
                self.group.lose_texts();
            }
        }
        Ok(Observed { grew: self.open_heap() as i64 - before as i64, ignored })
    }

    /// Close the open group, and move what it changed onto `charge` before
    /// the next column closes: **a close's growth is charged column by
    /// column**, so a wide block's account never waits on all its columns.
    /// A dictionary's interning map growing here is charged as it grows
    /// (`DictionaryGatherer::intern`), which the column's own difference then
    /// takes in rather than adding twice.
    pub(super) fn close_group(&mut self, charge: &mut Charge) {
        let before = self.held();
        let base = charge.charged();
        let group = self.take_group();
        push_charged(&mut self.null_counts, group.nulls, charge);
        push_charged(&mut self.value_bytes, group.value_bytes, charge);
        if let Some(counts) = &mut self.unrepresentable {
            push_charged(counts, group.unrepresentable, charge);
        }
        match (&mut self.sums, group.sum) {
            (Some(sums), Some(sum)) => push_charged(sums, sum, charge),
            // The column held a value that is not its type: it has no sum.
            (sums, None) => *sums = None,
            (None, Some(_)) => {}
        }
        for (bounds, group) in self.bounds.iter_mut().zip(group.bounds) {
            if let (Some(bounds), Some(group)) = (bounds, group) {
                bounds.close_group(group, charge);
            }
        }
        if let Some(dictionary) = &mut self.dictionary {
            dictionary.close_group(group.texts, charge);
        }
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
    }

    /// Merge every closed group with the one after it
    /// ([`Gatherer::merge_pairs`](super::gatherer::Gatherer::merge_pairs)), moving `charge` by what the column frees
    /// before its dictionary renumbers, and again after.
    pub(super) fn merge_pairs(&mut self, charge: &mut Charge) {
        let (before, base) = (self.held(), charge.charged());
        merge_adjacent(&mut self.null_counts, |a, b| a + b);
        if let Some(sums) = &mut self.sums {
            merge_adjacent(sums, i128::wrapping_add);
        }
        merge_adjacent(&mut self.value_bytes, |a, b| a + b);
        if let Some(counts) = &mut self.unrepresentable {
            merge_adjacent(counts, |mut a, b| {
                a.merge(&b);
                a
            });
        }
        for bounds in self.bounds.iter_mut().flatten() {
            bounds.merge_pairs();
        }
        if let Some(dictionary) = &mut self.dictionary {
            merge_adjacent(&mut dictionary.groups, union_indices);
            dictionary.indices = dictionary.groups.iter().flatten().map(vec_heap).sum();
        }
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
        if let Some(dictionary) = &mut self.dictionary {
            let (before, base) = (dictionary.held(), charge.charged());
            dictionary.renumber(charge);
            let after = dictionary.held();
            charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
        }
    }

    /// Take the last closed group back into the open group, which follows it
    /// ([`Gatherer::reopen_last`](super::gatherer::Gatherer::reopen_last)), moving `charge` by the difference.
    pub(super) fn reopen_last(&mut self, charge: &mut Charge) {
        let (before, base) = (self.held(), charge.charged());
        let nulls = self.null_counts.pop().expect("a closed group counts its NULLs");
        // A column that has lost its sums reopens a group without one, which
        // loses nothing further.
        let sum = self.sums.as_mut().map(|sums| sums.pop().expect("a closed group sums"));
        let value_bytes = self.value_bytes.pop().expect("a closed group counts its text");
        let unrepresentable =
            self.unrepresentable.as_mut().map_or_else(Unrepresentable::default, |counts| {
                counts.pop().expect("a closed group counts what its type cannot hold")
            });
        let bounds = self.bounds.each_mut().map(|b| b.as_mut().map(BoundsGatherer::reopen_last));
        let texts = match &mut self.dictionary {
            Some(dictionary) => dictionary.reopen_last(),
            None => Some(Vec::new()),
        };
        let text_bytes = texts.iter().flatten().map(text_heap).sum();
        let mut group =
            GroupState { nulls, sum, value_bytes, unrepresentable, bounds, texts, text_bytes };
        group.absorb(mem::replace(&mut self.group, GroupState::fresh(&[None, None])));
        self.group = group;
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
    }

    /// Append `later`'s closed groups, which follow this column's, and take
    /// its open group as this one's — **draining `later` in place**, and after
    /// each part moves or is freed moving `charge` by the pair's difference,
    /// so the account follows the piece's structures across rather than
    /// releasing them all at the end.
    pub(super) fn append(&mut self, later: &mut ColumnGatherer, charge: &mut Charge) {
        let pair = |mine: &Self, theirs: &Self| {
            let ((a, b), (c, d)) = (mine.held(), theirs.held());
            (a + c, b + d)
        };
        let before = pair(self, later);
        let base = charge.charged();
        let resync = |now: (u64, u64), charge: &mut Charge| {
            charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
        };
        reserve_charged(&mut self.null_counts, later.null_counts.len(), charge);
        self.null_counts.extend_from_slice(&later.null_counts);
        later.null_counts = Vec::new();
        resync(pair(self, later), charge);
        reserve_charged(&mut self.value_bytes, later.value_bytes.len(), charge);
        self.value_bytes.extend_from_slice(&later.value_bytes);
        later.value_bytes = Vec::new();
        resync(pair(self, later), charge);
        if let (Some(mine), Some(theirs)) = (&mut self.unrepresentable, &mut later.unrepresentable)
        {
            reserve_charged(mine, theirs.len(), charge);
            mine.append(theirs);
            *theirs = Vec::new();
            resync(pair(self, later), charge);
        }
        match (&mut self.sums, &mut later.sums) {
            (Some(mine), Some(theirs)) => {
                reserve_charged(mine, theirs.len(), charge);
                mine.append(theirs);
                *theirs = Vec::new();
            }
            // The piece met a value that is not the column's type.
            (sums, None) => *sums = None,
            (None, Some(_)) => {}
        }
        resync(pair(self, later), charge);
        for set in 0..self.bounds.len() {
            let (Some(mine), Some(theirs)) = (&mut self.bounds[set], &mut later.bounds[set]) else {
                continue;
            };
            mine.append_tiers(theirs, charge);
            reserve_charged(&mut mine.groups, theirs.groups.len(), charge);
            reserve_charged(&mut mine.flags, theirs.flags.len(), charge);
            mine.groups.append(&mut theirs.groups);
            mine.flags.append(&mut theirs.flags);
            mine.stored += mem::take(&mut theirs.stored);
            (theirs.groups, theirs.flags) = (Vec::new(), Vec::new());
            resync(pair(self, later), charge);
        }
        let fresh = GroupState::fresh(&later.bounds);
        self.group = mem::replace(&mut later.group, fresh);
        if let (Some(mine), Some(theirs)) = (&mut self.dictionary, &mut later.dictionary) {
            mine.append(theirs, charge);
        }
        resync(pair(self, later), charge);
    }

    pub(super) fn finish(self) -> ColumnStatistics {
        let [primary, datafusion] = self.bounds;
        ColumnStatistics {
            declared_type: self.declared_type,
            collation: self.collation,
            null_counts: self.null_counts,
            sums: self.sums,
            value_bytes: self.value_bytes,
            bounds: primary.map(BoundsGatherer::finish),
            datafusion_bounds: datafusion.map(BoundsGatherer::finish),
            dictionary: self.dictionary.map(DictionaryGatherer::finish),
            unrepresentable: self
                .unrepresentable
                .filter(|counts| counts.iter().any(|count| !count.is_zero())),
        }
    }
}

/// One column over one group while the group is open.
pub(super) struct GroupState {
    nulls: u64,
    /// The values' wrapping sum so far, `None` once one did not decode as
    /// the column's type; read only for a column keeping sums.
    sum: Option<i128>,
    /// The values' text bytes so far.
    value_bytes: u64,
    /// The values past what the column's type holds so far, by tier.
    unrepresentable: Unrepresentable,
    /// One per [`pgdt_model::BoundsSet`], `None` for a set the column does not keep.
    bounds: [Option<GroupBounds>; 2],
    /// The group's distinct texts in first-seen order, `None` once past a
    /// cap, at a field that is not text or at one an ignoring parse went past;
    /// read only for a column keeping a dictionary.
    texts: Option<Vec<String>>,
    /// The heap `texts`' strings hold, summed as each is pushed.
    text_bytes: u64,
}

impl GroupState {
    fn fresh(bounds: &[Option<BoundsGatherer>; 2]) -> Self {
        Self {
            nulls: 0,
            sum: Some(0),
            value_bytes: 0,
            unrepresentable: Unrepresentable::default(),
            bounds: bounds.each_ref().map(|b| b.as_ref().map(BoundsGatherer::fresh_group)),
            texts: Some(Vec::new()),
            text_bytes: 0,
        }
    }

    /// No distinct texts for this group: a cap passed, a field not text, or
    /// one an ignoring parse went past.
    fn lose_texts(&mut self) {
        self.texts = None;
        self.text_bytes = 0;
    }

    /// The heap this group's state holds: its distinct texts and its running
    /// bounds.
    fn heap_bytes(&self) -> u64 {
        let texts = self.texts.as_ref().map_or(0, |texts| vec_heap(texts) + self.text_bytes);
        let bounds: u64 = self
            .bounds
            .iter()
            .flatten()
            .map(|bounds| match bounds {
                GroupBounds::Bytewise { min, max, .. } => {
                    [min, max].into_iter().flatten().map(|c| text_heap(&c.head)).sum()
                }
                GroupBounds::Keyed(group) => group.heap_bytes(),
            })
            .sum();
        texts + bounds
    }

    /// Fold `later`, the same column over the same group's following rows.
    /// The texts fold as the rows would have: each new one appended until a
    /// cap is passed.
    pub(super) fn absorb(&mut self, later: GroupState) {
        self.nulls += later.nulls;
        self.sum = self.sum.zip(later.sum).map(|(sum, more)| sum.wrapping_add(more));
        self.value_bytes += later.value_bytes;
        self.unrepresentable.merge(&later.unrepresentable);
        for (mine, theirs) in self.bounds.iter_mut().zip(later.bounds) {
            if let (Some(mine), Some(theirs)) = (mine, theirs) {
                mine.absorb(theirs);
            }
        }
        let Some(theirs) = later.texts else {
            self.lose_texts();
            return;
        };
        let Some(mine) = &mut self.texts else { return };
        for text in theirs {
            if mine.contains(&text) {
                continue;
            }
            if mine.len() == DICTIONARY_MAX_ENTRIES {
                self.lose_texts();
                return;
            }
            self.text_bytes += text_heap(&text);
            mine.push(text);
        }
    }
}

/// A column's dictionary. A `character` entry is its text without the
/// trailing blanks its comparison ignores, deduplicated and measured against
/// [`DICTIONARY_ENTRY_MAX_BYTES`] as such (`docs/design/decisions.md`, "D34").
pub(super) struct DictionaryGatherer {
    /// Whether the column is `character`, whose entries give up their padding.
    padded: bool,
    entries: Vec<String>,
    /// The text every entry holds, summed as each is interned — and so the
    /// text the interning map's keys hold too.
    entry_text: u64,
    interned: HashMap<String, u32>,
    groups: Vec<Option<Vec<u32>>>,
    /// The heap every group's index list holds, summed as each is pushed.
    indices: u64,
}

impl DictionaryGatherer {
    pub(super) fn new(kind: &CompareKind) -> Self {
        Self {
            padded: matches!(kind, CompareKind::PaddedText { .. }),
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    pub(super) fn fresh(&self) -> Self {
        Self {
            padded: self.padded,
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    /// The heap this dictionary holds, as its entries and index lists and as
    /// its interning map.
    pub(super) fn held(&self) -> (u64, u64) {
        let entries =
            vec_heap(&self.entries) + self.entry_text + vec_heap(&self.groups) + self.indices;
        (entries, map_heap::<String, u32>(self.interned.capacity()) + self.entry_text)
    }

    /// Add `text` to `group`'s distinct texts, which a cap passed leaves
    /// `None`.
    pub(super) fn observe(&self, group: &mut GroupState, text: &str) {
        let Some(texts) = &mut group.texts else { return };
        let text = if self.padded { text.trim_end_matches(' ') } else { text };
        if texts.iter().any(|seen| seen == text) {
            return;
        }
        if text.len() > DICTIONARY_ENTRY_MAX_BYTES || texts.len() == DICTIONARY_MAX_ENTRIES {
            group.lose_texts();
            return;
        }
        let text = text.to_owned();
        group.text_bytes += text_heap(&text);
        texts.push(text);
    }

    /// **A map about to grow is charged its larger table before the insert
    /// allocates it**, and released of the smaller once the insert has freed
    /// it: the two are allocated at once while the table rehashes, and a
    /// close interning into several full maps would otherwise hold each
    /// growth uncharged until its observer next updates the account.
    fn intern(&mut self, text: &str, charge: &mut Charge) -> u32 {
        if let Some(&id) = self.interned.get(text) {
            return id;
        }
        let id = self.entries.len() as u32;
        push_charged(&mut self.entries, text.to_owned(), charge);
        let capacity = self.interned.capacity();
        let key = text.to_owned();
        if self.interned.len() == capacity {
            let grown = grown_map_heap::<String, u32>(capacity);
            let smaller = map_heap::<String, u32>(capacity);
            charge.ahead((0, grown), (0, smaller), || self.interned.insert(key, id));
        } else {
            self.interned.insert(key, id);
        }
        self.entry_text += text.len() as u64;
        id
    }

    pub(super) fn close_group(&mut self, texts: Option<Vec<String>>, charge: &mut Charge) {
        let indices: Option<Vec<u32>> =
            texts.map(|texts| texts.iter().map(|text| self.intern(text, charge)).collect());
        self.indices += indices.as_ref().map_or(0, vec_heap);
        push_charged(&mut self.groups, indices, charge);
    }

    /// Append `later`'s groups, which follow this dictionary's, interning
    /// their entries here in the order a pass closing them would have —
    /// draining `later`'s index lists as they are re-interned, and moving
    /// `charge` by the pair's difference whenever what this one gained passes
    /// [`STATISTICS_ACCOUNT_CHARGE_STEP`]. `later`'s entries and map stay until it is dropped.
    pub(super) fn append(&mut self, later: &mut DictionaryGatherer, charge: &mut Charge) {
        let pair = |mine: &Self, theirs: &Self| {
            let ((a, b), (c, d)) = (mine.held(), theirs.held());
            (a + c, b + d)
        };
        let before = pair(self, later);
        let base = charge.charged();
        let mut synced = self.entry_text * 2 + self.indices;
        for at in 0..later.groups.len() {
            let indices: Option<Vec<u32>> = later.groups[at].take().map(|ids| {
                later.indices -= vec_heap(&ids);
                ids.iter().map(|&id| self.intern(&later.entries[id as usize], charge)).collect()
            });
            self.indices += indices.as_ref().map_or(0, vec_heap);
            push_charged(&mut self.groups, indices, charge);
            let gained = self.entry_text * 2 + self.indices;
            if gained - synced >= STATISTICS_ACCOUNT_CHARGE_STEP {
                let now = pair(self, later);
                charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
                synced = gained;
            }
        }
        later.groups = Vec::new();
        let now = pair(self, later);
        charge.set(base.0 + now.0 - before.0, base.1 + now.1 - before.1);
    }

    /// The last closed group's distinct texts, no longer listed.
    pub(super) fn reopen_last(&mut self) -> Option<Vec<String>> {
        let ids = self.groups.pop().expect("a closed group lists its texts")?;
        self.indices -= vec_heap(&ids);
        Some(ids.iter().map(|&id| self.entries[id as usize].clone()).collect())
    }

    /// **Renumber the entries the groups still name in the order a pass
    /// closing these groups interns them, and let every other go**: a merge
    /// past [`DICTIONARY_MAX_ENTRIES`] leaves entries no group names, and a merged
    /// group lists its second half's new texts after its first's. The
    /// renumbering's scratch is charged ahead of it, and **the interning map
    /// is rebuilt rather than pruned**, its new table charged ahead and the old
    /// one released in one update with every text let go: a table pruned in
    /// place keeps its size while its reported capacity stops naming it
    /// (`docs/design/runtime-invariants.md`, "RT11").
    fn renumber(&mut self, charge: &mut Charge) {
        let scratch = (self.entries.len() * size_of::<u32>()) as u64;
        let mut to = charge.ahead((scratch, 0), (0, 0), || vec![u32::MAX; self.entries.len()]);
        let mut kept = 0u32;
        for id in self.groups.iter_mut().flatten().flatten() {
            let slot = &mut to[*id as usize];
            if *slot == u32::MAX {
                *slot = kept;
                kept += 1;
            }
            *id = *slot;
        }
        let old_table = map_heap::<String, u32>(self.interned.capacity());
        let new_table = match kept {
            0 => 0,
            kept => grown_map_heap::<String, u32>(kept as usize - 1),
        };
        charge.ahead_freeing((0, new_table), || {
            let mut interned = HashMap::with_capacity(kept as usize);
            for (key, id) in mem::take(&mut self.interned) {
                if to[id as usize] != u32::MAX {
                    interned.insert(key, to[id as usize]);
                }
            }
            self.interned = interned;
            // Every entry no group names goes past the kept ones, then the
            // permutation is applied in place, cycle by cycle.
            for (past, slot) in (kept..).zip(to.iter_mut().filter(|slot| **slot == u32::MAX)) {
                *slot = past;
            }
            for at in 0..to.len() {
                while to[at] as usize != at {
                    let there = to[at] as usize;
                    self.entries.swap(at, there);
                    to.swap(at, there);
                }
            }
            drop(to);
            let dropped: u64 =
                self.entries[kept as usize..].iter().map(|entry| entry.len() as u64).sum();
            self.entries.truncate(kept as usize);
            self.entry_text -= dropped;
            ((), (scratch + dropped, old_table + dropped))
        });
    }

    pub(super) fn finish(self) -> ColumnDictionary {
        ColumnDictionary { entries: self.entries, groups: self.groups }
    }
}

/// One group's distinct texts, as indices, merged with the following group's
/// as one pass over both groups' values gathers them: the first's, then the
/// second's not already listed, and none past [`DICTIONARY_MAX_ENTRIES`].
fn union_indices(first: Option<Vec<u32>>, second: Option<Vec<u32>>) -> Option<Vec<u32>> {
    let (first, second) = (first?, second?);
    let added = second.iter().filter(|id| !first.contains(id)).count();
    if added == 0 {
        return Some(first);
    }
    if first.len() + added > DICTIONARY_MAX_ENTRIES {
        return None;
    }
    let mut union = Vec::with_capacity(first.len() + added);
    union.extend_from_slice(&first);
    union.extend(second.iter().filter(|id| !first.contains(id)));
    Some(union)
}
