//! What one pass over a `COPY` block's rows records beside them — the
//! array-shape census and the unrepresentable count (`docs/design/decisions.md`,
//! "D35", "D96"), and how a row is folded into them — and the contract over a
//! block's rows that counts it, which the parsers call and a crate above
//! implements ([`FieldCount`]).

use pgdt_core::split_fields;
use serde::{Deserialize, Serialize};

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
    fn observe_row(&mut self, counter: &dyn FieldCount, raw: &[u8]);
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

    /// Fold one data row of a `COPY` block in: its fields into the
    /// array-shape census, one [`ArrayShape`] per column, growing it for a row
    /// that turns out to have more fields than the header named — a malformed
    /// row, which the mapping pass does not refuse — and each field `counter`
    /// tests into the unrepresentable count. Every pass recording a census
    /// calls it: the serial mapping pass, an interior worker and a re-read.
    ///
    /// The row is rejected wholesale before it is split where neither needs
    /// it: an array literal always contains a `{`, the only other thing that
    /// can start one is an `[lb:ub]=` prefix, so a row holding neither byte
    /// in a block whose counter tests no column costs one `memchr2` pass.
    ///
    /// What it costs a scan is read off a profile of a data-level `parse`
    /// (`scripts/measure.py`, `PROFILE_SHAPES`), not off a build without it.
    fn observe_row(&mut self, counter: &dyn FieldCount, raw: &[u8]) {
        let shaped = memchr::memchr2(b'{', b'[', raw).is_some();
        if !shaped && !counter.counts_any() {
            return;
        }
        for (i, field) in split_fields(raw).enumerate() {
            if shaped {
                if i >= self.shapes.len() {
                    self.shapes.resize(i + 1, Default::default());
                }
                self.shapes[i].observe(field);
            }
            if let Some(tier) = counter.tier(i, field)
                && let Some(count) = self.unrepresentable.get_mut(i)
            {
                count.add(tier);
            }
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

/// **What a data-level block's values are counted against**: whether a field
/// holds a value no Arrow type of its column's could hold, and past which
/// limit (`docs/design/decisions.md`, "D96"). Defined here and implemented by
/// `pgdt_codec::unrepresentable`, which walks each leaf by the column's declared
/// type — a contract over a block's rows, as `statistics`' `BlockObserver` is
/// (`docs/design/decisions.md`, "D74").
pub trait FieldCount: Send + Sync {
    /// Whether any column is tested, so a row is split for its count.
    fn counts_any(&self) -> bool;
    /// The tier `field` — column `column`'s still-escaped bytes — is past,
    /// `None` for a value its column holds and for a column past the header.
    fn tier(&self, column: usize, field: &[u8]) -> Option<UnrepresentableTier>;
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
