//! What a resolved tree answers off a group's statistics: its verdict, every
//! value it could take over the rows of one group, read off the statistics
//! instead of the rows (`docs/design/decisions.md`, "D75"); and the terms a
//! sorted run's stop reads.

use pgdt_codec::compare::CompareKind;
use pgdt_codec::internal::{OrderKey, compare_keys, order_key};
use pgdt_model::Unrepresentable;

use super::resolve::{ResolvedExpr, ResolvedMembership, ResolvedTerm};
use super::{PredicateOp, Truth};

/// A set of [`Truth`] values — what the rows of one row group could make a
/// term or a tree evaluate to, read off the group's statistics instead of its
/// rows ([`ResolvedExpr::truths`]).
///
/// A set rather than a "may match" flag because of [`Expr::Not`](super::Expr::Not): a flag
/// saying `True` is impossible under a `Not` says nothing about whether
/// `False` is, which is what the negation turns into `True`. Carrying all
/// three keeps every node's answer an over-approximation its parent can
/// combine soundly, `Unknown` included (`docs/design/decisions.md`, "D54").
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TruthSet(u8);

/// The members, as a set: `{True, Unknown}`.
impl std::fmt::Debug for TruthSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.members()).finish()
    }
}

impl TruthSet {
    /// No value at all — the answer over a group no row starts in.
    pub(super) const EMPTY: Self = Self(0);
    /// `True` and `False`: a comparison over a non-NULL value it knows
    /// nothing more about.
    const TWO_VALUED: Self = Self(Self::bit(Truth::True) | Self::bit(Truth::False));
    const MEMBERS: [Truth; 3] = [Truth::True, Truth::False, Truth::Unknown];

    const fn bit(truth: Truth) -> u8 {
        match truth {
            Truth::True => 1,
            Truth::False => 2,
            Truth::Unknown => 4,
        }
    }

    pub(super) fn of(truth: Truth) -> Self {
        Self(Self::bit(truth))
    }

    /// Whether some row could evaluate to `truth`. A group the evaluator
    /// answers without [`Truth::True`] holds no row a filter keeps.
    pub(crate) fn contains(self, truth: Truth) -> bool {
        self.0 & Self::bit(truth) != 0
    }

    pub(super) fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    fn members(self) -> impl Iterator<Item = Truth> {
        Self::MEMBERS.into_iter().filter(move |t| self.contains(*t))
    }

    /// Kleene's `f` lifted to sets: every value `f` takes on a member of each.
    fn lift(self, other: Self, f: impl Fn(Truth, Truth) -> Truth) -> Self {
        self.members()
            .flat_map(|a| other.members().map(move |b| (a, b)))
            .fold(Self::EMPTY, |set, (a, b)| set.union(Self::of(f(a, b))))
    }

    fn and(self, other: Self) -> Self {
        self.lift(other, |a, b| match (a, b) {
            (Truth::False, _) | (_, Truth::False) => Truth::False,
            (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
            _ => Truth::True,
        })
    }

    fn or(self, other: Self) -> Self {
        self.lift(other, |a, b| match (a, b) {
            (Truth::True, _) | (_, Truth::True) => Truth::True,
            (Truth::Unknown, _) | (_, Truth::Unknown) => Truth::Unknown,
            _ => Truth::False,
        })
    }

    fn not(self) -> Self {
        self.members().fold(Self::EMPTY, |set, t| set.union(Self::of(t.not())))
    }

    fn from_flags(can_be_true: bool, can_be_false: bool) -> Self {
        let pick = |on: bool, truth| if on { Self::of(truth) } else { Self::EMPTY };
        pick(can_be_true, Truth::True).union(pick(can_be_false, Truth::False))
    }
}

/// One row group's statistics, as the truth-set evaluator reads them. A
/// column is numbered the way a raw row's fields are — by the block's
/// **unprojected** column list, which is what [`ResolvedTerm`]'s index is.
///
/// Every statistic is optional per column: a column the group gathered
/// nothing for answers `None` to each, and every term over it answers what
/// any row might.
pub(crate) trait GroupStatistics {
    /// How many rows start in the group. Zero is a group no row starts in,
    /// over which every term is [`TruthSet::EMPTY`].
    fn rows(&self) -> u64;

    /// How many of those rows hold NULL in `column`.
    fn null_count(&self, column: usize) -> Option<u64>;

    /// A lower and an upper bound on `column`'s non-NULL values, as unescaped
    /// field text, from the stored set ordered as `kind` orders
    /// ([`pgdt_codec::compare::bounds_set_keyed_by`]): no value's key under `kind`
    /// is below `min`'s or above `max`'s.
    ///
    /// **A bound need not be a value the group holds**, and is never read as
    /// one: a bound truncated past a storage cap is read exactly as an exact
    /// one is, so it only has to be on the right side.
    fn bounds(&self, column: usize, kind: &CompareKind) -> Option<(&str, &str)>;

    /// Every distinct text among `column`'s non-NULL values, as unescaped
    /// field text — complete, or `None`.
    fn dictionary(&self, column: usize) -> Option<impl Iterator<Item = &str>>;

    /// How many of those rows hold, in `column`, a value its type cannot
    /// hold, per tier — `None` where the group did not count them.
    fn unrepresentable(&self, column: usize) -> Option<Unrepresentable>;
}

/// One row group's bounds on one column in one kind, read as keys once for
/// each run of terms reading them one after another — a membership's `=`
/// terms all read its column's, so a list is keyed against a group once
/// rather than once per value. It holds the last column and kind asked for, and lives no longer
/// than one group's answer, a group's bounds on a column in a kind being one
/// pair of texts.
#[derive(Default)]
struct KeyedBounds<'a> {
    /// The column and kind last asked for, `None` before the first ask.
    of: Option<(usize, &'a CompareKind)>,
    /// Their bounds as keys, `None` where either does not key.
    keys: Option<(OrderKey, OrderKey)>,
}

impl<'a> KeyedBounds<'a> {
    /// `min` and `max`, the group's bounds on `column` ordered as `kind`
    /// orders, as keys — `None` where either does not key.
    fn keys(
        &mut self,
        column: usize,
        kind: &'a CompareKind,
        min: &str,
        max: &str,
    ) -> Option<(&OrderKey, &OrderKey)> {
        if self.of != Some((column, kind)) {
            self.keys = order_key(kind, min).zip(order_key(kind, max));
            self.of = Some((column, kind));
        }
        self.keys.as_ref().map(|(low, high)| (low, high))
    }
}

impl ResolvedTerm {
    /// Every value this term could take over a row of `group` — a superset
    /// of what [`Self::eval`] answers over each of its rows, and exact where
    /// the statistics are. What each operator reads is
    /// `docs/design/decisions.md`, "D75".
    ///
    /// **A row whose value is not of the column's type is outside it**: the
    /// row path raises `Error::FieldDecode` there rather than answering, and
    /// no statistic records that such a row exists.
    pub(crate) fn truths(&self, group: &impl GroupStatistics) -> TruthSet {
        self.truths_keyed(group, &mut KeyedBounds::default())
    }

    /// [`Self::truths`], reading `group`'s bounds as keys out of `keyed`,
    /// which keys them once for every term of a tree reading them.
    fn truths_keyed<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        let rows = group.rows();
        // Off the group's own count, never its NULL count, which under the
        // null mode's view counts the values the test matches.
        if self.op.tests_unrepresentable() {
            let matched = match (&self.tested, group.unrepresentable(self.index)) {
                (None, _) => Some(0),
                (Some(test), Some(count)) => Some(test.counted(count)),
                (Some(_), None) => None,
            };
            let (some, all) = matched.map_or((rows > 0, false), |n| (n > 0, n >= rows));
            let (can_match, can_miss) = (some, rows > 0 && !all);
            return match self.op {
                PredicateOp::IsUnrepresentable => TruthSet::from_flags(can_match, can_miss),
                _ => TruthSet::from_flags(can_miss, can_match),
            };
        }
        let (nulls, values) = match group.null_count(self.index) {
            Some(nulls) => (nulls > 0, nulls < rows),
            None => (rows > 0, rows > 0),
        };
        let mut set = TruthSet::EMPTY;
        if nulls {
            set = set.union(TruthSet::of(self.eval_value(None).expect("a NULL always answers")));
        }
        if values {
            set = set.union(self.value_truths(group, keyed));
        }
        set
    }

    /// What this term could answer over a **non-NULL** value of `group`.
    ///
    /// The dictionary and the bounds each give a superset of the answer, so
    /// where the term reads both it takes what they agree on.
    fn value_truths<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        let Some(compared) = self.compared.as_ref() else {
            return TruthSet::of(Truth::of(self.op == PredicateOp::IsNotNull));
        };
        let mut set = TruthSet::TWO_VALUED;
        if compared.statistics.dictionary
            && let Some(entries) = group.dictionary(self.index)
        {
            // Through `eval_value`, the row path's own comparison, so an
            // entry is answered exactly as a row holding it is. An empty
            // dictionary beside a non-NULL row contradicts itself and is
            // read as none.
            let answered = entries
                .map(|entry| {
                    self.eval_value(Some(entry)).map_or(TruthSet::TWO_VALUED, TruthSet::of)
                })
                .reduce(TruthSet::union);
            if let Some(answered) = answered {
                set = set.intersection(answered);
            }
        }
        if let Some((kind, literal)) = compared.statistics.bounds.as_deref()
            && let Some((min, max)) = group.bounds(self.index, kind)
        {
            set = set.intersection(match keyed.keys(self.index, kind, min, max) {
                Some((low, high)) => self.bounded(literal, low, high),
                // A bound that does not key allows anything.
                None => TruthSet::TWO_VALUED,
            });
        }
        set
    }

    /// What bounds keyed as `low` and `high` allow this term to answer over
    /// a value between them.
    ///
    /// The four ordering operators are monotone in the value, so each end
    /// settles one truth. **The equality operators only ever rule out
    /// "equal"**, when the literal lies outside the bounds: that no value is
    /// *unequal* would need every value to be the literal, and bounds equal
    /// to it say so of keys, where a canonicalized comparison reads spellings
    /// — one per key only in text `*_out` wrote
    /// (`docs/design/decisions.md`, "D56").
    fn bounded(&self, literal: &OrderKey, low: &OrderKey, high: &OrderKey) -> TruthSet {
        let low = compare_keys(low, literal);
        let high = compare_keys(high, literal);
        let (can_be_true, can_be_false) = match self.op {
            PredicateOp::Lt(_) => (low.is_lt(), high.is_ge()),
            PredicateOp::Le(_) => (low.is_le(), high.is_gt()),
            PredicateOp::Gt(_) => (high.is_gt(), low.is_le()),
            PredicateOp::Ge(_) => (high.is_ge(), low.is_lt()),
            _ => {
                let can_be_equal = low.is_le() && high.is_ge();
                if self.wants_equal() { (can_be_equal, true) } else { (true, can_be_equal) }
            }
        };
        TruthSet::from_flags(can_be_true, can_be_false)
    }
}

impl ResolvedMembership {
    /// Every value this membership could take over a row of `group`: its
    /// terms' sets combined as [`ResolvedExpr::Or`] combines them, a NULL in
    /// the list adding `Unknown` over any group a row starts in — the answer
    /// the `Or` of `=` over the same list gives, not a tighter one. Tighter,
    /// though a row equals at most one value, would break the per-group
    /// agreement with the `Or` its generated check proves, for pruning
    /// nobody has measured; so only the row path answers by one lookup — a
    /// hash, or a binary search over the sorted keys, but for
    /// [`Lookup::Each`](super::resolve::Lookup::Each) — a group being answered term by term.
    #[cfg(test)]
    fn truths(&self, group: &impl GroupStatistics) -> TruthSet {
        self.truths_keyed(group, &mut KeyedBounds::default())
    }

    /// [`Self::truths`], every term reading the group's bounds out of
    /// `keyed`, so they are keyed once for the whole list. The sorted list is
    /// not searched within the bounds, which would stop the cost scaling with
    /// the list: what remains per term against the bounds is a key
    /// comparison, no parse. A dictionary is still answered per term through
    /// the row path, which parses each entry of a decoded kind.
    fn truths_keyed<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        let set = self
            .terms
            .iter()
            .fold(TruthSet::of(Truth::False), |set, t| set.or(t.truths_keyed(group, keyed)));
        match (self.null, group.rows()) {
            (false, _) => set,
            // `= NULL` over a group no row starts in is the empty set, as
            // every term is there, and the empty set absorbs a disjunction.
            (true, 0) => TruthSet::EMPTY,
            (true, _) => set.or(TruthSet::of(Truth::Unknown)),
        }
    }
}

impl ResolvedExpr {
    /// Every value this tree could take over a row of `group`, combining its
    /// terms' sets through Kleene's tables — so a group whose answer lacks
    /// [`Truth::True`] holds no row [`Self::matches`] keeps.
    ///
    /// Terms are combined as if independent, which is where the answer stops
    /// being exact: `v < 5 AND v >= 5` cannot be true of any one row, and
    /// each term alone can.
    pub(crate) fn truths(&self, group: &impl GroupStatistics) -> TruthSet {
        self.truths_keyed(group, &mut KeyedBounds::default())
    }

    /// [`Self::truths`], every term reading the group's bounds out of
    /// `keyed`, so terms reading a column's in one kind one after another
    /// key them once.
    fn truths_keyed<'a>(
        &'a self,
        group: &impl GroupStatistics,
        keyed: &mut KeyedBounds<'a>,
    ) -> TruthSet {
        match self {
            Self::Term(term) => term.truths_keyed(group, keyed),
            Self::In(membership) => membership.truths_keyed(group, keyed),
            Self::And(children) => children.iter().fold(TruthSet::of(Truth::True), |set, child| {
                set.and(child.truths_keyed(group, keyed))
            }),
            Self::Or(children) => children.iter().fold(TruthSet::of(Truth::False), |set, child| {
                set.or(child.truths_keyed(group, keyed))
            }),
            Self::Not(inner) => inner.truths_keyed(group, keyed).not(),
        }
    }

    /// The ordering terms every row this tree keeps must make `True` — the
    /// root when it is one, and each member of a conjunction at the root,
    /// conjunctions nested in it flattened — whose column's stored row order
    /// is ordered as the term's semantics orders it
    /// ([`BelievedStatistics::bounds`](super::resolve::BelievedStatistics::bounds)). A term
    /// beneath an `Or` or a `Not` is not one: a row can be kept while it is `False`.
    pub(crate) fn required_ordering_terms(&self) -> Vec<&ResolvedTerm> {
        let mut conjuncts = Vec::new();
        self.conjuncts(&mut conjuncts);
        conjuncts
            .into_iter()
            .filter_map(|conjunct| match conjunct {
                Self::Term(term)
                    if term.op.is_ordering()
                        && term
                            .compared
                            .as_ref()
                            .is_some_and(|c| c.statistics.bounds.is_some()) =>
                {
                    Some(term)
                }
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
