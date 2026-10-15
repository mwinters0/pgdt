//! A column's row order over the rows observed, in each view of its values:
//! the block's [`Sortedness`] once every row is in, carried across a join by
//! its first value.

use std::cmp::Ordering;
use std::mem::size_of;

use pgdt_codec::key::ValueKey;
use pgdt_model::{
    Sortedness, UnrepresentableTier,
    internal::{text_heap, vec_heap},
};

use super::clip::{CLIP_BYTES, Clipped};

/// A column's row order over the rows observed so far: the block's
/// [`Sortedness`] once every row is in.
#[derive(Clone)]
pub(super) struct RowOrder {
    /// The first value placed, carried to a join as much as places it against
    /// the value before it.
    first: Option<FirstValue>,
    /// The last value placed.
    previous: Option<Previous>,
    never_decreased: bool,
    never_increased: bool,
    /// A value the order cannot place was seen.
    lost: bool,
}

/// A column's first value, kept for the join.
#[derive(Clone)]
enum FirstValue {
    /// Its length and its first `CLIP_BYTES` bytes, cut at a byte rather
    /// than a character: [`Clipped::locate_bytes`] reads no more.
    Bytewise {
        prefix: Vec<u8>,
        len: usize,
    },
    Keyed(ValueKey),
}

#[derive(Clone)]
enum Previous {
    Bytewise(Clipped),
    Keyed(ValueKey),
}

impl Default for RowOrder {
    fn default() -> Self {
        Self {
            first: None,
            previous: None,
            never_decreased: true,
            never_increased: true,
            lost: false,
        }
    }
}

impl RowOrder {
    /// The heap the first and the last value placed hold.
    fn heap_bytes(&self) -> u64 {
        let first = match &self.first {
            None => 0,
            Some(FirstValue::Bytewise { prefix, .. }) => vec_heap(prefix),
            Some(FirstValue::Keyed(key)) => key.heap_bytes(),
        };
        let previous = match &self.previous {
            None => 0,
            Some(Previous::Bytewise(clipped)) => text_heap(&clipped.head),
            Some(Previous::Keyed(key)) => key.heap_bytes(),
        };
        first + previous
    }

    /// Fold one step of the row order: `step` is where the value orders
    /// against the previous one, `None` where that is unknown.
    fn step(&mut self, step: Option<Ordering>) {
        match step {
            Some(Ordering::Less) => self.never_decreased = false,
            Some(Ordering::Greater) => self.never_increased = false,
            Some(Ordering::Equal) => {}
            None => self.lost = true,
        }
    }

    /// Place a bytewise value after the one placed before it.
    fn place_bytewise(&mut self, text: &str) {
        let step = match &self.previous {
            Some(Previous::Bytewise(previous)) => Some(previous.locate(text)),
            Some(Previous::Keyed(_)) => unreachable!("a bytewise order places no key"),
            None => {
                let prefix = text.as_bytes()[..text.len().min(CLIP_BYTES)].to_vec();
                self.first = Some(FirstValue::Bytewise { prefix, len: text.len() });
                None
            }
        };
        self.previous = Some(Previous::Bytewise(Clipped::of(text)));
        if let Some(step) = step {
            self.step(step);
        }
    }

    /// Place a keyed value after the one placed before it.
    fn place_keyed(&mut self, key: ValueKey) {
        let step = match &self.previous {
            Some(Previous::Keyed(previous)) => Some(key.stored_order(previous)),
            Some(Previous::Bytewise(_)) => unreachable!("a keyed order places no head"),
            None => {
                self.first = Some(FirstValue::Keyed(key.clone()));
                None
            }
        };
        self.previous = Some(Previous::Keyed(key));
        if let Some(step) = step {
            self.step(step);
        }
    }

    /// Fold `later`, the order over the rows following these: one step across
    /// the join, then its own.
    fn absorb(&mut self, later: RowOrder) {
        if let (Some(previous), Some(first)) = (&self.previous, &later.first) {
            let step = match (previous, first) {
                (Previous::Bytewise(previous), FirstValue::Bytewise { prefix, len }) => {
                    previous.locate_bytes(prefix, *len)
                }
                (Previous::Keyed(previous), FirstValue::Keyed(first)) => {
                    first.stored_order(previous)
                }
                _ => unreachable!("one column's values are ordered one way"),
            };
            self.step(step);
        }
        self.never_decreased &= later.never_decreased;
        self.never_increased &= later.never_increased;
        self.lost |= later.lost;
        if self.first.is_none() {
            self.first = later.first;
        }
        if later.previous.is_some() {
            self.previous = later.previous;
        }
    }

    fn sortedness(&self) -> Sortedness {
        if self.lost {
            Sortedness::Unsorted
        } else if self.never_decreased {
            Sortedness::Ascending
        } else if self.never_increased {
            Sortedness::Descending
        } else {
            Sortedness::Unsorted
        }
    }
}

/// A column's row order in each view of its values
/// (`pgdt_model::StatisticsView`): over the values every layer holds,
/// and — **once the rows observed hold a value past a tier, and not before** —
/// over those and the engine's tier, and over every value. Until then the
/// three are one order, which is what a fork copies.
#[derive(Default)]
pub(super) struct ViewOrders {
    plain: RowOrder,
    /// Over the values the format spec holds, then over every value.
    wider: Option<Box<[RowOrder; 2]>>,
}

impl ViewOrders {
    fn fork(&mut self) -> &mut [RowOrder; 2] {
        let plain = &self.plain;
        self.wider.get_or_insert_with(|| Box::new([plain.clone(), plain.clone()]))
    }

    /// Place `text`, a bytewise kind's canonical value: its type holds every
    /// value, so the plain order is the only one.
    pub(super) fn place_bytewise(&mut self, text: &str) {
        self.plain.place_bytewise(text);
    }

    /// Place `key`, a value past `tier`, in each order that takes it.
    pub(super) fn place_keyed(&mut self, key: ValueKey, tier: Option<UnrepresentableTier>) {
        match tier {
            None => {
                if let Some([representable, every]) = self.wider.as_deref_mut() {
                    representable.place_keyed(key.clone());
                    every.place_keyed(key.clone());
                }
                self.plain.place_keyed(key);
            }
            Some(UnrepresentableTier::Engine) => {
                let [representable, every] = self.fork();
                representable.place_keyed(key.clone());
                every.place_keyed(key);
            }
            Some(UnrepresentableTier::Format) => self.fork()[1].place_keyed(key),
        }
    }

    /// A value past `tier` no order that takes it can place.
    pub(super) fn lose(&mut self, tier: Option<UnrepresentableTier>) {
        match tier {
            None => {
                self.plain.lost = true;
                for order in self.wider.iter_mut().flat_map(|wider| wider.iter_mut()) {
                    order.lost = true;
                }
            }
            Some(UnrepresentableTier::Engine) => {
                for order in self.fork() {
                    order.lost = true;
                }
            }
            Some(UnrepresentableTier::Format) => self.fork()[1].lost = true,
        }
    }

    pub(super) fn absorb(&mut self, mut later: ViewOrders) {
        if self.wider.is_some() || later.wider.is_some() {
            self.fork();
            later.fork();
        }
        self.plain.absorb(later.plain);
        if let (Some(mine), Some(theirs)) = (&mut self.wider, later.wider) {
            let [representable, every] = *theirs;
            mine[0].absorb(representable);
            mine[1].absorb(every);
        }
    }

    pub(super) fn heap_bytes(&self) -> u64 {
        let wider = self.wider.as_ref().map_or(0, |wider| {
            size_of::<[RowOrder; 2]>() as u64 + wider.iter().map(RowOrder::heap_bytes).sum::<u64>()
        });
        self.plain.heap_bytes() + wider
    }

    /// Each view's sortedness: over the values the format spec holds, over
    /// every value, and over those the engine holds as well.
    pub(super) fn sortedness(&self) -> [Sortedness; 3] {
        let plain = self.plain.sortedness();
        match self.wider.as_deref() {
            None => [plain; 3],
            Some([representable, every]) => [representable.sortedness(), every.sortedness(), plain],
        }
    }
}
