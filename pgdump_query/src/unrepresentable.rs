//! Which values a column's Arrow type cannot hold, told from the text: the
//! count a data-level mapping pass takes beside the array-shape census
//! (`docs/design/decisions.md`, "D96").
//!
//! **A leaf is tested against its own declared type's spellings.** A column
//! is walked by the pair its declared type resolves to under
//! [`SchemaMode::Typed`] against the block's DDL and no census, so an array
//! element, a range bound and a composite's field are each their own type's
//! leaf, and a `text` field reading `infinity` beside a `date` one counts for
//! nothing. A container holding one such leaf is one value, counted once.
//!
//! **The test is lexical and never otherwise a decode**: the spellings
//! `infinity`, `-infinity`, `NaN` and `24:00:00` exactly, an `interval`'s hour
//! part of seven digits or more, and a `date` or timestamp year at or past the
//! calendar's end — and only those last two, rare by construction, take the
//! arithmetic that says which side of the bound they fall.
//! A value that does not parse as its type is not counted: it is outside the
//! input contract, and refuses in every mode.

use std::sync::Arc;

use arrow::datatypes::DataType;

use crate::copy::{CopyHeader, decode_field};
use crate::decode::{civil_from_days, decode_date32, interval_parts, timestamp_micros_wide};
use crate::index::{UnrepresentableTier, calendar_end};
use crate::map::FieldCount;
use crate::nested::{RangeLiteral, decode_array, decode_multirange, decode_range, decode_record};
use crate::pgtype::{NestedPlan, RANGE_STRUCT_FIELDS};
use crate::preamble::DumpMetadata;
use crate::resolve::{ResolvedSchema, SchemaMode, resolve_columns};

use UnrepresentableTier::{Engine, Format};

/// One position of a column's declared type, as the count tests it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Leaf {
    /// Every value it can spell is one its Arrow type holds — `Utf8View`, an
    /// integer, a float, and every container of nothing else.
    Held,
    Date,
    Timestamp {
        with_tz: bool,
    },
    Time,
    Interval,
    /// `numeric(p,s)` held as a decimal, where `NaN` has no encoding.
    Decimal,
    Array(Box<Leaf>),
    Record(Vec<Leaf>),
    /// The bound type's leaf, shared by both bounds.
    Range(Box<Leaf>),
    Multirange(Box<Leaf>),
}

impl Leaf {
    /// The leaf `data_type`, filled as `plan` says, is — [`Leaf::Held`]
    /// wherever nothing under it can be unrepresentable, so a column of
    /// nothing else splits no row.
    fn of(data_type: &DataType, plan: &NestedPlan) -> Leaf {
        let bound = |range: &DataType| match range {
            DataType::Struct(fields) => {
                fields.find(RANGE_STRUCT_FIELDS[0]).map(|(_, f)| f.data_type().clone())
            }
            _ => None,
        };
        let leaf = match (plan, data_type) {
            (NestedPlan::Scalar, DataType::Date32) => Leaf::Date,
            (NestedPlan::Scalar, DataType::Timestamp(_, tz)) => {
                Leaf::Timestamp { with_tz: tz.is_some() }
            }
            (NestedPlan::Scalar, DataType::Time64(_)) => Leaf::Time,
            (NestedPlan::Scalar, DataType::Interval(_)) => Leaf::Interval,
            (NestedPlan::Scalar, DataType::Decimal128(..) | DataType::Decimal256(..)) => {
                Leaf::Decimal
            }
            (NestedPlan::Array(child), DataType::List(item)) => {
                Leaf::Array(Box::new(Leaf::of(item.data_type(), child)))
            }
            (NestedPlan::Record(plans), DataType::Struct(fields)) => Leaf::Record(
                fields.iter().zip(plans).map(|(f, plan)| Leaf::of(f.data_type(), plan)).collect(),
            ),
            (NestedPlan::Range(child), range) => match bound(range) {
                Some(bound) => Leaf::Range(Box::new(Leaf::of(&bound, child))),
                None => Leaf::Held,
            },
            (NestedPlan::Multirange(child), DataType::List(member)) => {
                match bound(member.data_type()) {
                    Some(bound) => Leaf::Multirange(Box::new(Leaf::of(&bound, child))),
                    None => Leaf::Held,
                }
            }
            _ => Leaf::Held,
        };
        if leaf.holds_everything() { Leaf::Held } else { leaf }
    }

    fn holds_everything(&self) -> bool {
        match self {
            Leaf::Held => true,
            Leaf::Array(child) | Leaf::Range(child) | Leaf::Multirange(child) => {
                child.holds_everything()
            }
            Leaf::Record(fields) => fields.iter().all(Leaf::holds_everything),
            _ => false,
        }
    }
}

/// **One block's count**: each of its columns' leaves, and the calendar end
/// the engine tier is counted against ([`calendar_end`]).
#[derive(Debug, Clone)]
pub(crate) struct Counter {
    columns: Vec<Leaf>,
    /// The first microsecond past the calendar, from 1970 UTC.
    calendar_past_micros: i128,
    calendar_end_days: i32,
    candidate_year: u64,
}

/// The count for a block under `header`, its columns resolved against the
/// DDL `metadata` states for `database` as a statistics observer resolves
/// them: typed, and against no census, which moves only an array's depth and
/// never a leaf.
pub(crate) fn counter_for(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
) -> Arc<dyn FieldCount> {
    let resolved = resolve_columns(
        &header.qualified_name(),
        &header.columns,
        metadata,
        database,
        SchemaMode::Typed,
        &[],
    );
    Arc::new(Counter::new(leaves(&resolved), calendar_end()))
}

/// Each of `resolved`'s columns' leaf.
fn leaves(resolved: &ResolvedSchema) -> Vec<Leaf> {
    resolved
        .schema
        .fields()
        .iter()
        .zip(&resolved.plans)
        .map(|(field, plan)| Leaf::of(field.data_type(), plan))
        .collect()
}

/// **One column's tier test**, for a reader holding the value's text already:
/// what a statistics observer counts each group's values by, and which of its
/// views a value joins (`crate::statistics::StatisticsView`).
#[derive(Debug, Clone)]
pub(crate) struct ColumnTier {
    counter: Arc<Counter>,
    column: usize,
}

impl ColumnTier {
    /// The tier `text`, one unescaped value of the column, is past.
    pub(crate) fn of(&self, text: &str) -> Option<UnrepresentableTier> {
        self.counter.tier_of(&self.counter.columns[self.column], text)
    }
}

/// Each of `resolved`'s columns' tier test, `None` for one every value of
/// which its type holds. `resolved` is the typed resolution against no census
/// that [`counter_for`] resolves, so a group counts what its block does.
pub(crate) fn column_tiers(resolved: &ResolvedSchema) -> Vec<Option<ColumnTier>> {
    let counter = Arc::new(Counter::new(leaves(resolved), calendar_end()));
    (0..counter.columns.len())
        .map(|column| {
            (counter.columns[column] != Leaf::Held)
                .then(|| ColumnTier { counter: Arc::clone(&counter), column })
        })
        .collect()
}

/// The tier test of a scalar column the typed read emits as `data_type`, for
/// a test holding no resolved schema.
#[cfg(test)]
pub(crate) fn scalar_tier(data_type: &DataType) -> Option<ColumnTier> {
    let leaf = Leaf::of(data_type, &NestedPlan::Scalar);
    (leaf != Leaf::Held).then(|| ColumnTier {
        counter: Arc::new(Counter::new(vec![leaf], calendar_end())),
        column: 0,
    })
}

impl Counter {
    fn new(columns: Vec<Leaf>, calendar_end_days: i32) -> Self {
        let calendar_past_micros = (i128::from(calendar_end_days) + 1) * 86_400_000_000;
        let candidate_year = candidate_year(calendar_end_days);
        Counter { columns, calendar_past_micros, calendar_end_days, candidate_year }
    }

    /// The tier `text`, one value of `leaf`, is past — a container's worst
    /// leaf's, the format spec's dominating.
    fn tier_of(&self, leaf: &Leaf, text: &str) -> Option<UnrepresentableTier> {
        let worst = |tiers: &mut dyn Iterator<Item = Option<UnrepresentableTier>>| {
            let mut worst = None;
            for tier in tiers.flatten() {
                if tier == Format {
                    return Some(Format);
                }
                worst = Some(tier);
            }
            worst
        };
        match leaf {
            Leaf::Held => None,
            Leaf::Date => self.date(text),
            Leaf::Timestamp { with_tz } => self.timestamp(text, *with_tz),
            Leaf::Time => (text == "24:00:00").then_some(Format),
            Leaf::Interval => interval(text),
            Leaf::Decimal => (text == "NaN").then_some(Format),
            Leaf::Array(child) => {
                let literal = decode_array(text)?;
                worst(&mut literal.elements.iter().flatten().map(|e| self.tier_of(child, e)))
            }
            Leaf::Record(fields) => {
                let literal = decode_record(text)?;
                worst(
                    &mut fields
                        .iter()
                        .zip(&literal.fields)
                        .filter_map(|(leaf, field)| Some((leaf, field.as_deref()?)))
                        .map(|(leaf, field)| self.tier_of(leaf, field)),
                )
            }
            Leaf::Range(bound) => {
                let range = decode_range(text)?;
                worst(&mut self.bounds(bound, &range))
            }
            Leaf::Multirange(bound) => {
                let members = decode_multirange(text)?;
                worst(&mut members.iter().flat_map(|range| self.bounds(bound, range)))
            }
        }
    }

    fn bounds<'a>(
        &'a self,
        bound: &'a Leaf,
        range: &'a RangeLiteral,
    ) -> impl Iterator<Item = Option<UnrepresentableTier>> + 'a {
        [range.lower.as_deref(), range.upper.as_deref()]
            .into_iter()
            .flatten()
            .map(move |value| self.tier_of(bound, value))
    }

    /// A `date`: an infinity is past the format spec, and a finite one never
    /// is, `Date32` holding PostgreSQL's whole range; past the calendar is a
    /// year at or past its end's, on the day's arithmetic.
    fn date(&self, text: &str) -> Option<UnrepresentableTier> {
        if is_infinity(text) {
            return Some(Format);
        }
        if !self.reaches_calendar_end(text) {
            return None;
        }
        (decode_date32(text)? > self.calendar_end_days).then_some(Engine)
    }

    /// A timestamp: an infinity, or an instant past `i64` microseconds from
    /// 1970, is past the format spec; one short of that and past the
    /// calendar's last microsecond is past the engine's. Only a year at or
    /// past the calendar's end takes the arithmetic — a `timestamptz`'s offset
    /// moving its instant by less than a day — which that bound lies below.
    fn timestamp(&self, text: &str, with_tz: bool) -> Option<UnrepresentableTier> {
        if is_infinity(text) {
            return Some(Format);
        }
        if !self.reaches_calendar_end(text) {
            return None;
        }
        let micros = timestamp_micros_wide(text, with_tz)?;
        if i64::try_from(micros).is_err() {
            Some(Format)
        } else {
            (micros >= self.calendar_past_micros).then_some(Engine)
        }
    }

    /// Whether `text`'s year, read lexically, is at or past
    /// [`candidate_year`] — a year before the common era never being.
    fn reaches_calendar_end(&self, text: &str) -> bool {
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || text.ends_with(" BC") {
            return false;
        }
        // Past `u64`'s digits is past any calendar.
        !text[..digits].parse::<u64>().is_ok_and(|year| year < self.candidate_year)
    }
}

/// **The first year a value's lexical year is tested past**: the year the
/// calendar ends in, or that of the last microsecond `i64` counts from 1970
/// if a calendar ever ended later, so every value near either bound takes
/// the arithmetic. A `timestamptz`'s offset moves its instant by less than a
/// day, which neither bound's year lets cross into it from the year before.
fn candidate_year(calendar_end_days: i32) -> u64 {
    const I64_MICROS_LAST_YEAR: i64 = 294_247;
    let (year, _, _) = civil_from_days(i64::from(calendar_end_days));
    u64::try_from(year.min(I64_MICROS_LAST_YEAR)).unwrap_or(0)
}

fn is_infinity(text: &str) -> bool {
    text == "infinity" || text == "-infinity"
}

/// An `interval`: an infinity (from 17), or a time part past Arrow's
/// nanoseconds, is past the format spec — `2562047:47:16.854775807`, so only
/// an hour part of seven digits or more takes the arithmetic. Its months and
/// days are PostgreSQL's `int32`s, which Arrow's are too.
fn interval(text: &str) -> Option<UnrepresentableTier> {
    if is_infinity(text) {
        return Some(Format);
    }
    let time = text.rsplit(' ').next()?;
    let hours = time.strip_prefix(['+', '-']).unwrap_or(time);
    let digits = hours.bytes().take_while(u8::is_ascii_digit).count();
    if digits < 7 || hours.as_bytes().get(digits) != Some(&b':') {
        return None;
    }
    let (_, _, micros) = interval_parts(text)?;
    let nanos = micros.checked_mul(1_000)?;
    i64::try_from(nanos).is_err().then_some(Format)
}

impl FieldCount for Counter {
    fn counts_any(&self) -> bool {
        self.columns.iter().any(|leaf| *leaf != Leaf::Held)
    }

    fn tier(&self, column: usize, field: &[u8]) -> Option<UnrepresentableTier> {
        let leaf = self.columns.get(column)?;
        if *leaf == Leaf::Held {
            return None;
        }
        // `\N` is NULL, and no value these leaves spell holds an escape, so
        // an escaped field is text the decoder undoes before it walks one.
        let text = decode_field(field).ok()??;
        self.tier_of(leaf, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calendar `chrono` 0.4.45 ends at, `262142-12-31` (RT21).
    fn counter(leaf: Leaf) -> Counter {
        Counter::new(vec![leaf], calendar_end())
    }

    fn tier(leaf: Leaf, text: &str) -> Option<UnrepresentableTier> {
        counter(leaf).tier(0, text.as_bytes())
    }

    #[test]
    fn the_calendar_ends_where_rt21_says() {
        let end = calendar_end();
        assert_eq!(crate::decode::render_date32(end), "262142-12-31");
        assert_eq!(candidate_year(end), 262142);
    }

    #[test]
    fn a_date_is_past_the_format_only_as_an_infinity_and_past_the_calendar_after_its_end() {
        assert_eq!(tier(Leaf::Date, "infinity"), Some(Format));
        assert_eq!(tier(Leaf::Date, "-infinity"), Some(Format));
        assert_eq!(tier(Leaf::Date, "5874897-12-31"), Some(Engine));
        assert_eq!(tier(Leaf::Date, "262143-01-01"), Some(Engine));
        assert_eq!(tier(Leaf::Date, "262142-12-31"), None);
        assert_eq!(tier(Leaf::Date, "2024-01-01"), None);
        assert_eq!(tier(Leaf::Date, "4714-11-24 BC"), None);
        assert_eq!(tier(Leaf::Date, "\\N"), None);
        // Text that does not parse is not one.
        assert_eq!(tier(Leaf::Date, "300000-xx-01"), None);
        assert_eq!(tier(Leaf::Date, "Infinity"), None);
    }

    #[test]
    fn a_timestamp_is_past_the_format_past_i64_and_past_the_calendar_short_of_it() {
        let ts = || Leaf::Timestamp { with_tz: false };
        assert_eq!(tier(ts(), "infinity"), Some(Format));
        assert_eq!(tier(ts(), "294276-12-31 23:59:59.999999"), Some(Format));
        assert_eq!(tier(ts(), "294247-01-10 04:00:54.775808"), Some(Format));
        assert_eq!(tier(ts(), "294247-01-10 04:00:54.775807"), Some(Engine));
        assert_eq!(tier(ts(), "262143-01-01 00:00:00"), Some(Engine));
        assert_eq!(tier(ts(), "262142-12-31 23:59:59.999999"), None);
        assert_eq!(tier(ts(), "4714-11-24 00:00:00 BC"), None);
    }

    /// **A `timestamptz`'s offset moves its instant within a year of the
    /// bound**, either way, which is why that year takes the arithmetic.
    #[test]
    fn a_timestamptz_is_tested_as_its_instant() {
        let tz = || Leaf::Timestamp { with_tz: true };
        assert_eq!(tier(tz(), "294247-01-10 04:00:54.775807+00"), Some(Engine));
        assert_eq!(tier(tz(), "294247-01-10 05:00:54.775807+01"), Some(Engine));
        assert_eq!(tier(tz(), "294247-01-10 04:00:54.775807-01"), Some(Format));
        assert_eq!(tier(tz(), "262142-12-31 23:30:00-01"), Some(Engine));
        assert_eq!(tier(tz(), "262143-01-01 00:30:00+01"), None);
        assert_eq!(tier(tz(), "262142-12-31 23:59:59.999999+00"), None);
        assert_eq!(tier(tz(), "4714-11-24 00:00:00+00 BC"), None);
    }

    #[test]
    fn a_time_is_past_the_format_at_24_00_00_alone() {
        assert_eq!(tier(Leaf::Time, "24:00:00"), Some(Format));
        assert_eq!(tier(Leaf::Time, "23:59:59.999999"), None);
    }

    #[test]
    fn an_interval_is_past_the_format_where_its_time_part_passes_nanoseconds() {
        assert_eq!(tier(Leaf::Interval, "infinity"), Some(Format));
        assert_eq!(tier(Leaf::Interval, "-infinity"), Some(Format));
        assert_eq!(tier(Leaf::Interval, "2562047:47:16.854776"), Some(Format));
        assert_eq!(tier(Leaf::Interval, "-2562047:47:16.854776"), Some(Format));
        assert_eq!(tier(Leaf::Interval, "2562047:47:16.854775"), None);
        assert_eq!(tier(Leaf::Interval, "-2562047:47:16.854775"), None);
        assert_eq!(tier(Leaf::Interval, "1 day -2147483647:59:59.999999"), Some(Format));
        assert_eq!(tier(Leaf::Interval, "178956970 years 7 mons"), None);
        assert_eq!(tier(Leaf::Interval, "-2147483648 days"), None);
        assert_eq!(tier(Leaf::Interval, "04:05:06"), None);
    }

    #[test]
    fn a_decimal_is_past_the_format_at_nan_alone() {
        assert_eq!(tier(Leaf::Decimal, "NaN"), Some(Format));
        assert_eq!(tier(Leaf::Decimal, "1.50"), None);
    }

    /// **A container is one value, its worst leaf's tier**, and a leaf is its
    /// own type's: the `text` field reading `infinity` counts for nothing.
    #[test]
    fn a_nested_value_is_counted_once_by_its_leaves_own_types() {
        let dates = || Leaf::Array(Box::new(Leaf::Date));
        assert_eq!(tier(dates(), "{2024-01-01,infinity}"), Some(Format));
        assert_eq!(tier(dates(), "{{2024-01-01},{262143-01-01}}"), Some(Engine));
        assert_eq!(tier(dates(), "{262143-01-01,infinity}"), Some(Format));
        assert_eq!(tier(dates(), "{2024-01-01,NULL}"), None);
        let range = || Leaf::Range(Box::new(Leaf::Date));
        assert_eq!(tier(range(), "[2024-01-01,infinity)"), Some(Format));
        assert_eq!(tier(range(), "(,2024-01-01)"), None);
        assert_eq!(tier(range(), "empty"), None);
        let multirange = Leaf::Multirange(Box::new(Leaf::Date));
        assert_eq!(
            tier(multirange, "{[2024-01-01,2024-02-01),[2025-01-01,infinity)}"),
            Some(Format)
        );
        let dated = || Leaf::Record(vec![Leaf::Held, Leaf::Date]);
        assert_eq!(tier(dated(), "(x,infinity)"), Some(Format));
        assert_eq!(tier(dated(), "(infinity,2024-01-01)"), None);
        assert_eq!(tier(dated(), "(infinity,)"), None);
        let intervals = Leaf::Array(Box::new(Leaf::Interval));
        assert_eq!(tier(intervals, "{\"1 day\",\"2562047:47:16.854776\"}"), Some(Format));
    }

    /// **A column of nothing its type cannot hold splits no row**, however
    /// deep the container.
    #[test]
    fn a_container_of_held_leaves_is_held() {
        let leaf = Leaf::of(
            &DataType::List(Arc::new(arrow::datatypes::Field::new("item", DataType::Int32, true))),
            &NestedPlan::Array(Box::new(NestedPlan::Scalar)),
        );
        assert_eq!(leaf, Leaf::Held);
        assert!(!Counter::new(vec![leaf, Leaf::Held], calendar_end()).counts_any());
        assert!(counter(Leaf::Date).counts_any());
    }
}
