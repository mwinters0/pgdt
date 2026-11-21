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
//! **A leaf's test is lexical and never otherwise a decode of it**: the spellings
//! `infinity`, `-infinity`, `NaN` and `24:00:00` exactly, an `interval`'s hour
//! part of seven digits or more, and a `date` or timestamp year at or past the
//! calendar's end — and only those last two, rare by construction, take the
//! arithmetic that says which side of the bound they fall.
//! A leaf that does not decode is not counted — a container holding one is
//! counted by its other leaves — and refuses wherever it is decoded: text its
//! PostgreSQL type's `*_out` would not write — `24:0:0` for `24:00:00`, which
//! `time_in` reads — is outside the input contract, and a nested array `KD2` leaves on the optimistic path is that
//! deficiency's. It is not decoded in a column the untyped mode reads as its
//! text, nor in a container the null mode takes as NULL whole for another
//! leaf past its tiers.
//!
//! **A container is read by the output grammars' walk**, each leaf tested as
//! the walk hands it over, so a test stages none of its elements and holds
//! no copy of a field read as `COPY` wrote it (`docs/design/decisions.md`,
//! "D153").

use std::sync::Arc;

use arrow_schema::DataType;

use crate::pgtype::{NestedPlan, RANGE_STRUCT_FIELDS};
use crate::resolve::{ResolvedSchema, SchemaMode, resolve_columns};
use crate::value::datetime::grammar::{DateTimeInput, INTERVAL_WORKBUF};
use crate::value::datetime::{
    civil_from_days, decode_date32, interval_parts, longest_read, timestamp_micros_wide,
};
use crate::value::nested::{
    FieldText, walk_array, walk_bracketed, walk_multirange, walk_range, walk_record,
};
use crate::value::text::escaped_utf8_error;
use pgdt_core::internal::unescaped;
use pgdt_core::{CopyHeader, unescape_field};
use pgdt_model::{
    DumpMetadata, Unrepresentable, UnrepresentableTier, calendar_end,
    internal::{FieldCount, Kept},
};

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
            (
                NestedPlan::Scalar | NestedPlan::Decimal { .. },
                DataType::Decimal128(..) | DataType::Decimal256(..),
            ) => Leaf::Decimal,
            (NestedPlan::Array(child), DataType::List(item)) => {
                Leaf::Array(Box::new(Leaf::of(item.data_type(), &child.plan)))
            }
            (NestedPlan::Record(plans), DataType::Struct(fields)) => Leaf::Record(
                fields
                    .iter()
                    .zip(plans)
                    .map(|(f, field)| Leaf::of(f.data_type(), &field.plan))
                    .collect(),
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

    /// **The longest text a scalar leaf's test finds past a tier**, which
    /// [`Counter::tier_of`] holds it to, so a field's head is all a count
    /// needs of it: a spelling's own length, or the longest a reader reads
    /// ([`longest_read`]), a reader refusing a literal past its input
    /// function's work buffer (I83, I84). `None` for a container, whose worst
    /// leaf may lie anywhere in it.
    fn longest_counted(&self) -> Option<usize> {
        Some(match self {
            Leaf::Held => 0,
            Leaf::Date => longest_read(DateTimeInput::Date.workbuf()),
            Leaf::Timestamp { with_tz: false } => longest_read(DateTimeInput::Timestamp.workbuf()),
            Leaf::Timestamp { with_tz: true } => longest_read(DateTimeInput::TimestampTz.workbuf()),
            Leaf::Time => "24:00:00".len(),
            Leaf::Interval => longest_read(INTERVAL_WORKBUF),
            Leaf::Decimal => "NaN".len(),
            Leaf::Array(_) | Leaf::Record(_) | Leaf::Range(_) | Leaf::Multirange(_) => return None,
        })
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
struct Counter {
    columns: Vec<Leaf>,
    /// Each column some leaf of which a value can be past a tier, with what
    /// its test needs of a field ([`FieldCount::tested`]).
    tested: Vec<(usize, Kept)>,
    /// The first microsecond past the calendar, from 1970 UTC.
    calendar_past_micros: i128,
    calendar_end_days: i32,
    candidate_year: u64,
}

/// The count for a block under `header`, its columns resolved against the
/// DDL `metadata` states for `database` as a statistics observer resolves
/// them: typed, and against no census, which moves only an array — its depth,
/// or a varying shape to text — and never a leaf's declared type.
pub fn counter_for(
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
    resolved.columns().iter().map(|column| Leaf::of(column.data_type(), column.plan())).collect()
}

/// **One column's tier test**, for a reader holding the value's text already:
/// what a statistics observer counts each group's values by, and which of its
/// views a value joins (`pgdt_model::StatisticsView`).
#[derive(Debug, Clone)]
pub struct ColumnTier {
    counter: Arc<Counter>,
    column: usize,
}

impl ColumnTier {
    /// The tier `text`, one unescaped value of the column, is past.
    pub fn of(&self, text: &str) -> Option<UnrepresentableTier> {
        self.counter.tier_of(&self.counter.columns[self.column], text)
    }
}

/// Each of `resolved`'s columns' tier test, `None` for one every value of
/// which its type holds. `resolved` is the typed resolution against no census
/// that [`counter_for`] resolves, so a group counts what its block does.
pub fn column_tiers(resolved: &ResolvedSchema) -> Vec<Option<ColumnTier>> {
    let counter = Arc::new(Counter::new(leaves(resolved), calendar_end()));
    (0..counter.columns.len())
        .map(|column| {
            (counter.columns[column] != Leaf::Held)
                .then(|| ColumnTier { counter: Arc::clone(&counter), column })
        })
        .collect()
}

/// **How a query reads a value PostgreSQL accepts for a column's declared
/// type and the column's Arrow type cannot hold** — `QueryOptions`'s
/// `unrepresentable`, stated where a dump is opened
/// (`docs/design/decisions.md`, "D103").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UnrepresentableMode {
    /// The column keeps its declared type and such a value is NULL, for every
    /// purpose: what a batch holds, what a filter compares, what statistics
    /// count and bound. The default.
    #[default]
    Null,
    /// A column holding such a value is read as `Utf8View`, each value its
    /// text, and every other column keeps its type: the value kept, the type
    /// wider than the floor. It compares by its bytes where DataFusion
    /// compares it, and in its declared type's order in PostgreSQL's
    /// semantics ([`crate::ColumnResolution::UnrepresentableValues`]).
    Text,
    /// A query materializing a column holding such a value refuses at
    /// planning, from the map's count, before a row is read; a column only a
    /// filter the library answers reads is not materialized, and compares in
    /// PostgreSQL's order.
    Refuse,
}

impl UnrepresentableMode {
    /// Every mode, in the order a front end lists them.
    pub const ALL: [Self; 3] = [Self::Null, Self::Text, Self::Refuse];
}

/// A mode's word, which every front end writes and reads it by.
impl std::fmt::Display for UnrepresentableMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Null => "null",
            Self::Text => "text",
            Self::Refuse => "refuse",
        })
    }
}

/// A mode by its word ([`Display`](std::fmt::Display)'s), in any case.
impl std::str::FromStr for UnrepresentableMode {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|mode| mode.to_string().eq_ignore_ascii_case(text)).ok_or_else(
            || {
                let words: Vec<String> = Self::ALL.iter().map(|mode| format!("`{mode}`")).collect();
                format!("`{text}` is not one of {}", words.join(", "))
            },
        )
    }
}

/// **How one query reads one column's values its type cannot hold as NULL**:
/// the column's tier test and the tiers the query's front end cannot hold
/// (`docs/design/decisions.md`, "D98").
#[derive(Debug, Clone)]
pub struct UnrepresentableRead {
    tier: ColumnTier,
    /// [`Engine`] where the front end cannot hold a value past the engine's
    /// calendar either, [`Format`] where it holds every value Arrow's format
    /// spec does.
    reach: UnrepresentableTier,
}

impl UnrepresentableRead {
    /// Whether `text`, one unescaped value of the column, is one the query's
    /// front end cannot hold: what the null mode reads as NULL, and what
    /// `IS UNREPRESENTABLE` matches.
    #[inline]
    pub fn cannot_hold(&self, text: &str) -> bool {
        self.reaches(self.tier.of(text))
    }

    /// [`Self::cannot_hold`] of a nested value read off `text` by the output
    /// grammars' walk, as a query's batch reads it: a field in place or as
    /// `COPY` wrote it, staged nowhere.
    pub fn cannot_hold_walked(&self, mut text: FieldText<'_>) -> bool {
        let counter = &self.tier.counter;
        self.reaches(counter.tier_walked(&counter.columns[self.tier.column], &mut text))
    }

    fn reaches(&self, tier: Option<UnrepresentableTier>) -> bool {
        match tier {
            Some(Format) => true,
            Some(Engine) => self.reach == Engine,
            None => false,
        }
    }

    /// How many of the values `count` counts are ones the query's front end
    /// cannot hold.
    pub fn counted(&self, count: Unrepresentable) -> u64 {
        count.format() + if self.reach == Engine { count.engine() } else { 0 }
    }
}

/// Per column of `resolved` — one block's schema as a query resolves it, in
/// the block's own column order — how that query reads its values their type
/// cannot hold as NULL: `None` for a column read as `Utf8View`, which holds
/// every value, for one whose declared type's leaves all hold every value,
/// and for every column under [`UnrepresentableMode::Refuse`], whose plan
/// has refused any column it materializes holding one (`ReplayPlan::new`),
/// and whose filter orders every such value in PostgreSQL's order — and
/// under [`UnrepresentableMode::Text`], whose plan reads every column
/// holding one as its text, the rest holding none.
///
/// **A column is tested only where `counts`, the block's own count
/// ([`pgdt_model::BlockRecord::unrepresentable`]), says it holds such a value
/// in the tiers `reach` reads**, the count being the map's exact record of
/// them (`docs/design/decisions.md`, "D96"); a block with no count is tested
/// throughout.
///
/// The leaves are the declared type's, resolved typed against no census as
/// [`counter_for`] resolves them, so a column a census deepens is tested as
/// the count tested it.
pub fn unrepresentable_reads(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    resolved: &ResolvedSchema,
    counts: Option<&[Unrepresentable]>,
    mode: UnrepresentableMode,
    reach: UnrepresentableTier,
) -> Vec<Option<UnrepresentableRead>> {
    let width = resolved.columns().len();
    let read_as_text = |i: usize| matches!(resolved.columns()[i].data_type(), DataType::Utf8View);
    if mode != UnrepresentableMode::Null || (0..width).all(read_as_text) {
        return vec![None; width];
    }
    let mut reads = unrepresentable_tests(header, metadata, database, counts, reach);
    reads.resize(width, None);
    for (i, read) in reads.iter_mut().enumerate() {
        if read_as_text(i) {
            *read = None;
        }
    }
    reads
}

/// Per column of a block under `header`, in its own column order, **what
/// `IS [NOT] UNREPRESENTABLE` tests its text by**
/// (`docs/design/decisions.md`, "D101"): the declared type's tier test and the
/// tiers `reach` names, whatever the query's mode or the column's resolution
/// — `None` for a column whose declared type's leaves all hold every value,
/// and for one whose block's `counts` say it holds none in those tiers, a
/// test over either being `False` on every row. A block with no count is
/// tested throughout. The leaves are resolved as [`unrepresentable_reads`]'s
/// are.
pub fn unrepresentable_tests(
    header: &CopyHeader,
    metadata: Option<&DumpMetadata>,
    database: Option<&str>,
    counts: Option<&[Unrepresentable]>,
    reach: UnrepresentableTier,
) -> Vec<Option<UnrepresentableRead>> {
    let declared = resolve_columns(
        &header.qualified_name(),
        &header.columns,
        metadata,
        database,
        SchemaMode::Typed,
        &[],
    );
    column_tiers(&declared)
        .into_iter()
        .enumerate()
        .map(|(i, tier)| {
            let tier = tier?;
            let held = counts.and_then(|counts| counts.get(i)).is_some_and(|count| {
                count.format() == 0 && (reach == Format || count.engine() == 0)
            });
            (!held).then_some(UnrepresentableRead { tier, reach })
        })
        .collect()
}

/// Whether `text`, one value of a column the typed read emits as `data_type`
/// filled as `plan` says, is past neither tier: what a key read in
/// DataFusion's semantics carries its emitted value by (`crate::key`), the
/// provider reading both tiers as unrepresentable.
pub(crate) fn holds_value(data_type: &DataType, plan: &NestedPlan, text: &str) -> bool {
    let leaf = Leaf::of(data_type, plan);
    leaf == Leaf::Held || Counter::new(Vec::new(), calendar_end()).tier_of(&leaf, text).is_none()
}

/// The tier test of a scalar column the typed read emits as `data_type`, for
/// a test holding no resolved schema.
#[cfg(any(test, feature = "test-support"))]
pub fn scalar_tier(data_type: &DataType) -> Option<ColumnTier> {
    let leaf = Leaf::of(data_type, &NestedPlan::Scalar);
    (leaf != Leaf::Held).then(|| ColumnTier {
        counter: Arc::new(Counter::new(vec![leaf], calendar_end())),
        column: 0,
    })
}

/// How a query reaching `reach` reads a scalar column the typed read emits as
/// `data_type`, for a test holding no resolved schema.
#[cfg(any(test, feature = "test-support"))]
pub fn scalar_read(
    data_type: &DataType,
    reach: UnrepresentableTier,
) -> Option<UnrepresentableRead> {
    column_read(data_type, &NestedPlan::Scalar, reach)
}

/// How a query reaching `reach` reads a column the typed read emits as
/// `data_type` filled as `plan` says, for a test holding no resolved schema.
#[cfg(any(test, feature = "test-support"))]
pub fn column_read(
    data_type: &DataType,
    plan: &NestedPlan,
    reach: UnrepresentableTier,
) -> Option<UnrepresentableRead> {
    let leaf = Leaf::of(data_type, plan);
    (leaf != Leaf::Held).then(|| UnrepresentableRead {
        tier: ColumnTier { counter: Arc::new(Counter::new(vec![leaf], calendar_end())), column: 0 },
        reach,
    })
}

impl Counter {
    fn new(columns: Vec<Leaf>, calendar_end_days: i32) -> Self {
        let calendar_past_micros = (i128::from(calendar_end_days) + 1) * 86_400_000_000;
        let candidate_year = candidate_year(calendar_end_days);
        let tested = (columns.iter().enumerate())
            .filter(|(_, leaf)| **leaf != Leaf::Held)
            .map(|(column, leaf)| (column, kept(leaf)))
            .collect();
        Counter { columns, tested, calendar_past_micros, calendar_end_days, candidate_year }
    }

    /// The tier `text`, one value of `leaf`, is past — a container's worst
    /// leaf's, the format spec's dominating. A scalar's text longer than its
    /// [`Leaf::longest_counted`] is past none, by construction rather than
    /// by its reader alone (`docs/design/decisions.md`, "D138").
    fn tier_of(&self, leaf: &Leaf, text: &str) -> Option<UnrepresentableTier> {
        if leaf.longest_counted().is_some_and(|longest| text.len() > longest) {
            return None;
        }
        match leaf {
            Leaf::Held => None,
            Leaf::Date => self.date(text),
            Leaf::Timestamp { with_tz } => self.timestamp(text, *with_tz),
            Leaf::Time => (text == "24:00:00").then_some(Format),
            Leaf::Interval => interval(text),
            Leaf::Decimal => (text == "NaN").then_some(Format),
            Leaf::Array(_) | Leaf::Record(_) | Leaf::Range(_) | Leaf::Multirange(_) => {
                self.tier_walked(leaf, &mut FieldText::plain(text))
            }
        }
    }

    /// [`Self::tier_of`] of a value read off `text` by the output grammars'
    /// walk, each leaf tested as it is handed over and none kept: `None`
    /// where the walk finds no value of `leaf`'s form, however bad a leaf it
    /// read first, so a value is tested whole or not at all, as a decode of
    /// it would be.
    fn tier_walked(&self, leaf: &Leaf, text: &mut FieldText<'_>) -> Option<UnrepresentableTier> {
        let mut worst = None;
        let mut test = |leaf: &Leaf, part: Option<&str>| {
            // Past the format spec is the worst a leaf is, so once one is,
            // the rest is only read through.
            if worst != Some(Format)
                && let Some(tier) = part.and_then(|part| self.tier_of(leaf, part))
            {
                worst = Some(tier);
            }
            Some(())
        };
        match leaf {
            Leaf::Array(child) => walk_array(text, |element| test(child, element)).map(drop),
            Leaf::Record(fields) => {
                // An arity `decode_record` leaves unchecked, a field past the
                // leaves untested.
                let mut at = 0;
                walk_record(text, |field| {
                    let leaf = fields.get(at);
                    at += 1;
                    leaf.map_or(Some(()), |leaf| test(leaf, field))
                })
                .map(drop)
            }
            Leaf::Range(bound) => walk_range(text, |_, value| test(bound, value)).map(drop),
            Leaf::Multirange(bound) => walk_multirange(text, |member| {
                walk_bracketed(member, |_, value| test(bound, value)).map(drop)
            })
            .map(drop),
            scalar => return text.rest().and_then(|text| self.tier_of(scalar, &text)),
        }?;
        worst
    }

    /// A `date`: an infinity is past the format spec, and a finite one never
    /// is, `Date32` holding PostgreSQL's whole range; past the calendar is a
    /// year at or past its end's, on the day's arithmetic. A day PostgreSQL
    /// refuses, `300000-13-45` or one past `5874897-12-31`, does not parse.
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
        let micros = timestamp_micros_wide(text, with_tz).ok()?;
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
    let (_, _, micros) = interval_parts(text).ok()?;
    micros.checked_mul(1_000).is_none().then_some(Format)
}

/// The most still-escaped bytes COPY TEXT spends on one byte, an octal
/// `\ooo` or a hex `\xhh` escape (`pgdt_core::unescaped`): a field longer
/// than this many times a text's length unescapes to a longer text.
const ESCAPED_BYTES_PER_BYTE: usize = 4;

/// What a test of a `leaf` other than [`Leaf::Held`] needs of its field: a
/// scalar's head, every field unescaping to a text its test counts, and a
/// container's whole field.
fn kept(leaf: &Leaf) -> Kept {
    leaf.longest_counted()
        .map_or(Kept::Whole, |longest| Kept::Head(longest * ESCAPED_BYTES_PER_BYTE))
}

impl FieldCount for Counter {
    fn tested(&self) -> &[(usize, Kept)] {
        &self.tested
    }

    fn tier(&self, column: usize, field: &[u8]) -> Option<UnrepresentableTier> {
        let leaf = self.columns.get(column)?;
        match leaf {
            Leaf::Held => None,
            // A container is walked off its field, an escaped one — its text
            // position can hold one — unescaped as it is read, so a field
            // held whole is never copied; `\N` is NULL, and a field whose
            // text is not UTF-8 is no value.
            Leaf::Array(_) | Leaf::Record(_) | Leaf::Range(_) | Leaf::Multirange(_) => {
                unescaped(field)?;
                let mut text = if field.contains(&b'\\') {
                    escaped_utf8_error(field).is_none().then(|| FieldText::escaped(field))?
                } else {
                    FieldText::plain(std::str::from_utf8(field).ok()?)
                };
                self.tier_walked(leaf, &mut text)
            }
            _ => self.tier_of(leaf, &unescape_field(field).ok()??),
        }
    }
}

#[cfg(test)]
mod tests;
