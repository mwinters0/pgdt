//! What one pass over a `COPY` block's rows records beside them — the
//! array-shape census and the unrepresentable count (`docs/design/decisions.md`,
//! "D35", "D96") — and the contracts over a block's rows the parsers call and a
//! crate above implements ([`FieldCount`], [`BlockObserver`]).

use serde::{Deserialize, Serialize};

use crate::{BlockGathered, CopyBlock, IgnoredRefusals};

/// The most dimensions PostgreSQL can give an array — `MAXDIM`, 6 in every
/// supported version (I25). A literal whose leading brace run is longer than
/// this did not come out of `array_out`, so a consumer must treat it as
/// unusable rather than as a depth.
pub const PG_ARRAY_MAX_DIMS: u8 = 6;

/// What one column's array values look like within one `COPY` block — the
/// shape census (`docs/design/decisions.md`, "D35").
///
/// Per column because an array's dimensionality belongs to the *value*
/// (I21), not to the DDL; per block because that is what the map extends
/// incrementally. Combining blocks is min-of-mins, max-of-maxes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArrayShape {
    /// Fewest and most dimensions any value in this column carried, as
    /// `(min, max)`. `None` until a value contributes one — a SQL NULL never
    /// does, and neither does `{}`, which is what `array_out` writes for an
    /// empty array of *any* dimensionality (I25) and so fits every depth.
    /// Both bounds, never the maximum alone (`docs/design/decisions.md`,
    /// "D35").
    pub dims: Option<(u8, u8)>,
    /// Whether any value carried an explicit `[lb:ub]=` lower-bound prefix.
    /// Kept apart from `dims` because it disqualifies a column on its own:
    /// Arrow lists are 0-based, with nowhere to record an index origin.
    pub lower_bound_prefix: bool,
}

impl ArrayShape {
    /// Fold another block's census for the same column into this one —
    /// min-of-mins, max-of-maxes, a lower-bound prefix anywhere counting
    /// everywhere. One table's data can occupy several blocks (I2).
    pub fn merge(&mut self, other: &ArrayShape) {
        self.dims = match (self.dims, other.dims) {
            (Some((a_min, a_max)), Some((b_min, b_max))) => {
                Some((a_min.min(b_min), a_max.max(b_max)))
            }
            (some, None) | (None, some) => some,
        };
        self.lower_bound_prefix |= other.lower_bound_prefix;
    }
}

/// What a crate above calls on [`ArrayShape`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait ArrayShapeExt {
    fn observe(&mut self, field: &[u8]);
}

impl ArrayShapeExt for ArrayShape {
    /// Fold one still-COPY-escaped field into this column's census. The
    /// field is not decoded first: neither `{` nor the `[lb:ub]=` prefix is
    /// in COPY TEXT's escape set (I15). A field that does not open with `{`,
    /// past any `[lb:ub]=` prefix, contributes nothing; one that does counts
    /// whatever its column's type, the census being type-blind (D35) and read
    /// only for a column resolved to an array.
    ///
    /// Dimensionality is the **leading brace run** (I25): `array_out` opens
    /// with exactly `ndim` braces and force-quotes any element containing a
    /// `{`, so no element can extend the run.
    fn observe(&mut self, field: &[u8]) {
        let mut rest = field;
        if rest.first() == Some(&b'[') {
            // `[lb:ub]…=` — the prefix runs to the `=` that introduces the
            // value proper. A prefix always has a real array after it
            // (`array_out` writes a bare `{}` before it formats dimensions);
            // anything else here is not array output and is left alone.
            let Some(eq) = rest.iter().position(|&b| b == b'=') else { return };
            if rest.get(eq + 1) != Some(&b'{') {
                return;
            }
            self.lower_bound_prefix = true;
            rest = &rest[eq + 1..];
        }
        if rest.first() != Some(&b'{') {
            return;
        }
        // The empty array fits any depth, so it constrains neither bound.
        if rest == b"{}" {
            return;
        }
        let depth = rest.iter().take_while(|&&b| b == b'{').count().min(u8::MAX as usize) as u8;
        let (min, max) = self.dims.unwrap_or((depth, depth));
        self.dims = Some((min.min(depth), max.max(depth)));
    }
}

/// Which limit a value is past, where its column's Arrow type cannot hold it
/// (`docs/design/decisions.md`, "D96").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum UnrepresentableTier {
    /// Within Arrow's format spec, and past the calendar [`calendar_end`]
    /// ends — a `date` or timestamp a DataFusion query cannot display, which
    /// only the provider reads as unrepresentable.
    Engine,
    /// Past what Arrow's format spec lets the type hold, which every layer
    /// reads: `±infinity`, `NaN` under a typmod, `time` `24:00:00`, an
    /// `interval` time part past nanoseconds, a timestamp past `i64`
    /// microseconds from 1970.
    Format,
}

/// One column's count of values its Arrow type cannot hold in one `COPY`
/// block, each value in the one tier it is past — the format spec's
/// dominating (`docs/design/decisions.md`, "D96"). Beside the array-shape
/// census, and at the same level: a block mapped at the metadata level holds
/// neither.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unrepresentable {
    /// Values past Arrow's format spec.
    pub format: u64,
    /// Values within it and past the engine's calendar.
    pub engine: u64,
}

impl Unrepresentable {
    /// Fold another block's, or a piece's, count for the same column in.
    pub fn merge(&mut self, other: &Unrepresentable) {
        self.format += other.format;
        self.engine += other.engine;
    }

    /// Whether no value was counted in either tier.
    pub fn is_zero(&self) -> bool {
        self.format == 0 && self.engine == 0
    }
}

/// What a crate above calls on [`Unrepresentable`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait UnrepresentableExt {
    fn add(&mut self, tier: UnrepresentableTier);
}

impl UnrepresentableExt for Unrepresentable {
    /// Count one value past `tier`.
    fn add(&mut self, tier: UnrepresentableTier) {
        match tier {
            UnrepresentableTier::Format => self.format += 1,
            UnrepresentableTier::Engine => self.engine += 1,
        }
    }
}

/// **The last day of the calendar `arrow-cast` formats a `date` or timestamp
/// through**, as a `Date32` day count from 1970 — `chrono`'s, whose end is the
/// engine tier's bound (RT21). A cache records the bound it counted under and,
/// holding a count, is refused by a build whose bound is another
/// (`pgdt_dump::cache::Unusable::CalendarChanged`), so a `chrono` upgrade moving
/// it reaches no count taken before it.
// upstream: UF2
pub fn calendar_end() -> i32 {
    let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("the Unix epoch is a date");
    let days = chrono::NaiveDate::MAX.signed_duration_since(epoch).num_days();
    i32::try_from(days).expect("chrono's calendar ends inside Date32's range")
}

/// What one pass over a block's rows records beside its rows' extent: the
/// array-shape census and the unrepresentable count, one entry each per
/// column in `header.columns` order — the data level's two query affordances
/// (`docs/design/decisions.md`, "D35", "D96"). The census grows for a row
/// wider than its header; the count never does, a column past the header
/// having no declared type to be tested against.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockCensus {
    pub shapes: Vec<ArrayShape>,
    pub unrepresentable: Vec<Unrepresentable>,
}

/// What a crate above calls on [`BlockCensus`] that is not its API: public only
/// because a crate boundary forced it, and reached through `internal`.
pub trait BlockCensusExt {
    fn new(columns: usize) -> Self;
    fn merge(&mut self, other: &BlockCensus);
}

impl BlockCensusExt for BlockCensus {
    /// Nothing folded yet, for a block `columns` wide.
    fn new(columns: usize) -> Self {
        BlockCensus {
            shapes: vec![ArrayShape::default(); columns],
            unrepresentable: vec![Unrepresentable::default(); columns],
        }
    }

    /// Fold another piece's, or another read's, census of the same block in —
    /// length-tolerant, since a row wider than its header grows the census.
    fn merge(&mut self, other: &BlockCensus) {
        if other.shapes.len() > self.shapes.len() {
            self.shapes.resize(other.shapes.len(), ArrayShape::default());
        }
        for (slot, shape) in self.shapes.iter_mut().zip(&other.shapes) {
            slot.merge(shape);
        }
        if other.unrepresentable.len() > self.unrepresentable.len() {
            self.unrepresentable.resize(other.unrepresentable.len(), Unrepresentable::default());
        }
        for (slot, count) in self.unrepresentable.iter_mut().zip(&other.unrepresentable) {
            slot.merge(count);
        }
    }
}

/// The census of several blocks read as one, one entry per name in `columns`
/// — what a table spanning more than one `COPY` block (I2) resolves against,
/// and what a query's schema commits to over exactly the blocks it will
/// replay (`docs/design/decisions.md`, "D35").
///
/// **Keyed by column name, not by position**: a leaf partition's block lists
/// its columns in the leaf's own order, which need not be the root's or a
/// sibling's. A name a block carries and `columns` does not contributes
/// nothing, and so does a census entry past its header's list — a block
/// naming no columns copies none (I5). A block holding no census contributes
/// nothing either, so a caller believing the union has first made sure every
/// block holds one.
pub fn union_census<'a>(
    columns: &[String],
    blocks: impl IntoIterator<Item = &'a CopyBlock>,
) -> Vec<ArrayShape> {
    let mut out = vec![ArrayShape::default(); columns.len()];
    for block in blocks {
        let census = block.array_shapes.iter().flatten();
        for (name, shape) in block.header.columns.iter().zip(census) {
            let slot = columns.iter().position(|c| c == name);
            if let Some(slot) = slot.and_then(|slot| out.get_mut(slot)) {
                slot.merge(shape);
            }
        }
    }
    out
}

/// **What a data-level block's values are counted against**: whether a field
/// holds a value no Arrow type of its column's could hold, and past which
/// limit (`docs/design/decisions.md`, "D96"). Defined here and implemented by
/// `pgdt_codec::unrepresentable`, which walks each leaf by the column's declared
/// type — a contract over a block's rows, as [`BlockObserver`] is
/// (`docs/design/decisions.md`, "D74").
pub trait FieldCount: Send + Sync {
    /// Whether any column is tested, so a row is split for its count.
    fn counts_any(&self) -> bool;
    /// The tier `field` — column `column`'s still-escaped bytes — is past,
    /// `None` for a value its column holds and for a column past the header.
    fn tier(&self, column: usize, field: &[u8]) -> Option<UnrepresentableTier>;
}

/// Receives every row of one `COPY` block from the mapping pass and answers
/// the block's statistics once it closes. Defined here and implemented in query,
/// the shape `docs/design/decisions.md`, "D74" gives a contract over a block's rows.
///
/// **A block the leader splits is observed piece by piece**: each piece's rows
/// go to an observer [`Self::piece`] made, and the pieces are handed back in
/// file order to [`Self::absorb`], after which the block's observer answers
/// what it would have had it been handed every row itself — bar a decline,
/// which a piece's charge alongside its neighbours' can reach where one
/// observer's would not (`docs/design/decisions.md`, "D85").
pub trait BlockObserver: Send {
    /// One row, `offset` being where its first byte sits relative to the
    /// block's first data byte, and `raw` the still-escaped line without its
    /// terminator.
    fn observe_row(&mut self, offset: u64, raw: &[u8]);

    /// The block's statistics, or [`BlockGathered::Declined`] where the
    /// allowance could not hold them before this call; the close it makes is
    /// kept even past the allowance (`docs/design/decisions.md`, "D85"). `end`
    /// is the terminator line's offset relative to the block's first data byte
    /// — where the last row's line ends.
    fn finish(self: Box<Self>, end: u64) -> BlockGathered;

    /// An observer for a piece of this block: rows starting anywhere after the
    /// ones this observer has been handed, to be given back to
    /// [`Self::absorb`] rather than finished.
    fn piece(&self) -> Box<dyn BlockObserver>;

    /// Fold `later`, made by [`Self::piece`] and handed rows that all follow
    /// every row this observer has seen, into this one.
    fn absorb(&mut self, later: Box<dyn BlockObserver>);

    /// This observer as [`Any`](std::any::Any), which is how [`Self::absorb`]
    /// recovers the concrete piece it made.
    fn into_any(self: Box<Self>) -> Box<dyn std::any::Any>;

    /// The fields its type's `*_in` refuses that this observer keyed and went
    /// past, being told to ignore them
    /// ([`crate::PostgresInvalidValues::Ignore`]), taken before
    /// [`Self::finish`]: what the block records
    /// ([`crate::CopyBlock::ignored_refusals`]).
    fn take_ignored(&mut self) -> Option<IgnoredRefusals>;

    /// Whether this observer checks every field of every row it is handed as
    /// a strict parse does ([`crate::PostgresInvalidValues::Strict`]), so a
    /// block it finishes unrefused is checked in full
    /// ([`crate::CopyBlock::checked_in_full`]).
    fn checks_every_field(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape_of(fields: &[&str]) -> ArrayShape {
        let mut shape = ArrayShape::default();
        for field in fields {
            shape.observe(field.as_bytes());
        }
        shape
    }

    #[test]
    fn dimensionality_is_the_leading_brace_run() {
        assert_eq!(shape_of(&["{1,2}"]).dims, Some((1, 1)));
        assert_eq!(shape_of(&["{{1,2},{3,4}}"]).dims, Some((2, 2)));
        assert_eq!(shape_of(&["{{{1}}}"]).dims, Some((3, 3)));
    }

    /// `array_out` force-quotes any element containing a `{` (I25), so an
    /// element that *looks* like a nested array cannot extend the run:
    /// `{"c{d}"}` is one-dimensional.
    #[test]
    fn a_quoted_element_containing_a_brace_does_not_deepen_the_run() {
        assert_eq!(shape_of(&[r#"{"a,b","c{d}"}"#]).dims, Some((1, 1)));
        assert_eq!(shape_of(&[r#"{"{a}"}"#]).dims, Some((1, 1)));
    }

    /// The census's reason for storing both bounds: these two columns are
    /// indistinguishable by maximum alone, and only the first may become
    /// `List<List<T>>`.
    #[test]
    fn a_mixed_column_is_told_apart_from_a_uniformly_deep_one() {
        assert_eq!(shape_of(&["{{1,2},{3,4}}", "{{5,6}}"]).dims, Some((2, 2)));
        assert_eq!(shape_of(&["{1,2}", "{{1,2},{3,4}}"]).dims, Some((1, 2)));
    }

    /// Neither a SQL NULL nor the empty array constrains a column — `{}`
    /// being what `array_out` writes at *any* dimensionality (I25) — and a
    /// real value alongside them settles it alone.
    #[test]
    fn nulls_and_empty_arrays_contribute_no_dimensionality() {
        assert_eq!(shape_of(&["\\N", "{}"]).dims, None);
        assert_eq!(shape_of(&["{}", "{1,2,3}", "\\N"]).dims, Some((1, 1)));
    }

    #[test]
    fn a_lower_bound_prefix_is_recorded_beside_the_dimensionality() {
        let shape = shape_of(&["[0:2]={7,8,9}"]);
        assert!(shape.lower_bound_prefix);
        assert_eq!(shape.dims, Some((1, 1)));
        // Multi-dimensional, one bracket pair per dimension.
        let shape = shape_of(&["[1:2][0:1]={{1,2},{3,4}}"]);
        assert!(shape.lower_bound_prefix);
        assert_eq!(shape.dims, Some((2, 2)));
    }

    /// Combining blocks is min-of-mins, max-of-maxes, and a prefix anywhere
    /// counts everywhere — what a table occupying several blocks (I2)
    /// resolves against.
    #[test]
    fn merging_two_blocks_widens_the_bounds_rather_than_replacing_them() {
        let mut a = shape_of(&["{1,2}"]);
        a.merge(&shape_of(&["{{1,2},{3,4}}"]));
        assert_eq!(a.dims, Some((1, 2)));
        assert!(!a.lower_bound_prefix);

        // A block that constrained nothing leaves the answer alone, from
        // either side.
        let mut b = shape_of(&["{1,2}"]);
        b.merge(&shape_of(&["\\N", "{}"]));
        assert_eq!(b.dims, Some((1, 1)));
        let mut c = shape_of(&["\\N"]);
        c.merge(&shape_of(&["{{1,2}}"]));
        assert_eq!(c.dims, Some((2, 2)));

        // A prefix in one block disqualifies the column in every block.
        let mut d = shape_of(&["{1,2}"]);
        d.merge(&shape_of(&["[0:1]={7,8}"]));
        assert!(d.lower_bound_prefix);
    }

    /// The census runs over every field of a row it did not type-check, so
    /// non-array values reach it, and none of these may contribute: a
    /// composite opens with `(`, and a text value starting `[` has no
    /// `=`-then-`{`. A text value opening with `{` does count, and is read
    /// only if its column resolves to an array.
    #[test]
    fn a_field_that_is_not_an_array_literal_contributes_nothing() {
        for field in ["", "\\N", "42", "(1,2)", "[hello]", "[a=b]", "hello {world}", "[1:2]=x"] {
            assert_eq!(shape_of(&[field]), ArrayShape::default(), "{field}");
        }
    }
}
