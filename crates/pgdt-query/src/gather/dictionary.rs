//! A column's per-group dictionary of the text it emits
//! (`docs/design/decisions.md`, "D34").

use std::collections::HashMap;
use std::mem::{self, size_of};

use pgdt_codec::pgtype::CompareKind;
use pgdt_model::{
    ColumnDictionary, DICTIONARY_ENTRY_MAX_BYTES, DICTIONARY_MAX_ENTRIES,
    internal::{text_heap, vec_heap},
};

use super::charge::{grown_map_heap, map_heap, push_charged};
use super::column::GroupState;
use crate::statistics::{Charge, STATISTICS_ACCOUNT_CHARGE_STEP};

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
    pub(super) groups: Vec<Option<Vec<u32>>>,
    /// The heap every group's index list holds, summed as each is pushed.
    pub(super) indices: u64,
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
    pub(super) fn renumber(&mut self, charge: &mut Charge) {
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
pub(super) fn union_indices(first: Option<Vec<u32>>, second: Option<Vec<u32>>) -> Option<Vec<u32>> {
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
