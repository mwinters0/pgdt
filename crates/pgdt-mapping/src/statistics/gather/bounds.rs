//! A column's per-group bounds: bytewise over a clipped canonical text or
//! keyed by the order a filter compares by, each view of an unrepresentable
//! column's values kept apart where it differs (`docs/design/decisions.md`,
//! "D76", "D97").

use std::borrow::Cow;
use std::cmp::Ordering;
use std::mem::{self, size_of};
#[cfg(any(test, feature = "test-support"))]
use std::sync::Arc;

use pgdt_codec::compare::CompareKind;
use pgdt_codec::key::ValueKey;
use pgdt_codec::{
    decode::{Unread, decode_bytea, decode_bytea_escape, render_bytea},
    internal::{bytea_unread, char_typmod_refuses},
};
use pgdt_model::{
    Bounds, DICTIONARY_ENTRY_MAX_BYTES, UnrepresentableTier,
    internal::{BoundsView, ColumnBounds, text_heap, vec_heap},
};

use super::charge::{push_charged, reserve_charged};
use super::clip::{CLIP_BYTES, Clipped, text_prefix};
use super::order::ViewOrders;
use crate::statistics::account::Charge;
#[cfg(any(test, feature = "test-support"))]
use crate::statistics::account::Term;

/// A field PostgreSQL refuses — one its type's `*_in` refuses, met while
/// keying it ([`BoundsGatherer::observe`]), or a NULL its column's `NOT NULL`
/// refuses — or one a strict parse's check refuses.
#[derive(Debug)]
pub(super) struct Refused;

/// How a column's values are ordered while gathering.
#[derive(Clone)]
enum Order {
    /// By the text itself, bytewise, once put in [`Canonical`] form: never
    /// keyed, since a key copies the whole value and a value may be hundreds
    /// of megabytes, and a bound past [`DICTIONARY_ENTRY_MAX_BYTES`] is truncated.
    Bytewise(Canonical),
    /// By [`ValueKey::stored_order`]: the key a filter orders by, a float's
    /// zeros told apart as Arrow's order tells them. A value past
    /// [`DICTIONARY_ENTRY_MAX_BYTES`] leaves its group unbounded and its block unordered,
    /// no truncation of it being a bound.
    Keyed(CompareKind),
}

/// The bytewise kinds, each by the text whose bytes order as its key does.
/// A text's `length` is its typmod's, past which a field is refused
/// ([`char_typmod_refuses`]) and which places none.
#[derive(Clone, Copy)]
enum Canonical {
    /// `text`, `varchar`, `name`, bounded bytewise in any collation: the text.
    Text { length: Option<u32> },
    /// `character`: the text without its trailing blanks, which its comparison
    /// ignores — so a bound is stored unpadded.
    PaddedText { length: Option<u32> },
    /// `bytea`: `\x` and lowercase hex pairs, which order as the bytes do —
    /// an `escape` value's head put in that form, as much as a bound reads
    /// ([`BYTEA_ESCAPE_HEAD_BYTES`]). A value in any other spelling is not
    /// placed.
    Bytea,
}

impl Canonical {
    /// The text whose bytes order `text` among its column's values, `None`
    /// where this kind cannot place it.
    fn of(self, text: &str) -> Option<Cow<'_, str>> {
        match self {
            Self::Text { .. } => Some(Cow::Borrowed(text)),
            Self::PaddedText { .. } => Some(Cow::Borrowed(text.trim_end_matches(' '))),
            Self::Bytea => match text.strip_prefix("\\x") {
                Some(hex) => (hex.len() % 2 == 0
                    && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
                .then_some(Cow::Borrowed(text)),
                None => decode_bytea_escape(text, BYTEA_ESCAPE_HEAD_BYTES)
                    .map(|head| Cow::Owned(render_bytea(&head))),
            },
        }
    }
}

/// How many of an `escape` value's bytes [`Canonical::of`] renders: enough
/// that a value cut there renders past [`CLIP_BYTES`], so its head is the
/// whole value's and it reads as no whole value, as the value itself does
/// (`docs/design/decisions.md`, "D76").
const BYTEA_ESCAPE_HEAD_BYTES: usize = CLIP_BYTES / 2;

pub(super) struct BoundsGatherer {
    order: Order,
    /// Per closed group, **as a merge needs it rather than as it is stored**:
    /// a keyed group's stored bounds, and a bytewise group's two extremes'
    /// heads ([`Clipped`]), `min_exact` and `max_exact` saying whether each
    /// head is its whole value — stored bounds only once [`Self::finish`]
    /// clips them, since a clipped upper bound no longer orders as its value
    /// does (`docs/design/decisions.md`, "D82"). **Over the values every
    /// layer's type holds**: a value past a tier is kept apart, in
    /// [`Self::tiers`].
    groups: Vec<Option<Bounds>>,
    /// Per closed group, [`LOST`].
    flags: Vec<u8>,
    /// Per closed group, its values past each tier as `groups` and `flags`
    /// hold them, `None` for a group holding none — **empty until a group
    /// holds one**, then one per closed group, so a column never meeting such
    /// a value pays nothing for the views (`docs/design/decisions.md`, "D97").
    tiers: Vec<Option<Box<ClosedTiers>>>,
    /// The text every entry of `groups` and `tiers` holds, summed as they are
    /// pushed, and each boxed entry of `tiers`.
    stored: u64,
    rows: ViewOrders,
}

/// One closed group's values past each tier, in [`tier_slot`] order, as
/// [`BoundsGatherer::groups`] and [`BoundsGatherer::flags`] hold a group's:
/// `(None, 0)` for a tier it holds no value past.
type ClosedTiers = [(Option<Bounds>, u8); 2];

/// A closed group held a value its bounds could not cover, so it has none —
/// where a group with `None` and no flag held no non-NULL value.
const LOST: u8 = 1;

/// Where a tier's extremes sit among a group's: the engine's, then the
/// format spec's.
fn tier_slot(tier: UnrepresentableTier) -> usize {
    match tier {
        UnrepresentableTier::Engine => 0,
        UnrepresentableTier::Format => 1,
    }
}

/// One group's running bounds.
pub(super) enum GroupBounds {
    Bytewise { min: Option<Clipped>, max: Option<Clipped>, lost: bool },
    Keyed(KeyedGroup),
}

/// A keyed group's running bounds: over the values every layer holds, and
/// apart from them each tier's, which only a group holding one allocates.
#[derive(Default)]
pub(super) struct KeyedGroup {
    values: Extremes,
    tiers: Option<Box<[Extremes; 2]>>,
}

/// The extremes of some keyed values, each with its text.
#[derive(Default)]
struct Extremes {
    min: Option<(ValueKey, String)>,
    max: Option<(ValueKey, String)>,
    lost: bool,
}

impl Extremes {
    fn observe(&mut self, key: &ValueKey, text: &str) {
        if self.min.as_ref().is_none_or(|(m, _)| key.stored_order(m) == Ordering::Less) {
            self.min = Some((key.clone(), text.to_owned()));
        }
        if self.max.as_ref().is_none_or(|(m, _)| key.stored_order(m) == Ordering::Greater) {
            self.max = Some((key.clone(), text.to_owned()));
        }
    }

    /// Fold `later`, the extremes of the values following these. The earliest
    /// of equal keys is kept, as a pass does.
    fn absorb(&mut self, later: Extremes) {
        self.lost |= later.lost;
        if let Some(value) = later.min
            && self.min.as_ref().is_none_or(|(m, _)| value.0.stored_order(m) == Ordering::Less)
        {
            self.min = Some(value);
        }
        if let Some(value) = later.max
            && self.max.as_ref().is_none_or(|(m, _)| value.0.stored_order(m) == Ordering::Greater)
        {
            self.max = Some(value);
        }
    }

    fn heap_bytes(&self) -> u64 {
        [&self.min, &self.max]
            .into_iter()
            .flatten()
            .map(|(key, text)| key.heap_bytes() + text_heap(text))
            .sum()
    }

    fn closed(self) -> (Option<Bounds>, u8) {
        match self {
            Extremes { lost: false, min: Some((_, min)), max: Some((_, max)) } => {
                (Some(Bounds { min, max, min_exact: true, max_exact: true }), 0)
            }
            Extremes { lost, .. } => (None, if lost { LOST } else { 0 }),
        }
    }

    /// Closed extremes as they stood before they closed.
    fn reopen(kind: &CompareKind, bounds: Option<Bounds>, flags: u8) -> Self {
        match bounds {
            Some(Bounds { min, max, .. }) => {
                let key = |text: &str| ValueKey::of(kind, text).expect("a stored bound keyed");
                Extremes { min: Some((key(&min), min)), max: Some((key(&max), max)), lost: false }
            }
            None => Extremes { lost: flags & LOST != 0, ..Extremes::default() },
        }
    }
}

impl KeyedGroup {
    /// The extremes a value past `tier` joins — a value no tier's joining
    /// [`Self::values`].
    fn extremes(&mut self, tier: Option<UnrepresentableTier>) -> &mut Extremes {
        match tier {
            None => &mut self.values,
            Some(tier) => &mut self.tiers.get_or_insert_with(Box::default)[tier_slot(tier)],
        }
    }

    fn absorb(&mut self, later: KeyedGroup) {
        self.values.absorb(later.values);
        let Some(theirs) = later.tiers else { return };
        match &mut self.tiers {
            Some(mine) => {
                let [engine, format] = *theirs;
                mine[0].absorb(engine);
                mine[1].absorb(format);
            }
            None => self.tiers = Some(theirs),
        }
    }

    pub(super) fn heap_bytes(&self) -> u64 {
        let tiers = self.tiers.as_ref().map_or(0, |tiers| {
            size_of::<[Extremes; 2]>() as u64 + tiers.iter().map(Extremes::heap_bytes).sum::<u64>()
        });
        self.values.heap_bytes() + tiers
    }
}

impl GroupBounds {
    /// Fold `later`, the same group's bounds over its following rows. Keyed
    /// extremes keep the earliest of equal keys, as a pass does; bytewise ones
    /// are placed by their heads alone ([`Clipped`]).
    pub(super) fn absorb(&mut self, later: GroupBounds) {
        match (self, later) {
            (
                Self::Bytewise { min, max, lost },
                Self::Bytewise { min: later_min, max: later_max, lost: later_lost },
            ) => {
                *lost |= later_lost;
                if let Some(value) = later_min
                    && min.as_ref().is_none_or(|m| m.order(&value) == Some(Ordering::Less))
                {
                    *min = Some(value);
                }
                if let Some(value) = later_max
                    && max.as_ref().is_none_or(|m| m.order(&value) == Some(Ordering::Greater))
                {
                    *max = Some(value);
                }
            }
            (Self::Keyed(mine), Self::Keyed(later)) => mine.absorb(later),
            _ => unreachable!("one column's groups keep one kind of bounds"),
        }
    }
}

impl BoundsGatherer {
    pub(super) fn new(kind: CompareKind) -> Self {
        let order = match kind {
            CompareKind::Text { length } => Order::Bytewise(Canonical::Text { length }),
            CompareKind::PaddedText { length } => Order::Bytewise(Canonical::PaddedText { length }),
            CompareKind::Bytea => Order::Bytewise(Canonical::Bytea),
            kind => Order::Keyed(kind),
        };
        Self::of(order)
    }

    fn of(order: Order) -> Self {
        Self {
            order,
            groups: Vec::new(),
            flags: Vec::new(),
            tiers: Vec::new(),
            stored: 0,
            rows: ViewOrders::default(),
        }
    }

    pub(super) fn fresh(&self) -> Self {
        Self::of(self.order.clone())
    }

    /// The heap this gatherer holds: its closed groups, the text they hold
    /// and its row order.
    pub(super) fn held(&self) -> u64 {
        vec_heap(&self.groups)
            + vec_heap(&self.flags)
            + vec_heap(&self.tiers)
            + self.stored
            + self.rows.heap_bytes()
    }

    /// The heap a row can change: the row order.
    pub(super) fn open_heap(&self) -> u64 {
        self.rows.heap_bytes()
    }

    /// Fold `later`'s row order, over the rows following these, into this
    /// one's, leaving `later`'s empty.
    pub(super) fn absorb_rows(&mut self, later: &mut BoundsGatherer) {
        self.rows.absorb(mem::take(&mut later.rows));
    }

    pub(super) fn fresh_group(&self) -> GroupBounds {
        match self.order {
            Order::Bytewise(_) => GroupBounds::Bytewise { min: None, max: None, lost: false },
            Order::Keyed(_) => GroupBounds::Keyed(KeyedGroup::default()),
        }
    }

    /// A value past `tier` that does not key, or that no stored bound could
    /// cover: no bounds for its group, and no order for its block, in each
    /// view that takes it.
    pub(super) fn lose_value(
        &mut self,
        group: &mut GroupBounds,
        tier: Option<UnrepresentableTier>,
    ) {
        match group {
            GroupBounds::Bytewise { lost, .. } => *lost = true,
            GroupBounds::Keyed(group) => group.extremes(tier).lost = true,
        }
        self.rows.lose(tier);
    }

    /// Observe `text`, a value past `tier` where it is past one — which only a
    /// keyed kind's column can hold. **A value its type's `*_in` refuses is
    /// [`Refused`]**, where keying it, placing a `bytea` or holding a text to
    /// its length finds that; no other value is, one this build cannot read
    /// losing the group's bounds and its block's order.
    pub(super) fn observe(
        &mut self,
        group: &mut GroupBounds,
        text: &str,
        tier: Option<UnrepresentableTier>,
    ) -> Result<(), Refused> {
        match &self.order {
            Order::Bytewise(canonical) => {
                debug_assert!(tier.is_none(), "a bytewise kind's type holds every value");
                let canonical = *canonical;
                if let Canonical::Text { length: Some(length) }
                | Canonical::PaddedText { length: Some(length) } = canonical
                    && char_typmod_refuses(text, length)
                {
                    self.lose_value(group, None);
                    return Err(Refused);
                }
                let Some(text) = canonical.of(text) else {
                    self.lose_value(group, None);
                    return match canonical {
                        Canonical::Bytea if bytea_unread(text) == Unread::Refused => Err(Refused),
                        _ => Ok(()),
                    };
                };
                let text = text.as_ref();
                let GroupBounds::Bytewise { min, max, .. } = group else {
                    unreachable!("a bytewise order keeps bytewise bounds")
                };
                if min.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Less)) {
                    *min = Some(Clipped::of(text));
                }
                if max.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Greater)) {
                    *max = Some(Clipped::of(text));
                }
                self.rows.place_bytewise(text);
            }
            Order::Keyed(kind) => {
                if text.len() > DICTIONARY_ENTRY_MAX_BYTES {
                    self.lose_value(group, tier);
                    return Ok(());
                }
                let key = match ValueKey::of_field(kind, text) {
                    Ok(key) => key,
                    Err(unread) => {
                        self.lose_value(group, tier);
                        return if unread == Unread::Refused { Err(Refused) } else { Ok(()) };
                    }
                };
                let GroupBounds::Keyed(group) = group else {
                    unreachable!("a keyed order keeps keyed bounds")
                };
                group.extremes(tier).observe(&key, text);
                self.rows.place_keyed(key, tier);
            }
        }
        Ok(())
    }

    pub(super) fn close_group(&mut self, group: GroupBounds, charge: &mut Charge) {
        let (bounds, flags, tiers) = closed(group);
        self.stored += closed_heap(&bounds) + tiers_heap(&tiers);
        push_charged(&mut self.groups, bounds, charge);
        push_charged(&mut self.flags, flags, charge);
        if tiers.is_some() || !self.tiers.is_empty() {
            let wanted = self.groups.len() - self.tiers.len();
            reserve_charged(&mut self.tiers, wanted, charge);
            self.tiers.resize_with(self.groups.len() - 1, || None);
            self.tiers.push(tiers);
        }
    }

    /// A closed group's running bounds again, as they stood when it closed.
    fn reopen(
        &self,
        bounds: Option<Bounds>,
        flags: u8,
        tiers: Option<Box<ClosedTiers>>,
    ) -> GroupBounds {
        match &self.order {
            Order::Bytewise(_) => match bounds {
                Some(Bounds { min, max, min_exact, max_exact }) => GroupBounds::Bytewise {
                    min: Some(Clipped { head: min, whole: min_exact }),
                    max: Some(Clipped { head: max, whole: max_exact }),
                    lost: false,
                },
                None => GroupBounds::Bytewise { min: None, max: None, lost: flags & LOST != 0 },
            },
            Order::Keyed(kind) => GroupBounds::Keyed(KeyedGroup {
                values: Extremes::reopen(kind, bounds, flags),
                tiers: tiers.map(|tiers| {
                    Box::new((*tiers).map(|(bounds, flags)| Extremes::reopen(kind, bounds, flags)))
                }),
            }),
        }
    }

    /// Closed group `index`, taken out of the lists, which keep a hole.
    fn take_closed(&mut self, index: usize) -> (Option<Bounds>, u8, Option<Box<ClosedTiers>>) {
        let tiers = self.tiers.get_mut(index).and_then(Option::take);
        (self.groups[index].take(), mem::take(&mut self.flags[index]), tiers)
    }

    /// The last closed group's running bounds, no longer listed.
    pub(super) fn reopen_last(&mut self) -> GroupBounds {
        let bounds = self.groups.pop().expect("a closed group lists its bounds");
        let flags = self.flags.pop().expect("a closed group flags its bounds");
        let tiers = self.tiers.pop().flatten();
        self.stored -= closed_heap(&bounds) + tiers_heap(&tiers);
        self.reopen(bounds, flags, tiers)
    }

    /// Merge every closed group with the one after it, as one pass over both
    /// groups' values closes them.
    pub(super) fn merge_pairs(&mut self) {
        let closed_groups = self.groups.len();
        let merged = closed_groups.div_ceil(2);
        let tiered = !self.tiers.is_empty();
        for j in 0..merged {
            let first = self.take_closed(2 * j);
            let (bounds, flags, tiers) = if 2 * j + 1 < closed_groups {
                let second = self.take_closed(2 * j + 1);
                let mut group = self.reopen(first.0, first.1, first.2);
                group.absorb(self.reopen(second.0, second.1, second.2));
                closed(group)
            } else {
                first
            };
            (self.groups[j], self.flags[j]) = (bounds, flags);
            if tiered {
                self.tiers[j] = tiers;
            }
        }
        self.groups.truncate(merged);
        self.flags.truncate(merged);
        if tiered {
            self.tiers.truncate(merged);
        }
        self.stored = self.groups.iter().map(closed_heap).sum::<u64>()
            + self.tiers.iter().map(tiers_heap).sum::<u64>();
    }

    /// Append `later`'s closed groups, which follow this gatherer's,
    /// **draining `later` in place**: its tiers first, then its groups and
    /// their flags, the text they hold moving with them. The caller moves
    /// the account by what the move changed.
    pub(super) fn append(&mut self, later: &mut BoundsGatherer, charge: &mut Charge) {
        self.append_tiers(later, charge);
        reserve_charged(&mut self.groups, later.groups.len(), charge);
        reserve_charged(&mut self.flags, later.flags.len(), charge);
        self.groups.append(&mut later.groups);
        self.flags.append(&mut later.flags);
        self.stored += mem::take(&mut later.stored);
        (later.groups, later.flags) = (Vec::new(), Vec::new());
    }

    /// Append `later`'s tiers, which follow this gatherer's, ahead of its
    /// groups: both lists keep one per closed group where either holds any.
    fn append_tiers(&mut self, later: &mut BoundsGatherer, charge: &mut Charge) {
        if self.tiers.is_empty() && later.tiers.is_empty() {
            return;
        }
        let total = self.groups.len() + later.groups.len();
        let wanted = total - self.tiers.len();
        reserve_charged(&mut self.tiers, wanted, charge);
        self.tiers.resize_with(self.groups.len(), || None);
        if later.tiers.is_empty() {
            self.tiers.resize_with(total, || None);
        } else {
            self.tiers.append(&mut later.tiers);
        }
        later.tiers = Vec::new();
    }

    pub(super) fn finish(self) -> ColumnBounds {
        let [sortedness, every_order, displayable_order] = self.rows.sortedness();
        let mut groups = self.groups;
        match &self.order {
            Order::Bytewise(canonical) => {
                for group in groups.iter_mut() {
                    if let Some(Bounds { min, max, min_exact, max_exact }) = group.take() {
                        let min = Clipped { head: min, whole: min_exact };
                        *group = clipped_bounds(
                            *canonical,
                            min,
                            Clipped { head: max, whole: max_exact },
                        );
                    }
                }
                ColumnBounds { sortedness, groups, every: None, displayable: None }
            }
            Order::Keyed(_) if self.tiers.is_empty() => {
                ColumnBounds { sortedness, groups, every: None, displayable: None }
            }
            Order::Keyed(kind) => {
                let seen =
                    |(bounds, flags): &(Option<Bounds>, u8)| bounds.is_some() || flags & LOST != 0;
                let (mut every, mut displayable) = (Vec::new(), Vec::new());
                let (mut any_every, mut any_displayable) = (false, false);
                let mut flags = self.flags.into_iter();
                for (group, tiers) in groups.iter_mut().zip(self.tiers) {
                    let base_flags = flags.next().expect("a closed group flags its bounds");
                    let Some(tiers) = tiers else {
                        every.push(None);
                        displayable.push(None);
                        continue;
                    };
                    let [engine, format] = *tiers;
                    let (engine_seen, format_seen) = (seen(&engine), seen(&format));
                    let values = group.take();
                    let mut representable = Extremes::reopen(kind, values.clone(), base_flags);
                    representable.absorb(Extremes::reopen(kind, engine.0, engine.1));
                    let (representable, representable_flags) = representable.closed();
                    displayable.push(values.filter(|_| engine_seen));
                    any_displayable |= engine_seen;
                    every.push(
                        format_seen
                            .then(|| {
                                let mut all = Extremes::reopen(
                                    kind,
                                    representable.clone(),
                                    representable_flags,
                                );
                                all.absorb(Extremes::reopen(kind, format.0, format.1));
                                all.closed().0
                            })
                            .flatten(),
                    );
                    any_every |= format_seen;
                    *group = representable;
                }
                let view = |any: bool, sortedness, groups| {
                    any.then_some(BoundsView { sortedness, groups })
                };
                ColumnBounds {
                    sortedness,
                    every: view(any_every, every_order, every),
                    displayable: view(any_displayable, displayable_order, displayable),
                    groups,
                }
            }
        }
    }
}

/// The text a closed group's bounds hold.
fn closed_heap(bounds: &Option<Bounds>) -> u64 {
    bounds.as_ref().map_or(0, |b| text_heap(&b.min) + text_heap(&b.max))
}

/// The heap a closed group's tiers hold: their box and their bounds' text.
fn tiers_heap(tiers: &Option<Box<ClosedTiers>>) -> u64 {
    tiers.as_ref().map_or(0, |tiers| {
        size_of::<ClosedTiers>() as u64
            + tiers.iter().map(|(bounds, _)| closed_heap(bounds)).sum::<u64>()
    })
}

/// The bounds one group of `values` is stored with under `kind`'s order, or
/// `None` where it keeps none — for a test outside this module that holds
/// stored bounds against another order.
#[cfg(any(test, feature = "test-support"))]
pub fn one_group_bounds(kind: CompareKind, values: &[&str]) -> Option<Bounds> {
    let mut gatherer = BoundsGatherer::new(kind);
    let mut group = gatherer.fresh_group();
    for value in values {
        gatherer.observe(&mut group, value, None).expect("a value the server wrote");
    }
    let mut charge = Charge::new(Arc::default(), Term::Gathering);
    gatherer.close_group(group, &mut charge);
    gatherer.finish().groups.pop().flatten()
}

/// A closed group's bounds as [`BoundsGatherer::groups`],
/// [`BoundsGatherer::flags`] and [`BoundsGatherer::tiers`] hold them.
fn closed(group: GroupBounds) -> (Option<Bounds>, u8, Option<Box<ClosedTiers>>) {
    match group {
        GroupBounds::Bytewise { lost: false, min: Some(min), max: Some(max) } => {
            let (min_exact, max_exact) = (min.whole, max.whole);
            (Some(Bounds { min: min.head, max: max.head, min_exact, max_exact }), 0, None)
        }
        GroupBounds::Bytewise { lost, .. } => (None, if lost { LOST } else { 0 }, None),
        GroupBounds::Keyed(KeyedGroup { values, tiers }) => {
            let (bounds, flags) = values.closed();
            (bounds, flags, tiers.map(|tiers| Box::new((*tiers).map(Extremes::closed))))
        }
    }
}

/// Merge every entry of `items` with the one after it, a last one alone, in
/// place.
pub(super) fn merge_adjacent<T: Default>(items: &mut Vec<T>, mut merge: impl FnMut(T, T) -> T) {
    let len = items.len();
    for j in 0..len.div_ceil(2) {
        let first = mem::take(&mut items[2 * j]);
        let pair = match items.get_mut(2 * j + 1) {
            Some(second) => merge(first, mem::take(second)),
            None => first,
        };
        items[j] = pair;
    }
    items.truncate(len.div_ceil(2));
}

/// A bytewise group's bounds as stored: each exact where its value fits the
/// cap, otherwise a prefix below and a successor above.
fn clipped_bounds(canonical: Canonical, min: Clipped, max: Clipped) -> Option<Bounds> {
    let fits = |c: &Clipped| c.whole && c.head.len() <= DICTIONARY_ENTRY_MAX_BYTES;
    let min_exact = fits(&min);
    let lower = if min_exact {
        min.head
    } else {
        text_prefix(&min.head, DICTIONARY_ENTRY_MAX_BYTES).to_owned()
    };
    if fits(&max) {
        return Some(Bounds { min: lower, max: max.head, min_exact, max_exact: true });
    }
    let upper = match canonical {
        Canonical::Text { .. } => text_upper(&max.head, false)?,
        Canonical::PaddedText { .. } => text_upper(&max.head, true)?,
        Canonical::Bytea => bytea_upper(&max.head)?,
    };
    Some(Bounds { min: lower, max: upper, min_exact, max_exact: false })
}

/// A text no longer than [`DICTIONARY_ENTRY_MAX_BYTES`] ordering above `text` bytewise:
/// a prefix with its last character replaced by the next one. UTF-8 orders
/// bytewise as it orders code points, so the successor is above every text
/// the prefix begins. `padded` is `character`, whose comparison drops trailing
/// blanks first: the prefix gives its own up, and a successor that is a blank
/// is skipped. `None` when no character of the prefix has a successor.
fn text_upper(text: &str, padded: bool) -> Option<String> {
    // Room for a successor one byte longer than the character it replaces.
    let mut prefix: String =
        text_prefix(text, DICTIONARY_ENTRY_MAX_BYTES - char::MAX.len_utf8() + 1).to_owned();
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
const BYTEA_CAP_BYTES: usize = (DICTIONARY_ENTRY_MAX_BYTES - 2) / 2;

/// A rendered `bytea` above the value: its prefix with trailing `0xFF` bytes
/// dropped and the last byte incremented. `None` when every byte of that
/// prefix is `0xFF`.
fn bytea_upper(text: &str) -> Option<String> {
    let mut bytes = decode_bytea(text)?;
    bytes.truncate(BYTEA_CAP_BYTES);
    while bytes.last() == Some(&0xFF) {
        bytes.pop();
    }
    *bytes.last_mut()? += 1;
    Some(render_bytea(&bytes))
}

#[cfg(test)]
mod tests;
