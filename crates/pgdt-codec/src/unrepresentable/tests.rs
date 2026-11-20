use super::*;

/// The calendar `chrono` 0.4.45 ends at, `262142-12-31` (RT21).
fn counter(leaf: Leaf) -> Counter {
    Counter::new(vec![leaf], calendar_end())
}

/// What `counter` keeps of column `column`'s field, `None` where it is
/// not tested.
fn kept_of(counter: &Counter, column: usize) -> Option<Kept> {
    counter.tested().iter().find(|&&(at, _)| at == column).map(|&(_, kept)| kept)
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
    assert_eq!(tier(multirange, "{[2024-01-01,2024-02-01),[2025-01-01,infinity)}"), Some(Format));
    let dated = || Leaf::Record(vec![Leaf::Held, Leaf::Date]);
    assert_eq!(tier(dated(), "(x,infinity)"), Some(Format));
    assert_eq!(tier(dated(), "(infinity,2024-01-01)"), None);
    assert_eq!(tier(dated(), "(infinity,)"), None);
    let intervals = Leaf::Array(Box::new(Leaf::Interval));
    assert_eq!(tier(intervals, "{\"1 day\",\"2562047:47:16.854776\"}"), Some(Format));
}

/// **A container is tested as the walk reads it**, its field as `COPY`
/// wrote it unescaped as it is read: a field holding an escape tests as
/// its text does, and as a reader holding the text, the batch's
/// [`UnrepresentableRead::cannot_hold_walked`], tests it; a value the
/// walk finds malformed past a leaf past the format spec is no value,
/// tested not at all.
#[test]
fn a_container_is_tested_as_the_walk_reads_it() {
    use pgdt_core::escape_field;
    let dates = || Leaf::Array(Box::new(Leaf::Date));
    let dated = || Leaf::Record(vec![Leaf::Held, Leaf::Date]);
    let cases = [
        (dates(), "{\"2024-01-01\",infinity}", Some(Format)),
        (dates(), "{{2024-01-01},{262143-01-01}}", Some(Engine)),
        (dates(), "{2024-01-01,NULL}", None),
        (dated(), "(\"a\tb\\\\c\",infinity)", Some(Format)),
        (dated(), "(\"a\nb\",2024-01-01)", None),
        (dated(), "(x,infinity,\"a\tb\")", Some(Format)),
        (Leaf::Range(Box::new(Leaf::Date)), "[\"2024-01-01\",infinity)", Some(Format)),
        (
            Leaf::Multirange(Box::new(Leaf::Date)),
            "{[2024-01-01,2024-02-01),[2025-01-01,infinity)}",
            Some(Format),
        ),
        // Malformed after its worst leaf: no value.
        (dates(), "{infinity,2024-01-01", None),
        (dates(), "{infinity},", None),
        (dated(), "(infinity,\"a\"b)", None),
    ];
    for (leaf, text, past) in cases {
        let field = escape_field(Some(text));
        assert_eq!(counter(leaf.clone()).tier(0, &field), past, "{text:?} as COPY wrote it");
        assert_eq!(counter(leaf.clone()).tier(0, text.as_bytes()), past, "{text:?}");
        let read = UnrepresentableRead {
            tier: ColumnTier { counter: Arc::new(counter(leaf.clone())), column: 0 },
            reach: Engine,
        };
        assert_eq!(read.cannot_hold(text), past.is_some(), "{text:?}");
        let escaped = FieldText::escaped(&field);
        assert_eq!(read.cannot_hold_walked(escaped), past.is_some(), "{text:?} walked");
        assert_eq!(read.cannot_hold_walked(FieldText::plain(text)), past.is_some(), "{text:?}");
    }
    assert_eq!(counter(dates()).tier(0, b"\\N"), None, "NULL");
    assert_eq!(counter(dates()).tier(0, b"{infinity,\\377}"), None, "not UTF-8");
}

/// The tier a container's leaves are past as `decode_*`'s elements read
/// them, staged: what the walk replaced, kept as the differential's
/// reference.
fn tier_decoded(counter: &Counter, leaf: &Leaf, text: &str) -> Option<UnrepresentableTier> {
    use crate::nested::{
        RangeLiteral, decode_array, decode_multirange, decode_range, decode_record,
    };
    let worst = |tiers: Vec<Option<UnrepresentableTier>>| {
        let tiers: Vec<_> = tiers.into_iter().flatten().collect();
        tiers.iter().find(|tier| **tier == Format).or(tiers.last()).copied()
    };
    let bounds = |bound: &Leaf, range: &RangeLiteral| {
        [range.lower.as_deref(), range.upper.as_deref()]
            .into_iter()
            .flatten()
            .map(|value| tier_decoded(counter, bound, value))
            .collect::<Vec<_>>()
    };
    match leaf {
        Leaf::Array(child) => worst(
            decode_array(text)?
                .elements
                .iter()
                .flatten()
                .map(|e| tier_decoded(counter, child, e))
                .collect(),
        ),
        Leaf::Record(fields) => worst(
            fields
                .iter()
                .zip(&decode_record(text)?.fields)
                .filter_map(|(leaf, field)| Some((leaf, field.as_deref()?)))
                .map(|(leaf, field)| tier_decoded(counter, leaf, field))
                .collect(),
        ),
        Leaf::Range(bound) => worst(bounds(bound, &decode_range(text)?)),
        Leaf::Multirange(bound) => {
            worst(decode_multirange(text)?.iter().flat_map(|range| bounds(bound, range)).collect())
        }
        scalar => counter.tier_of(scalar, text),
    }
}

/// **The walk tests a container as `decode_*`'s elements did**: over
/// seeds of every container form holding dates, and every one of them
/// damaged a byte at a time, the tier read in place, as `COPY` escapes
/// it and through the batch's walked test is the staged reference's.
#[test]
fn the_walk_tests_a_container_as_its_elements_did() {
    const INSERTED: &[u8] = b"{}[](),\"\\ 9in";
    let dates = || Leaf::Array(Box::new(Leaf::Date));
    let dated = || Leaf::Record(vec![Leaf::Held, Leaf::Date]);
    let cases = [
        (dates(), vec!["{2024-01-01,infinity}", "{{262143-01-01,1-01-01},{NULL,infinity}}"]),
        (dates(), vec!["[0:1]={-infinity,2024-01-01}", "{}", "{\"infinity\"}"]),
        (dated(), vec!["(x,infinity)", "(\"a,b\",262143-01-01)", "(,)"]),
        (Leaf::Array(Box::new(dated())), vec!["{\"(x,infinity)\",\"(y,2024-01-01)\",NULL}"]),
        (Leaf::Range(Box::new(Leaf::Date)), vec!["[2024-01-01,infinity)", "empty", "(,)"]),
        (
            Leaf::Multirange(Box::new(Leaf::Date)),
            vec!["{[2024-01-01,2024-02-01),[262143-01-01,infinity)}", "{}"],
        ),
    ];
    let damaged = |seed: &str| {
        let bytes = seed.as_bytes();
        let mut out = vec![seed.to_string()];
        for at in 0..=bytes.len() {
            let (head, tail) = bytes.split_at(at);
            if let Some((_, rest)) = tail.split_first() {
                out.extend(String::from_utf8([head, rest].concat()).ok());
            }
            for &byte in INSERTED {
                out.extend(String::from_utf8([head, &[byte], tail].concat()).ok());
            }
        }
        out
    };
    let mut past = 0;
    for (leaf, seeds) in cases {
        let counter = counter(leaf.clone());
        let read = UnrepresentableRead {
            tier: ColumnTier { counter: Arc::new(counter.clone()), column: 0 },
            reach: Engine,
        };
        for text in seeds.iter().flat_map(|seed| damaged(seed)) {
            let expected = tier_decoded(&counter, &leaf, &text);
            past += usize::from(expected.is_some());
            assert_eq!(counter.tier_of(&leaf, &text), expected, "{text:?}");
            let field = pgdt_core::escape_field(Some(&text));
            assert_eq!(counter.tier(0, &field), expected, "{text:?} as COPY wrote it");
            let walked = read.cannot_hold_walked(FieldText::escaped(&field));
            assert_eq!(walked, expected.is_some(), "{text:?} walked");
        }
    }
    // A corpus no value of which is past a tier would agree vacuously.
    assert!(past > 100, "{past} values past a tier");
}

/// **A scalar is counted from a head of its field**: the longest text
/// each reader reads, its parts padded with zeros until the reader
/// refuses it for its work buffer, is within the bound its test is held
/// to — so the bound changes no count — and that text escaped byte by
/// byte is within the head its column keeps, and counted from it.
#[test]
fn a_scalar_is_counted_from_a_head_of_its_field() {
    use crate::value::datetime::grammar::MAXDATEFIELDS;
    use crate::value::datetime::{date_days, interval_parts};
    let reads = |leaf: &Leaf, text: &str| match leaf {
        Leaf::Date => date_days(text).is_ok(),
        Leaf::Timestamp { with_tz } => timestamp_micros_wide(text, *with_tz).is_ok(),
        Leaf::Interval => interval_parts(text).is_ok(),
        _ => unreachable!(),
    };
    for (leaf, text, past) in [
        (Leaf::Date, "2000-01-01 BC", None),
        (Leaf::Date, "262143-01-01", Some(Engine)),
        (Leaf::Timestamp { with_tz: false }, "2000-01-01 00:00:00 BC", None),
        (Leaf::Timestamp { with_tz: true }, "2000-01-01 00:00:00+00 BC", None),
        (Leaf::Timestamp { with_tz: true }, "262143-01-01 00:00:00+00", Some(Engine)),
        (Leaf::Interval, "1 years 1 mons 1 days 2562047:47:16.854776", Some(Format)),
    ] {
        let longest = leaf.longest_counted().expect("a scalar");
        let read = (0..)
            .map(|zeros| {
                // Zeros where the reader takes a part at any length:
                // the year, and an interval's hours.
                let at = if leaf == Leaf::Interval { text.rfind(' ').unwrap() + 1 } else { 0 };
                format!("{}{}{}", &text[..at], "0".repeat(zeros), &text[at..])
            })
            .take_while(|padded| reads(&leaf, padded))
            .last()
            .unwrap_or_else(|| panic!("{text} is read"));
        // Within the spaces the bound allows of the buffer, so the padding
        // stopped at the buffer.
        assert!(read.len() + MAXDATEFIELDS + MAX_SPACES >= longest, "{read}: stopped short");
        assert!(read.len() <= longest, "{read}: {} bytes past {longest}", read.len());
        let escaped: String = read.bytes().map(|b| format!("\\{b:03o}")).collect();
        let Some(Kept::Head(head)) = kept_of(&counter(leaf.clone()), 0) else { panic!("a head") };
        assert!(escaped.len() <= head, "{read}");
        assert_eq!(counter(leaf.clone()).tier(0, escaped.as_bytes()), past, "{read}");
    }
    for (leaf, kept) in [
        (Leaf::Time, Some(Kept::Head(32))),
        (Leaf::Decimal, Some(Kept::Head(12))),
        (Leaf::Held, None),
        (Leaf::Array(Box::new(Leaf::Date)), Some(Kept::Whole)),
    ] {
        assert_eq!(kept_of(&counter(leaf), 0), kept);
    }
    assert_eq!(kept_of(&counter(Leaf::Date), 1), None, "a column past the header");
}

/// The most spaces a text the tests above pad holds, so the padding is
/// seen to reach its reader's work buffer.
const MAX_SPACES: usize = 8;

/// **A column of nothing its type cannot hold splits no row**, however
/// deep the container.
#[test]
fn a_container_of_held_leaves_is_held() {
    let leaf = Leaf::of(
        &DataType::List(Arc::new(arrow_schema::Field::new("item", DataType::Int32, true))),
        &NestedPlan::array(NestedPlan::Scalar),
    );
    assert_eq!(leaf, Leaf::Held);
    assert!(Counter::new(vec![leaf, Leaf::Held], calendar_end()).tested().is_empty());
    assert_eq!(
        Counter::new(vec![Leaf::Held, Leaf::Date, Leaf::Held, Leaf::Time], calendar_end())
            .tested()
            .iter()
            .map(|&(column, _)| column)
            .collect::<Vec<_>>(),
        [1, 3],
        "the tested columns, ascending",
    );
}
