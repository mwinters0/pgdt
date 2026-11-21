//! What one pass over a `COPY` block's rows records beside them — the
//! array-shape census and the unrepresentable count (`docs/design/decisions.md`,
//! "D35", "D96"), and how a row is folded into them — and the contract over a
//! block's rows that counts it, which the parsers call and a crate above
//! implements ([`FieldCount`]). The shapes are the cache's
//! (`crate::persisted::block`).

use std::sync::Arc;

use crate::{ArrayShape, BlockCensus, Unrepresentable};

/// The most dimensions PostgreSQL can give an array — `MAXDIM`, 6 in every
/// supported version (I25). A literal whose leading brace run is longer than
/// this did not come out of `array_out`, so a consumer must treat it as
/// unusable rather than as a depth.
pub const PG_ARRAY_MAX_DIMS: u8 = 6;

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

    /// Fold one still-COPY-escaped field into this column's census: the
    /// [`ShapeFold`] a field is folded by, fed it once — what the tests
    /// below read a column's shape by.
    #[cfg(test)]
    fn observe(&mut self, field: &[u8]) {
        let mut fold = ShapeFold::default();
        fold.feed(field);
        self.apply(fold);
    }

    /// Fold in what `fold` found of one field.
    fn apply(&mut self, fold: ShapeFold) {
        self.lower_bound_prefix |= fold.prefix;
        if let Some(depth) = fold.depth() {
            let (min, max) = self.dims.unwrap_or((depth, depth));
            self.dims = Some((min.min(depth), max.max(depth)));
        }
    }
}

/// **One field's contribution to its column's [`ArrayShape`], folded over
/// its bytes in order however they are split** — a held row's field fed
/// once, a streamed row's a run at a time — answering what the census
/// answers of the whole field (`docs/design/decisions.md`, "D35").
///
/// The field is not decoded first: neither `{` nor the `[lb:ub]=` prefix is
/// in COPY TEXT's escape set (I15). A field that does not open with `{`,
/// past any `[lb:ub]=` prefix, contributes nothing; one that does counts
/// whatever its column's type, the census being type-blind (D35) and read
/// only for a column resolved to an array.
///
/// Dimensionality is the **leading brace run** (I25): `array_out` opens
/// with exactly `ndim` braces and force-quotes any element containing a
/// `{`, so no element can extend the run. The empty array, `{}`, fits any
/// depth and constrains neither bound.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ShapeFold {
    state: ShapeState,
    /// Whether a `[lb:ub]=` prefix with a `{` after it was read: one
    /// `array_out` writes always has a non-empty array after it (it writes a
    /// bare `{}` before it formats dimensions), and a prefix with no `{` after
    /// it is not array output.
    prefix: bool,
}

/// How far into its field a [`ShapeFold`] has read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ShapeState {
    /// No byte yet.
    #[default]
    Start,
    /// Inside a `[`-opened prefix, looking for the `=` that ends it.
    Prefix,
    /// Just past the prefix's `=`, which a `{` must follow.
    Equals,
    /// In the leading brace run, this many braces long, saturating.
    Braces(u8),
    /// `{}` so far: the empty array, unless another byte follows.
    Empty,
    /// The brace run ended at this depth.
    Done(u8),
    /// The field is no array literal.
    Nothing,
}

impl ShapeFold {
    /// Fold in the field's next `bytes`.
    fn feed(&mut self, mut bytes: &[u8]) {
        while let Some((&byte, rest)) = bytes.split_first() {
            self.state = match self.state {
                ShapeState::Start => match byte {
                    b'[' => ShapeState::Prefix,
                    b'{' => ShapeState::Braces(1),
                    _ => ShapeState::Nothing,
                },
                // The first `=` ends the prefix, found without a byte loop
                // over a long field that opens with `[`.
                ShapeState::Prefix => match memchr::memchr(b'=', bytes) {
                    Some(at) => {
                        bytes = &bytes[at + 1..];
                        self.state = ShapeState::Equals;
                        continue;
                    }
                    None => return,
                },
                ShapeState::Equals if byte == b'{' => {
                    self.prefix = true;
                    ShapeState::Braces(1)
                }
                ShapeState::Equals => ShapeState::Nothing,
                ShapeState::Braces(depth) if byte == b'{' => {
                    ShapeState::Braces(depth.saturating_add(1))
                }
                ShapeState::Braces(1) if byte == b'}' => ShapeState::Empty,
                ShapeState::Braces(depth) => ShapeState::Done(depth),
                ShapeState::Empty => ShapeState::Done(1),
                ShapeState::Done(_) | ShapeState::Nothing => return,
            };
            bytes = rest;
        }
    }

    /// The field's dimensionality, once it has ended: `None` for one that
    /// is no array literal, and for the empty array.
    fn depth(&self) -> Option<u8> {
        match self.state {
            ShapeState::Braces(depth) | ShapeState::Done(depth) => Some(depth),
            _ => None,
        }
    }

    /// Whether the field contributes to its column's shape.
    fn contributes(&self) -> bool {
        self.prefix || self.depth().is_some()
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

impl Unrepresentable {
    /// Values past Arrow's format spec.
    pub fn format(&self) -> u64 {
        self.format
    }

    /// Values within Arrow's format spec and past the engine's calendar.
    pub fn engine(&self) -> u64 {
        self.engine
    }

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
    fn counted(format: u64, engine: u64) -> Self;
    fn add(&mut self, tier: UnrepresentableTier);
}

impl UnrepresentableExt for Unrepresentable {
    /// A count already taken — what a test states and a sum builds.
    fn counted(format: u64, engine: u64) -> Self {
        Self { format, engine }
    }

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
    /// length-tolerant, since a row wider than its header may grow the census.
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

impl BlockCensus {
    /// Count one value of column `column` past `tier`, a column past the
    /// header counting nothing.
    fn count(&mut self, column: usize, tier: Option<UnrepresentableTier>) {
        if let Some(tier) = tier
            && let Some(count) = self.unrepresentable.get_mut(column)
        {
            count.add(tier);
        }
    }
}

/// **What a data-level block's values are counted against**: whether a field
/// holds a value no Arrow type of its column's could hold, and past which
/// limit (`docs/design/decisions.md`, "D96"). Defined here and implemented by
/// `pgdt_codec::unrepresentable`, which walks each leaf by the column's declared
/// type — a contract over a block's rows, as `statistics`' `BlockObserver` is
/// (`docs/design/decisions.md`, "D74").
pub trait FieldCount: Send + Sync {
    /// **The columns tested**, ascending, each with what of its field
    /// [`tier`](Self::tier) needs, so a row no reader holds whole keeps only
    /// that much of it as it goes by (the census fold a
    /// [`Recorder`](crate::internal::Recorder) holds). No field of a column
    /// it leaves out is past a tier, and no field of one is asked of
    /// [`tier`](Self::tier); empty, a row is split only for its shape.
    fn tested(&self) -> &[(usize, Kept)];
    /// The tier `field` — column `column`'s still-escaped bytes — is past,
    /// `None` for a value its column holds. Asked only of a column
    /// [`tested`](Self::tested) names.
    fn tier(&self, column: usize, field: &[u8]) -> Option<UnrepresentableTier>;
}

/// **What a count needs of one tested column's field** ([`FieldCount::tested`]).
///
/// Per column, not per block: a block holding its lines wherever its counter
/// tests a container would hold a long `text` line for a short
/// `timestamptz[]` beside it. The shape census takes no head, which would move
/// a `text` column's persisted census for no array's sake; and a counted
/// container is held and tested by `decode_*`'s grammar, never the `*_in`
/// walks a gathering parse streams, which read spellings `decode_*` refuses
/// (`{ infinity }` in a `date[]`), so the census would count a field the query
/// reading it refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// Its first this many bytes, still escaped: no field longer is past a
    /// tier, so a field that outgrows its head counts nothing — a counted
    /// scalar, whose tier test answers for literals no longer than a bound.
    Head(usize),
    /// All of it — a counted container, whose worst leaf may lie anywhere
    /// in it.
    Whole,
}

/// **What of a held row its census reads** ([`CensusFold::reach`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reach {
    /// The row holds a `{` or a `[`: every field is folded, its shape among
    /// it.
    Shaped,
    /// The row holds no array: only its tested columns are counted, which
    /// all lie within its first this many fields.
    Tested(usize),
}

/// **A block's census and count, folded field by field**: each field's bytes
/// handed in order, however they are split — a held row's each once, a
/// streamed row's as they go by — the shape census by [`ShapeFold`], and the
/// count from what [`FieldCount::tested`] says of the field: nothing where it
/// leaves its column out, a head, or the field whole, which its reader holds
/// and hands over once (`docs/design/decisions.md`, "D138"). What a piece's
/// answers is unioned into its block's ([`Self::absorb`]). A pass reaches it
/// through the block's [`Recorder`](crate::internal::Recorder).
///
/// What it costs a scan is read off a profile of a data-level `parse`
/// (`scripts/measurement/recipes/profile.py`, `PROFILE_SHAPES`), not off a build without it.
pub(crate) struct CensusFold {
    census: BlockCensus,
    width: usize,
    counter: Arc<dyn FieldCount>,
    /// What each of the block's columns keeps of a field, read once off
    /// [`FieldCount::tested`]: `None` for a column it leaves out.
    kept: Vec<Option<Kept>>,
    /// One past the last column [`Self::kept`] keeps anything of, `0` where
    /// none: how far a held row holding no array is read ([`Self::reach`]).
    reach: usize,
    /// The field being folded: its shape so far, and its head where its
    /// column keeps one.
    shape: ShapeFold,
    head: Vec<u8>,
    /// Whether the field outgrew its head, so it counts nothing.
    outgrown: bool,
    /// Whether the row so far holds a `{` or a `[`, which grows the shape
    /// census to its width as a held row's does.
    shaped: bool,
}

impl std::fmt::Debug for CensusFold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CensusFold").field("census", &self.census).finish_non_exhaustive()
    }
}

impl CensusFold {
    /// Nothing folded yet, for a block `width` columns wide counted against
    /// `counter`.
    pub(crate) fn new(width: usize, counter: Arc<dyn FieldCount>) -> Self {
        let mut kept = vec![None; width];
        // A column past the header counts nothing, so keeps nothing.
        for &(column, keeps) in counter.tested() {
            if let Some(slot) = kept.get_mut(column) {
                *slot = Some(keeps);
            }
        }
        let reach = kept.iter().rposition(Option::is_some).map_or(0, |last| last + 1);
        Self {
            census: BlockCensus::new(width),
            width,
            counter,
            kept,
            reach,
            shape: ShapeFold::default(),
            head: Vec::new(),
            outgrown: false,
            shaped: false,
        }
    }

    /// Nothing folded yet, for a piece of the same block: its width and its
    /// counter.
    pub(crate) fn fresh(&self) -> Self {
        Self::new(self.width, Arc::clone(&self.counter))
    }

    /// **What of a held row is read**, before the row is split: every field
    /// [`folded`](Self::fold) where it holds a `{` or a `[`, and otherwise
    /// each through its last tested column only [`tested`](Self::test) —
    /// none where no column is tested. An array literal always contains a
    /// `{`, and the only other thing that can open one is an `[lb:ub]=`
    /// prefix, so a row holding neither byte costs the census one `memchr2`
    /// pass and its tested fields' tiers, whatever else reads the row.
    pub(crate) fn reach(&self, raw: &[u8]) -> Reach {
        match memchr::memchr2(b'{', b'[', raw) {
            Some(_) => Reach::Shaped,
            None => Reach::Tested(self.reach),
        }
    }

    /// Whether column `column`'s field is folded whole: its reader holds it
    /// and hands it over once, as its last bytes.
    pub(crate) fn holds(&self, column: usize) -> bool {
        self.kept(column) == Some(Kept::Whole)
    }

    /// What column `column`'s field keeps, `None` where it is not tested.
    fn kept(&self, column: usize) -> Option<Kept> {
        self.kept.get(column).copied().flatten()
    }

    /// Fold in `bytes`, the next of field `column` of the row being folded,
    /// a delimiter never among them; `ends` where they are its last. A field
    /// [`holds`](Self::holds) says is held is handed over whole, once.
    pub(crate) fn fold(&mut self, column: usize, bytes: &[u8], ends: bool) {
        self.shaped |= memchr::memchr2(b'{', b'[', bytes).is_some();
        self.shape.feed(bytes);
        let kept = self.kept(column);
        // A field handed over in one run, as a held row's always is, is
        // tested where it lies rather than copied into the head.
        let whole = ends && self.head.is_empty() && !self.outgrown;
        if let Some(Kept::Head(bound)) = kept
            && !self.outgrown
            && !whole
        {
            if self.head.len() + bytes.len() > bound {
                self.outgrown = true;
                self.head.clear();
            } else {
                self.head.extend_from_slice(bytes);
            }
        }
        if !ends {
            debug_assert!(kept != Some(Kept::Whole), "a held field handed over in pieces");
            return;
        }
        let shape = std::mem::take(&mut self.shape);
        if shape.contributes() {
            if column >= self.census.shapes.len() {
                self.census.shapes.resize(column + 1, ArrayShape::default());
            }
            self.census.shapes[column].apply(shape);
        }
        let tier = match kept {
            Some(Kept::Head(_)) if whole => self.whole_tier(column, kept, bytes),
            Some(Kept::Head(_)) if self.outgrown => None,
            Some(Kept::Head(_)) => self.counter.tier(column, &self.head),
            _ => self.whole_tier(column, kept, bytes),
        };
        self.census.count(column, tier);
        self.head.clear();
        self.outgrown = false;
    }

    /// Count `field`, column `column`'s whole field in a held row holding
    /// no `{` or `[` ([`Reach::Tested`]): it has no shape to fold, so only
    /// its tier is read, as [`Self::fold`] reads one handed over in one run.
    pub(crate) fn test(&mut self, column: usize, field: &[u8]) {
        let tier = self.whole_tier(column, self.kept(column), field);
        self.census.count(column, tier);
    }

    /// The tier `field`, the whole of column `column`'s, is past, by what
    /// its column keeps: none where it is not tested, or where `field` is
    /// longer than its head.
    fn whole_tier(
        &self,
        column: usize,
        kept: Option<Kept>,
        field: &[u8],
    ) -> Option<UnrepresentableTier> {
        match kept {
            None => None,
            Some(Kept::Head(bound)) => {
                (field.len() <= bound).then(|| self.counter.tier(column, field)).flatten()
            }
            Some(Kept::Whole) => self.counter.tier(column, field),
        }
    }

    /// End the row being folded, `fields` fields wide, its last field's
    /// end folded already.
    pub(crate) fn end_row(&mut self, fields: usize) {
        if std::mem::take(&mut self.shaped) && fields > self.census.shapes.len() {
            self.census.shapes.resize(fields, ArrayShape::default());
        }
    }

    /// What the rows folded since the last take answered, leaving nothing
    /// folded.
    pub(crate) fn take(&mut self) -> BlockCensus {
        std::mem::replace(&mut self.census, BlockCensus::new(self.width))
    }

    /// Union `later`'s census, a piece of the same block made by
    /// [`Self::fresh`], into this one's.
    pub(crate) fn absorb(&mut self, later: &CensusFold) {
        self.census.merge(&later.census);
    }
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
