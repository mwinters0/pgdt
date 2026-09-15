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
//!
//! **A leader piece gathers into an observer of its own, and the pieces join
//! in file order into exactly what one observer handed every row gathers**
//! ([`Gatherer::join`]). Two things cross a join: the group a cut falls
//! inside, which a piece holds open rather than closing ([`Gatherer::head`]),
//! and each ordered column's first value, which the rows before the piece
//! place their last value against ([`RowOrder`]).
//!
//! **Every observer charges what it holds to the pass's
//! [`StatisticsAccount`]** (`docs/design/decisions.md`, "D81"), recomputed as
//! each column closes a group and as a piece is folded in: a block's observer
//! as [`Term::Gathering`], a piece's as [`Term::Pieces`], each dictionary's
//! interning map as [`Term::Interned`]. What is recomputed is O(1) a column
//! but for the open group's distinct texts, which [`DICTIONARY_CAP`] bounds.

use std::any::Any;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::mem::{self, align_of, size_of};
use std::sync::Arc;

use crate::copy::{CopyHeader, decode_field, split_fields};
use crate::decode::{decode_bytea, render_bytea};
use crate::instrument::StatisticsScope;
use crate::pgtype::{CompareKind, ComparisonPlan, NestedPlan};
use crate::preamble::{ColumnDef, DumpMetadata};
use crate::predicate::ValueKey;
use crate::resolve::{SchemaMode, resolve_columns};
use crate::statistics::{
    BlockObserver, BlockStatistics, Bounds, Charge, ColumnBounds, ColumnDictionary,
    ColumnStatistics, DICTIONARY_CAP, RowGroup, STORED_VALUE_CAP, Sortedness, StatisticsAccount,
    StatisticsRequest, Term, text_heap, vec_heap,
};

/// The observer for one block, or `None` when `request` tracks nothing in it,
/// charging what it holds to `account`.
///
/// Resolved against an empty census: the census moves only an array column,
/// and a nested column gets neither bounds nor a dictionary.
pub(crate) fn observer_for(
    request: &StatisticsRequest,
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    account: &Arc<StatisticsAccount>,
) -> Option<Box<dyn BlockObserver>> {
    let tracked = request.tracked_columns(header)?;
    Some(observer_tracking(&tracked, request.group_size(), header, metadata, database, account))
}

/// The observer for one block gathering `tracked`'s columns — positional to
/// `header` — at `group_size`: what [`observer_for`] builds from a request,
/// and what a back-fill builds from a
/// [`crate::statistics::StatisticsBackfill`].
pub(crate) fn observer_tracking(
    tracked: &[bool],
    group_size: u64,
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    account: &Arc<StatisticsAccount>,
) -> Box<dyn BlockObserver> {
    let _attributed = StatisticsScope::enter();
    let qualified = header.qualified_name();
    let resolved =
        resolve_columns(&qualified, &header.columns, metadata, database, SchemaMode::Typed, &[]);
    let declared = declared_columns(metadata, database, &qualified);
    let columns: Vec<Option<ColumnGatherer>> = header
        .columns
        .iter()
        .enumerate()
        .map(|(i, name)| {
            tracked.get(i).is_some_and(|&t| t).then(|| {
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
    let charge = Charge::new(Arc::clone(account), Term::Gathering);
    let mut gatherer = Gatherer::block(group_size, columns, charge);
    gatherer.charge_held();
    Box::new(gatherer)
}

/// The columns `metadata` declares for the table `qualified` in `database` —
/// what a column's statistics record their declared type and collation from,
/// and what a query compares those against (`crate::prune`).
pub(crate) fn declared_columns<'m>(
    metadata: Option<&'m DumpMetadata>,
    database: Option<&str>,
    qualified: &str,
) -> Option<&'m [ColumnDef]> {
    metadata
        .and_then(|m| m.databases.iter().find(|db| db.name.as_deref() == database))
        .and_then(|db| db.tables.get(qualified))
        .map(Vec::as_slice)
}

/// The group a row is being added to.
struct OpenGroup {
    index: u64,
    first_start: u64,
    rows: u64,
}

struct Gatherer {
    group_size: u64,
    /// Whether this observes a piece of its block rather than the whole of it:
    /// a piece lists no group ahead of its first row's, holds that group open
    /// once a row moves past it, and is joined rather than finished.
    piece: bool,
    /// Every closed group in order, from the block's first — or, in a piece,
    /// from the one after [`Self::head`].
    groups: Vec<RowGroup>,
    /// A piece's first group once a later row has moved past it, with where
    /// its last row ends. **Held open, not closed**: the rows before the piece
    /// may have begun it, and only [`Gatherer::join`] knows. Each column's
    /// state over it is [`ColumnGatherer::head`].
    head: Option<(OpenGroup, u64)>,
    open: Option<OpenGroup>,
    /// Whether any column is tracked, so a row is worth splitting.
    splits: bool,
    columns: Vec<Option<ColumnGatherer>>,
    /// What this observer holds, in the pass's account. **Last**, so it is
    /// released after everything above is freed ([`Charge`]).
    charge: Charge,
}

impl Gatherer {
    fn block(group_size: u64, columns: Vec<Option<ColumnGatherer>>, charge: Charge) -> Self {
        Self {
            group_size,
            piece: false,
            groups: Vec::new(),
            head: None,
            open: None,
            splits: columns.iter().any(Option::is_some),
            columns,
            charge,
        }
    }

    /// The heap this observer holds, as its structure and its interning maps.
    fn held(&self) -> (u64, u64) {
        let own = vec_heap(&self.groups) + vec_heap(&self.columns);
        self.columns
            .iter()
            .flatten()
            .map(ColumnGatherer::held)
            .fold((own, 0), |(structure, interned), (s, i)| (structure + s, interned + i))
    }

    /// Charge what this observer holds now to the account.
    fn charge_held(&mut self) {
        let (structure, interned) = self.held();
        self.charge.set(structure, interned);
    }

    /// Close the open group, its last row's line ending at `end`, and list an
    /// empty group for every index short of `next`.
    fn close_through(&mut self, end: u64, next: u64) {
        let first = match self.open.take() {
            Some(group) => {
                let after = group.index + 1;
                if self.piece && self.head.is_none() {
                    for column in self.columns.iter_mut().flatten() {
                        column.hold_head();
                    }
                    self.head = Some((group, end));
                } else {
                    self.push_closed(group, end);
                }
                after
            }
            // A piece before its first row lists nothing ahead of it.
            None if self.piece => next,
            None => 0,
        };
        for _ in first..next {
            self.groups.push(RowGroup { rows: 0, bytes: 0 });
            for column in self.columns.iter_mut().flatten() {
                column.close_group(&mut self.charge);
            }
        }
        self.charge_held();
    }

    fn push_closed(&mut self, group: OpenGroup, end: u64) {
        self.groups.push(RowGroup { rows: group.rows, bytes: end - group.first_start });
        for column in self.columns.iter_mut().flatten() {
            column.close_group(&mut self.charge);
        }
    }

    /// Fold `later`, a piece whose rows all follow this observer's in file
    /// order, into it — leaving it as it would be had it observed those rows
    /// itself. Its first group continues this one's open group where both hold
    /// the same index, and each ordered column steps from this observer's last
    /// value to the piece's first.
    ///
    /// **The piece's charge is released after this observer's is raised**, so
    /// the account never reads the moved groups as held by neither.
    fn join(&mut self, mut later: Gatherer) {
        let placeholder = Charge::new(Arc::clone(later.charge.account()), Term::Pieces);
        let released = mem::replace(&mut later.charge, placeholder);
        self.join_rows(later);
        self.charge_held();
        drop(released);
    }

    /// [`Self::join`] but for the account.
    fn join_rows(&mut self, mut later: Gatherer) {
        debug_assert!(!self.piece && later.piece, "a block observer joins its pieces");
        let (first, first_end) = match (later.head.take(), later.open.take()) {
            (Some((group, end)), open) => {
                later.open = open;
                (group, Some(end))
            }
            (None, Some(group)) => (group, None),
            // The piece held no row.
            (None, None) => return,
        };
        let continues = self.open.as_ref().is_some_and(|open| open.index == first.index);
        match &mut self.open {
            Some(open) if continues => open.rows += first.rows,
            _ => {
                self.close_through(first.first_start, first.index);
                self.open = Some(first);
            }
        }
        for (mine, theirs) in self.columns.iter_mut().zip(&mut later.columns) {
            let (Some(mine), Some(theirs)) = (mine, theirs) else { continue };
            let state = match first_end {
                Some(_) => theirs.head.take().expect("a piece past its first group holds it"),
                None => theirs.take_group(),
            };
            if continues {
                mine.group.absorb(state);
            } else {
                mine.group = state;
            }
            if let (Some(mine), Some(theirs)) = (&mut mine.bounds, &mut theirs.bounds) {
                mine.rows.absorb(mem::take(&mut theirs.rows));
            }
        }
        // The piece moved past its first group, so the group closes where the
        // piece's next row starts and the piece's own closed groups follow.
        let Some(end) = first_end else { return };
        let open = self.open.take().expect("the joined group is open");
        self.push_closed(open, end);
        self.groups.append(&mut later.groups);
        self.open = later.open.take();
        for (mine, theirs) in self.columns.iter_mut().zip(mem::take(&mut later.columns)) {
            if let (Some(mine), Some(theirs)) = (mine, theirs) {
                mine.append(theirs, &mut self.charge);
            }
        }
    }
}

/// Under the instrument, an observer's fields are dropped inside a statistics
/// scope, so what it frees is attributed as what it allocated was
/// (`crate::instrument`); its charge, declared last, is released after.
#[cfg(feature = "introspect")]
impl Drop for Gatherer {
    fn drop(&mut self) {
        let _attributed = StatisticsScope::enter();
        drop(mem::take(&mut self.columns));
        drop(mem::take(&mut self.groups));
        (self.head, self.open) = (None, None);
    }
}

impl BlockObserver for Gatherer {
    fn observe_row(&mut self, offset: u64, raw: &[u8]) {
        let _attributed = StatisticsScope::enter();
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

    /// **The observer's charge becomes the block's retained bytes** once the
    /// interning maps are freed, in one update of the account.
    fn finish(mut self: Box<Self>, end: u64) -> BlockStatistics {
        let _attributed = StatisticsScope::enter();
        debug_assert!(!self.piece, "a piece is joined, never finished");
        if let Some(group) = &self.open {
            let next = group.index + 1;
            self.close_through(end, next);
        }
        let columns = mem::take(&mut self.columns);
        let statistics = BlockStatistics {
            group_size: self.group_size,
            groups: mem::take(&mut self.groups),
            columns: columns.into_iter().map(|c| c.map(ColumnGatherer::finish)).collect(),
        };
        self.charge.retain(statistics.heap_bytes());
        statistics
    }

    fn piece(&self) -> Box<dyn BlockObserver> {
        let _attributed = StatisticsScope::enter();
        let columns = self.columns.iter().map(|c| c.as_ref().map(ColumnGatherer::fresh)).collect();
        let charge = Charge::new(Arc::clone(self.charge.account()), Term::Pieces);
        let mut piece = Gatherer::block(self.group_size, columns, charge);
        piece.piece = true;
        piece.charge_held();
        Box::new(piece)
    }

    fn absorb(&mut self, later: Box<dyn BlockObserver>) {
        let _attributed = StatisticsScope::enter();
        let later = later.into_any().downcast::<Gatherer>().expect("a piece of this observer");
        self.join(*later);
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

struct ColumnGatherer {
    declared_type: Option<String>,
    collation: Option<String>,
    null_counts: Vec<u64>,
    bounds: Option<BoundsGatherer>,
    dictionary: Option<DictionaryGatherer>,
    /// The column over the open group.
    group: GroupState,
    /// The column over a piece's held first group ([`Gatherer::head`]).
    head: Option<GroupState>,
    /// The most heap an open group of this column held when it closed: what
    /// the account charges for the group still open, which it cannot measure
    /// row by row.
    open_peak: u64,
}

impl ColumnGatherer {
    /// Bounds where the comparison orders exactly, a dictionary where it
    /// equates exactly (`docs/design/decisions.md`, "D79").
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
        Self::with(declared_type, collation, bounds, dictionary)
    }

    fn with(
        declared_type: Option<String>,
        collation: Option<String>,
        bounds: Option<BoundsGatherer>,
        dictionary: Option<DictionaryGatherer>,
    ) -> Self {
        let group = GroupState::fresh(bounds.as_ref());
        Self {
            declared_type,
            collation,
            null_counts: Vec::new(),
            bounds,
            dictionary,
            group,
            head: None,
            open_peak: 0,
        }
    }

    /// The heap this column holds, as its structure and its interning map.
    /// The open group is charged at [`Self::open_peak`] or at what it holds
    /// now, whichever is more, and a held head at what it holds.
    fn held(&self) -> (u64, u64) {
        let named = self.declared_type.iter().chain(&self.collation).map(text_heap).sum::<u64>();
        let open = self.open_peak.max(self.group.heap_bytes());
        let head = self.head.as_ref().map_or(0, GroupState::heap_bytes);
        let mut structure = named + vec_heap(&self.null_counts) + open + head;
        if let Some(bounds) = &self.bounds {
            structure += vec_heap(&bounds.groups) + bounds.stored + bounds.rows.heap_bytes();
        }
        let mut interned = 0;
        if let Some(dictionary) = &self.dictionary {
            structure += vec_heap(&dictionary.entries)
                + dictionary.entry_text
                + vec_heap(&dictionary.groups)
                + dictionary.indices;
            interned =
                map_heap::<String, u32>(dictionary.interned.capacity()) + dictionary.entry_text;
        }
        (structure, interned)
    }

    /// A column of this one's kind that has gathered nothing: what a piece
    /// starts from.
    fn fresh(&self) -> Self {
        Self::with(
            self.declared_type.clone(),
            self.collation.clone(),
            self.bounds.as_ref().map(BoundsGatherer::fresh),
            self.dictionary.as_ref().map(DictionaryGatherer::fresh),
        )
    }

    /// The open group's state, leaving a fresh one open.
    fn take_group(&mut self) -> GroupState {
        let fresh = GroupState::fresh(self.bounds.as_ref());
        mem::replace(&mut self.group, fresh)
    }

    fn hold_head(&mut self) {
        self.head = Some(self.take_group());
    }

    fn observe(&mut self, field: &[u8]) {
        match decode_field(field) {
            Ok(None) => self.group.nulls += 1,
            Ok(Some(text)) => {
                if let (Some(bounds), Some(group)) = (&mut self.bounds, &mut self.group.bounds) {
                    bounds.observe(group, &text);
                }
                if let Some(dictionary) = &self.dictionary {
                    dictionary.observe(&mut self.group.texts, &text);
                }
            }
            // Not text at all, so neither a key nor an entry: the group can
            // cover the row with neither, and the block's order is lost.
            Err(_) => {
                if let (Some(bounds), Some(group)) = (&mut self.bounds, &mut self.group.bounds) {
                    bounds.lose_value(group);
                }
                self.group.texts = None;
            }
        }
    }

    /// Close the open group, and move what it changed onto `charge` before
    /// the next column closes: **a close's growth is charged column by
    /// column**, so a wide block's account never waits on all its columns.
    /// A dictionary's interning map growing here is charged as it grows
    /// (`DictionaryGatherer::intern`), which the column's own difference then
    /// takes in rather than adding twice.
    fn close_group(&mut self, charge: &mut Charge) {
        let before = self.held();
        let base = charge.charged();
        self.open_peak = self.open_peak.max(self.group.heap_bytes());
        let group = self.take_group();
        self.null_counts.push(group.nulls);
        if let (Some(bounds), Some(group)) = (&mut self.bounds, group.bounds) {
            bounds.close_group(group);
        }
        if let Some(dictionary) = &mut self.dictionary {
            dictionary.close_group(group.texts, charge);
        }
        let after = self.held();
        charge.set(base.0 + after.0 - before.0, base.1 + after.1 - before.1);
    }

    /// Append `later`'s closed groups, which follow this column's, and take
    /// its open group as this one's.
    fn append(&mut self, later: ColumnGatherer, charge: &mut Charge) {
        self.null_counts.extend(later.null_counts);
        if let (Some(mine), Some(theirs)) = (&mut self.bounds, later.bounds) {
            mine.groups.extend(theirs.groups);
            mine.stored += theirs.stored;
        }
        self.open_peak = self.open_peak.max(later.open_peak);
        if let (Some(mine), Some(theirs)) = (&mut self.dictionary, later.dictionary) {
            mine.append(theirs, charge);
        }
        self.group = later.group;
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

/// One column over one group while the group is open.
struct GroupState {
    nulls: u64,
    /// `None` for a column keeping no bounds.
    bounds: Option<GroupBounds>,
    /// The group's distinct texts in first-seen order, `None` once past a cap
    /// or at a field that is not text;
    /// read only for a column keeping a dictionary.
    texts: Option<Vec<String>>,
}

impl GroupState {
    fn fresh(bounds: Option<&BoundsGatherer>) -> Self {
        Self { nulls: 0, bounds: bounds.map(BoundsGatherer::fresh_group), texts: Some(Vec::new()) }
    }

    /// The heap this group's state holds: its distinct texts and its running
    /// bounds.
    fn heap_bytes(&self) -> u64 {
        let texts = self
            .texts
            .as_ref()
            .map_or(0, |texts| vec_heap(texts) + texts.iter().map(text_heap).sum::<u64>());
        let bounds = match &self.bounds {
            None => 0,
            Some(GroupBounds::Bytewise { min, max, .. }) => {
                [min, max].into_iter().flatten().map(|c| text_heap(&c.head)).sum()
            }
            Some(GroupBounds::Keyed { min, max, .. }) => [min, max]
                .into_iter()
                .flatten()
                .map(|(key, text)| key.heap_bytes() + text_heap(text))
                .sum(),
        };
        texts + bounds
    }

    /// Fold `later`, the same column over the same group's following rows.
    /// The texts fold as the rows would have: each new one appended until a
    /// cap is passed.
    fn absorb(&mut self, later: GroupState) {
        self.nulls += later.nulls;
        if let (Some(mine), Some(theirs)) = (&mut self.bounds, later.bounds) {
            mine.absorb(theirs);
        }
        let Some(theirs) = later.texts else {
            self.texts = None;
            return;
        };
        let Some(mine) = &mut self.texts else { return };
        for text in theirs {
            if mine.contains(&text) {
                continue;
            }
            if mine.len() == DICTIONARY_CAP {
                self.texts = None;
                return;
            }
            mine.push(text);
        }
    }
}

/// How a column's values are ordered while gathering.
#[derive(Clone)]
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
/// taken from (`docs/design/decisions.md`, "D76").
///
/// **Two values whose heads cannot be told apart store the same bound.** A
/// head that is not its whole value is at least `CLIP_BYTES` less one
/// character long, so two values it cannot place against each other share
/// more bytes than [`clipped_bounds`] reads of either. That is what lets a
/// group's bounds be joined from two halves' heads ([`GroupBounds::absorb`])
/// and still store what one pass over the group's values stores: each keeps a
/// value no other value in its half is certainly beyond, and that value can
/// differ from the true extreme only by bytes no stored bound holds.
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
        self.locate_bytes(text.as_bytes(), text.len())
    }

    /// [`Self::locate`] for a text known by its length and its first
    /// [`CLIP_BYTES`] bytes (all of them, if it is shorter) — as much as the
    /// answer reads — so a piece's first value is placed exactly as a row is.
    fn locate_bytes(&self, prefix: &[u8], len: usize) -> Option<Ordering> {
        let head = self.head.as_bytes();
        let shared = head.len().min(len);
        match prefix[..shared].cmp(&head[..shared]) {
            Ordering::Equal if len > head.len() => self.whole.then_some(Ordering::Greater),
            Ordering::Equal if len == head.len() && self.whole => Some(Ordering::Equal),
            Ordering::Equal => Some(Ordering::Less),
            unequal => Some(unequal),
        }
    }

    /// Where `other`'s value orders against this one's, known only by their
    /// heads: `None` exactly where the shorter head agrees with the longer and
    /// is not its whole value, which [`Clipped`] says stores the same bound.
    fn order(&self, other: &Clipped) -> Option<Ordering> {
        let (mine, theirs) = (self.head.as_bytes(), other.head.as_bytes());
        let shared = mine.len().min(theirs.len());
        match theirs[..shared].cmp(&mine[..shared]) {
            Ordering::Equal => {}
            unequal => return Some(unequal),
        }
        match theirs.len().cmp(&mine.len()) {
            Ordering::Less => other.whole.then_some(Ordering::Less),
            Ordering::Greater => self.whole.then_some(Ordering::Greater),
            Ordering::Equal => match (self.whole, other.whole) {
                (true, true) => Some(Ordering::Equal),
                (true, false) => Some(Ordering::Greater),
                (false, true) => Some(Ordering::Less),
                (false, false) => None,
            },
        }
    }
}

struct BoundsGatherer {
    order: Order,
    groups: Vec<Option<Bounds>>,
    /// The text every bound in `groups` holds, summed as they are pushed.
    stored: u64,
    rows: RowOrder,
}

/// One group's running bounds.
enum GroupBounds {
    Bytewise { min: Option<Clipped>, max: Option<Clipped>, lost: bool },
    Keyed { min: Option<(ValueKey, String)>, max: Option<(ValueKey, String)>, lost: bool },
}

impl GroupBounds {
    /// Fold `later`, the same group's bounds over its following rows. Keyed
    /// extremes keep the earliest of equal keys, as a pass does; bytewise ones
    /// are placed by their heads alone ([`Clipped`]).
    fn absorb(&mut self, later: GroupBounds) {
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
            (
                Self::Keyed { min, max, lost },
                Self::Keyed { min: later_min, max: later_max, lost: later_lost },
            ) => {
                *lost |= later_lost;
                if let Some(value) = later_min
                    && min.as_ref().is_none_or(|(m, _)| value.0.compare(m) == Ordering::Less)
                {
                    *min = Some(value);
                }
                if let Some(value) = later_max
                    && max.as_ref().is_none_or(|(m, _)| value.0.compare(m) == Ordering::Greater)
                {
                    *max = Some(value);
                }
            }
            _ => unreachable!("one column's groups keep one kind of bounds"),
        }
    }
}

/// A column's row order over the rows observed so far: the block's
/// [`Sortedness`] once every row is in.
struct RowOrder {
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
enum FirstValue {
    /// Its length and its first [`CLIP_BYTES`] bytes, cut at a byte rather
    /// than a character: [`Clipped::locate_bytes`] reads no more.
    Bytewise {
        prefix: Vec<u8>,
        len: usize,
    },
    Keyed(ValueKey),
}

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

    /// Fold `later`, the order over the rows following these: one step across
    /// the join, then its own.
    fn absorb(&mut self, later: RowOrder) {
        if let (Some(previous), Some(first)) = (&self.previous, &later.first) {
            let step = match (previous, first) {
                (Previous::Bytewise(previous), FirstValue::Bytewise { prefix, len }) => {
                    previous.locate_bytes(prefix, *len)
                }
                (Previous::Keyed(previous), FirstValue::Keyed(first)) => {
                    Some(first.compare(previous))
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
}

impl BoundsGatherer {
    fn new(kind: CompareKind) -> Self {
        let order = match kind {
            CompareKind::Text => Order::Bytewise(Canonical::Text),
            CompareKind::PaddedText => Order::Bytewise(Canonical::PaddedText),
            CompareKind::Bytea => Order::Bytewise(Canonical::Bytea),
            kind => Order::Keyed(kind),
        };
        Self { order, groups: Vec::new(), stored: 0, rows: RowOrder::default() }
    }

    fn fresh(&self) -> Self {
        Self { order: self.order.clone(), groups: Vec::new(), stored: 0, rows: RowOrder::default() }
    }

    fn fresh_group(&self) -> GroupBounds {
        match self.order {
            Order::Bytewise(_) => GroupBounds::Bytewise { min: None, max: None, lost: false },
            Order::Keyed(_) => GroupBounds::Keyed { min: None, max: None, lost: false },
        }
    }

    /// A value that does not key, or that no stored bound could cover: no
    /// bounds for its group, and no order for its block.
    fn lose_value(&mut self, group: &mut GroupBounds) {
        match group {
            GroupBounds::Bytewise { lost, .. } | GroupBounds::Keyed { lost, .. } => *lost = true,
        }
        self.rows.lost = true;
    }

    fn observe(&mut self, group: &mut GroupBounds, text: &str) {
        match &self.order {
            Order::Bytewise(canonical) => {
                let Some(text) = canonical.of(text) else { return self.lose_value(group) };
                let GroupBounds::Bytewise { min, max, .. } = group else {
                    unreachable!("a bytewise order keeps bytewise bounds")
                };
                if min.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Less)) {
                    *min = Some(Clipped::of(text));
                }
                if max.as_ref().is_none_or(|m| m.locate(text) == Some(Ordering::Greater)) {
                    *max = Some(Clipped::of(text));
                }
                let step = match &self.rows.previous {
                    Some(Previous::Bytewise(previous)) => Some(previous.locate(text)),
                    Some(Previous::Keyed(_)) => unreachable!("a bytewise order places no key"),
                    None => {
                        let prefix = text.as_bytes()[..text.len().min(CLIP_BYTES)].to_vec();
                        self.rows.first = Some(FirstValue::Bytewise { prefix, len: text.len() });
                        None
                    }
                };
                self.rows.previous = Some(Previous::Bytewise(Clipped::of(text)));
                if let Some(step) = step {
                    self.rows.step(step);
                }
            }
            Order::Keyed(kind) => {
                if text.len() > STORED_VALUE_CAP {
                    return self.lose_value(group);
                }
                let Some(key) = ValueKey::of(kind, text) else { return self.lose_value(group) };
                let GroupBounds::Keyed { min, max, .. } = group else {
                    unreachable!("a keyed order keeps keyed bounds")
                };
                if min.as_ref().is_none_or(|(m, _)| key.compare(m) == Ordering::Less) {
                    *min = Some((key.clone(), text.to_owned()));
                }
                if max.as_ref().is_none_or(|(m, _)| key.compare(m) == Ordering::Greater) {
                    *max = Some((key.clone(), text.to_owned()));
                }
                let step = match &self.rows.previous {
                    Some(Previous::Keyed(previous)) => Some(key.compare(previous)),
                    Some(Previous::Bytewise(_)) => unreachable!("a keyed order places no head"),
                    None => {
                        self.rows.first = Some(FirstValue::Keyed(key.clone()));
                        None
                    }
                };
                self.rows.previous = Some(Previous::Keyed(key));
                if let Some(step) = step {
                    self.rows.step(Some(step));
                }
            }
        }
    }

    fn close_group(&mut self, group: GroupBounds) {
        let bounds = match group {
            GroupBounds::Bytewise { lost: false, min: Some(min), max: Some(max) } => {
                let Order::Bytewise(canonical) = self.order else { unreachable!() };
                clipped_bounds(canonical, &min, &max)
            }
            GroupBounds::Keyed { lost: false, min: Some((_, min)), max: Some((_, max)) } => {
                Some(Bounds { min, max, max_exact: true })
            }
            _ => None,
        };
        self.stored += bounds.as_ref().map_or(0, |b| text_heap(&b.min) + text_heap(&b.max));
        self.groups.push(bounds);
    }

    fn finish(self) -> ColumnBounds {
        let sortedness = if self.rows.lost {
            Sortedness::Unsorted
        } else if self.rows.never_decreased {
            Sortedness::Ascending
        } else if self.rows.never_increased {
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

/// The heap a `HashMap<K, V>` of `capacity` allocates, as the standard
/// library's table lays one out: a slot per bucket, then a control byte per
/// bucket and one probe group's worth past the end. The layout is the
/// toolchain's, not a guarantee (`docs/design/runtime-invariants.md`, "RT11").
fn map_heap<K, V>(capacity: usize) -> u64 {
    if capacity == 0 {
        return 0;
    }
    let buckets = if capacity < 8 { capacity + 1 } else { (capacity * 8 / 7).next_power_of_two() };
    table_heap::<K, V>(buckets)
}

/// The heap the table a full `HashMap<K, V>` of `capacity` grows into
/// allocates: the table sized for one more entry than it holds.
fn grown_map_heap<K, V>(capacity: usize) -> u64 {
    let wanted = capacity + 1;
    let buckets = if wanted < 15 {
        match wanted.max(3) {
            3 => 4,
            4..=7 => 8,
            _ => 16,
        }
    } else {
        (wanted * 8 / 7).next_power_of_two()
    };
    table_heap::<K, V>(buckets)
}

fn table_heap<K, V>(buckets: usize) -> u64 {
    let probe = if cfg!(any(target_arch = "x86", target_arch = "x86_64")) { 16 } else { 8 };
    let align = align_of::<(K, V)>().max(probe);
    let slots = (size_of::<(K, V)>() * buckets).next_multiple_of(align);
    (slots + buckets + probe) as u64
}

/// A column's dictionary. A `character` entry is its text without the
/// trailing blanks its comparison ignores, deduplicated and measured against
/// [`STORED_VALUE_CAP`] as such (`docs/design/decisions.md`, "D34").
struct DictionaryGatherer {
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
    fn new(kind: &CompareKind) -> Self {
        Self {
            padded: matches!(kind, CompareKind::PaddedText),
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    fn fresh(&self) -> Self {
        Self {
            padded: self.padded,
            entries: Vec::new(),
            entry_text: 0,
            interned: HashMap::new(),
            groups: Vec::new(),
            indices: 0,
        }
    }

    /// Add `text` to a group's distinct `texts`, which a cap passed leaves
    /// `None`.
    fn observe(&self, texts: &mut Option<Vec<String>>, text: &str) {
        let Some(group) = texts else { return };
        let text = if self.padded { text.trim_end_matches(' ') } else { text };
        if group.iter().any(|seen| seen == text) {
            return;
        }
        if text.len() > STORED_VALUE_CAP || group.len() == DICTIONARY_CAP {
            *texts = None;
            return;
        }
        group.push(text.to_owned());
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
        self.entries.push(text.to_owned());
        let capacity = self.interned.capacity();
        let grows = self.interned.len() == capacity;
        if grows {
            charge.adjust_interned(grown_map_heap::<String, u32>(capacity) as i64);
        }
        self.interned.insert(text.to_owned(), id);
        if grows {
            charge.adjust_interned(-(map_heap::<String, u32>(capacity) as i64));
        }
        self.entry_text += text.len() as u64;
        id
    }

    fn close_group(&mut self, texts: Option<Vec<String>>, charge: &mut Charge) {
        let indices: Option<Vec<u32>> =
            texts.map(|texts| texts.iter().map(|text| self.intern(text, charge)).collect());
        self.indices += indices.as_ref().map_or(0, vec_heap);
        self.groups.push(indices);
    }

    /// Append `later`'s groups, which follow this dictionary's, interning
    /// their entries here in the order a pass closing them would have.
    fn append(&mut self, later: DictionaryGatherer, charge: &mut Charge) {
        let DictionaryGatherer { entries, groups, .. } = later;
        for group in groups {
            let indices: Option<Vec<u32>> = group.map(|ids| {
                ids.iter().map(|&id| self.intern(&entries[id as usize], charge)).collect()
            });
            self.indices += indices.as_ref().map_or(0, vec_heap);
            self.groups.push(indices);
        }
    }

    fn finish(self) -> ColumnDictionary {
        ColumnDictionary { entries: self.entries, groups: self.groups }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::StatisticsTerms;

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
                let mut state = gatherer.fresh_group();
                for v in &all {
                    gatherer.observe(&mut state, v);
                }
                gatherer.close_group(state);
                let column = gatherer.finish();
                assert_ne!(column.sortedness, Sortedness::Descending, "{kind:?}: sorted input");
                continue;
            }
            for group in &groups {
                let mut state = gatherer.fresh_group();
                for v in group {
                    gatherer.observe(&mut state, v);
                }
                gatherer.close_group(state);
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

    /// The kinds the join test's columns are compared by, a bytewise kind of
    /// each canonical form and a keyed one.
    const JOIN_KINDS: [CompareKind; 4] =
        [CompareKind::Text, CompareKind::PaddedText, CompareKind::Bytea, CompareKind::Int];

    /// A block's columns: one per [`JOIN_KINDS`], each keeping bounds and a
    /// dictionary, and an untracked one.
    fn join_columns() -> Vec<Option<ColumnGatherer>> {
        let plan =
            |kind: &CompareKind| ComparisonPlan::Compared { kind: kind.clone(), divergence: None };
        JOIN_KINDS
            .iter()
            .map(|kind| Some(ColumnGatherer::new(None, None, &plan(kind), &NestedPlan::Scalar)))
            .chain([None])
            .collect()
    }

    /// One field as `COPY` writes it: a backslash doubled.
    fn escaped(text: &str) -> Vec<u8> {
        text.replace('\\', "\\\\").into_bytes()
    }

    /// **A block observed in pieces, joined in file order, gathers exactly what
    /// one observer handed every row gathers** — over random blocks whose
    /// groups straddle the cuts, whose bytewise values share heads past the
    /// cap, and which hold NULLs, values that do not key or are not text,
    /// dictionaries on both sides of their cap, and sorted columns a cut
    /// falls inside. The fixture sweep does not guard a join's order step, a
    /// block of `pg_dump` output rarely turning on it: this test does.
    #[test]
    fn pieces_joined_in_file_order_gather_what_one_pass_gathers() {
        let mut rng = Rng(0x0001_0105);
        let (mut straddles, mut ordered, mut inexact, mut dictionaries, mut overflowed) =
            (0, 0, 0, 0, 0);
        for round in 0..600 {
            // Every fourth round is short values in wide groups, so a
            // dictionary on each side of a cut can pass the count cap between them.
            let short = round % 4 == 1;
            let group_size =
                if short { 4096 } else { [16u64, 64, 256, 700, 4096][rng.below(5) as usize] };
            let sorted = round % 3 == 0;
            let rows = if short { 60 + rng.below(140) } else { rng.below(120) } as usize;
            // A small pool per column for some rounds, so dictionaries fit.
            let pool = 2 + rng.below(if round % 2 == 0 { 6 } else { 200 });
            let columns: Vec<Vec<Option<Vec<u8>>>> = JOIN_KINDS
                .iter()
                .map(|kind| {
                    let base = value(&mut rng, kind);
                    let values: Vec<String> = (0..pool)
                        .map(|_| {
                            let v = match kind {
                                CompareKind::Bytea if short => {
                                    render_bytea(&(rng.below(1 << 16) as u16).to_be_bytes())
                                }
                                _ if short => rng.below(1000).to_string(),
                                _ => value(&mut rng, kind),
                            };
                            match kind {
                                CompareKind::Text | CompareKind::PaddedText
                                    if rng.below(2) == 0 =>
                                {
                                    format!("{base}{v}")
                                }
                                _ => v,
                            }
                        })
                        .collect();
                    let mut drawn: Vec<String> =
                        (0..rows).map(|_| values[rng.below(pool) as usize].clone()).collect();
                    if sorted {
                        let key = |v: &String| ValueKey::of(kind, v).unwrap();
                        drawn.sort_by(|a, b| key(a).compare(&key(b)));
                        if rng.below(2) == 0 {
                            drawn.reverse();
                        }
                    }
                    drawn
                        .into_iter()
                        .map(|v| match rng.below(40) {
                            0..=4 => None,
                            5 if !sorted && !short => Some(vec![0xff, b'a']),
                            6 if !sorted && matches!(kind, CompareKind::Int) => Some(b"x".to_vec()),
                            6 if !sorted && matches!(kind, CompareKind::Bytea) => {
                                Some(escaped("\\xABC"))
                            }
                            _ => Some(escaped(&v)),
                        })
                        .collect()
                })
                .collect();
            let lines: Vec<Vec<u8>> = (0..rows)
                .map(|r| {
                    let mut line = Vec::new();
                    for column in &columns {
                        line.extend(column[r].clone().unwrap_or_else(|| b"\\N".to_vec()));
                        line.push(b'\t');
                    }
                    line.extend(b"untracked");
                    line
                })
                .collect();
            let mut offsets = Vec::with_capacity(rows);
            let mut end = 0u64;
            for line in &lines {
                offsets.push(end);
                end += line.len() as u64 + 1;
            }

            let serial_account = Arc::new(StatisticsAccount::default());
            let charge = Charge::new(Arc::clone(&serial_account), Term::Gathering);
            let mut serial = Gatherer::block(group_size, join_columns(), charge);
            for (line, &offset) in lines.iter().zip(&offsets) {
                serial.observe_row(offset, line);
            }
            let serial = Box::new(serial).finish(end);

            let cut_odds = 1 + rng.below(30);
            let account = Arc::new(StatisticsAccount::default());
            let charge = Charge::new(Arc::clone(&account), Term::Gathering);
            let mut block: Box<dyn BlockObserver> =
                Box::new(Gatherer::block(group_size, join_columns(), charge));
            let mut piece = block.piece();
            for (r, (line, &offset)) in lines.iter().zip(&offsets).enumerate() {
                if r > 0 && rng.below(cut_odds) == 0 {
                    block.absorb(mem::replace(&mut piece, block.piece()));
                    if rng.below(4) == 0 {
                        block.absorb(block.piece());
                    }
                    if offsets[r - 1] / group_size == offset / group_size {
                        straddles += 1;
                    }
                }
                piece.observe_row(offset, line);
            }
            block.absorb(piece);
            let joined = block.finish(end);
            assert_eq!(joined, serial, "round {round}");
            // Every observer's charge is released into the block it became,
            // pieces included, and nothing else is left in either account.
            for (account, statistics) in [(&serial_account, &serial), (&account, &joined)] {
                let retained = statistics.heap_bytes();
                let expected = StatisticsTerms { retained, ..StatisticsTerms::default() };
                assert_eq!(account.held().now, expected, "round {round}");
            }

            for column in joined.columns.iter().flatten() {
                let bounds = column.bounds.as_ref().unwrap();
                if bounds.sortedness != Sortedness::Unsorted && rows > 10 {
                    ordered += 1;
                }
                inexact += bounds.groups.iter().flatten().filter(|b| !b.max_exact).count();
                let dictionary = column.dictionary.as_ref().unwrap();
                dictionaries += dictionary.groups.iter().flatten().filter(|g| g.len() > 1).count();
                if short {
                    overflowed += dictionary.groups.iter().filter(|g| g.is_none()).count();
                }
            }
        }
        assert!(straddles > 500, "only {straddles} cuts fell inside a group");
        assert!(ordered > 200, "only {ordered} columns ordered");
        assert!(inexact > 500, "only {inexact} truncated upper bounds");
        assert!(dictionaries > 500, "only {dictionaries} dictionaries of several entries");
        assert!(overflowed > 50, "only {overflowed} dictionaries past their count cap");
    }

    /// **A full map grows into the table [`grown_map_heap`] charges**, and
    /// every table's size follows from its capacity as [`map_heap`] reads it:
    /// the arithmetic the account's interned term is, against the capacities
    /// the standard library's map actually reports as it fills. That the
    /// arithmetic is the allocation is the instrument build's to show
    /// (`pgdump_query-cli/tests/statistics_account.rs`).
    #[test]
    fn a_full_map_grows_into_the_table_it_is_charged() {
        let mut map: HashMap<String, u32> = HashMap::new();
        let mut growths = 0;
        for i in 0..100_000u32 {
            let capacity = map.capacity();
            let full = map.len() == capacity;
            map.insert(i.to_string(), i);
            if full {
                growths += 1;
                assert_eq!(
                    map_heap::<String, u32>(map.capacity()),
                    grown_map_heap::<String, u32>(capacity)
                );
            } else {
                assert_eq!(map.capacity(), capacity, "a map grew before it was full");
            }
        }
        assert!(growths > 10, "only {growths} growths");
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
