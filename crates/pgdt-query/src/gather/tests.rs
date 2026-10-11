use super::bounds::{
    BYTEA_ESCAPE_HEAD_BYTES, BoundsGatherer, bytea_upper, text_prefix, text_upper,
};
use super::charge::{grown_map_heap, map_heap, push_charged, reserve_charged};
use super::column::ColumnGatherer;
use super::gatherer::{Gatherer, Sizing, density_merges};
use crate::statistics::StatisticsTerms;
use crate::statistics::{Charge, StatisticsAccount, Term};
use arrow::datatypes::DataType;
use pgdt_codec::decode::{decode_bytea, render_bytea};
use pgdt_codec::key::ValueKey;
use pgdt_codec::pgtype::{CompareKind, ComparisonPlan, NestedPlan};
use pgdt_model::internal::UnrepresentableExt;
use pgdt_model::internal::{BlockObserver, max_rows_group, min_rows_group, vec_heap};
use pgdt_model::{
    BlockGathered, BlockStatistics, Bounds, ColumnStatistics, DICTIONARY_ENTRY_MAX_BYTES,
    FieldRefusal, GroupSizing, PostgresInvalidValues, RowGroup, Sortedness, Unrepresentable,
    UnrepresentableTier,
};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;

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
        1 => DICTIONARY_ENTRY_MAX_BYTES - 2 + rng.below(8) as usize,
        _ => 250 + rng.below(40) as usize,
    };
    let alphabet: &[char] = &['a', 'b', ' ', 'é', '\u{10FFFF}', '\u{1f}'];
    match kind {
        CompareKind::Int { .. } => (rng.below(20) as i64 - 10).to_string(),
        // Finite dates either side of the calendar's end, and the
        // infinities, which no format holds.
        CompareKind::Date => match rng.below(10) {
            0 => "infinity".into(),
            1 => "-infinity".into(),
            2 => format!("{}-01-0{}", 262142 + rng.below(3), 1 + rng.below(9)),
            _ => format!("20{:02}-0{}-1{}", rng.below(30), 1 + rng.below(9), rng.below(10)),
        },
        // As a date, and past `i64` microseconds from 1970 too, which
        // keys from PostgreSQL's epoch (I49).
        CompareKind::Timestamp { .. } => match rng.below(12) {
            0 => "infinity".into(),
            1 => "-infinity".into(),
            2 => format!("262143-01-0{} 00:00:00", 1 + rng.below(9)),
            3 => format!("29427{}-12-31 23:59:59", 1 + rng.below(5)),
            _ => format!("20{:02}-01-01 0{}:00:00", rng.below(30), rng.below(10)),
        },
        CompareKind::Bytea => {
            let bytes: Vec<u8> =
                (0..length / 2).map(|_| [0x00, 0x7f, 0xfe, 0xff][rng.below(4) as usize]).collect();
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
    let kinds = [
        CompareKind::Text { length: None },
        CompareKind::PaddedText { length: None },
        CompareKind::Bytea,
        CompareKind::Int { bytes: 8 },
    ];
    let (mut bounded, mut ordered) = (0usize, 0usize);
    let mut charge = Charge::new(Arc::default(), Term::Gathering);
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
                            && !matches!(kind, CompareKind::Int { .. } | CompareKind::Bytea)
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
                gatherer.observe(&mut state, v, None).expect("no value here is refused");
            }
            gatherer.close_group(state, &mut charge);
            let column = gatherer.finish();
            assert_ne!(column.sortedness, Sortedness::Descending, "{kind:?}: sorted input");
            continue;
        }
        for group in &groups {
            let mut state = gatherer.fresh_group();
            for v in group {
                gatherer.observe(&mut state, v, None).expect("no value here is refused");
            }
            gatherer.close_group(state, &mut charge);
        }
        let all: Vec<&String> = groups.iter().flatten().collect();
        let column = gatherer.finish();
        for (group, stored) in groups.iter().zip(&column.groups) {
            let Some(b) = stored else {
                assert!(
                    group.is_empty() || matches!(kind, CompareKind::Int { .. }),
                    "{kind:?}: a group of {} values lost its bounds",
                    group.len()
                );
                continue;
            };
            bounded += 1;
            assert!(
                b.min.len() <= DICTIONARY_ENTRY_MAX_BYTES
                    && b.max.len() <= DICTIONARY_ENTRY_MAX_BYTES
            );
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
                        .any(|v| ValueKey::of(kind, v).unwrap().compare(&high) == Ordering::Equal)
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

/// **Each view's stored bounds are the extremes of the values it takes,
/// and its stated order holds over them row by row**, read back through
/// [`ColumnStatistics::group_bounds`] and [`ColumnStatistics::sortedness`]
/// — over random groups of dates and timestamps holding values past each
/// tier, one past the format's not keying at all. A view kept for no
/// group is kept for none.
#[test]
fn each_view_bounds_and_orders_the_values_it_takes() {
    use UnrepresentableTier::Format;
    use pgdt_codec::internal::scalar_tier;
    use pgdt_model::{BoundsSet, StatisticsView};

    let takes = |view: StatisticsView, tier: Option<UnrepresentableTier>| match view {
        StatisticsView::Every => true,
        StatisticsView::Representable => tier != Some(Format),
        StatisticsView::Displayable => tier.is_none(),
    };
    let views = [StatisticsView::Every, StatisticsView::Representable, StatisticsView::Displayable];
    let kinds = [CompareKind::Date, CompareKind::Timestamp { with_tz: false }];
    let mut rng = Rng(0x7135);
    let (mut bounded, mut ordered, mut kept) = ([0usize; 3], [0usize; 3], [0usize; 2]);
    let mut charge = Charge::new(Arc::default(), Term::Gathering);
    for round in 0..900 {
        let kind = &kinds[round % kinds.len()];
        let tier = scalar_tier(&join_type(kind)).expect("a date or timestamp is tested");
        let mut values: Vec<String> = (0..rng.below(16)).map(|_| value(&mut rng, kind)).collect();
        if round % 3 == 0 {
            let key = |v: &String| ValueKey::of(kind, v);
            values.sort_by(|a, b| match (key(a), key(b)) {
                (Some(a), Some(b)) => a.compare(&b),
                (a, b) => a.is_none().cmp(&b.is_none()),
            });
            if rng.below(2) == 0 {
                values.reverse();
            }
        }
        let mut groups: Vec<Vec<String>> = vec![Vec::new()];
        for v in values {
            if rng.below(4) == 0 {
                groups.push(Vec::new());
            }
            groups.last_mut().unwrap().push(v);
        }
        let mut gatherer = BoundsGatherer::new(kind.clone());
        let mut counts = Vec::new();
        for group in &groups {
            let mut state = gatherer.fresh_group();
            let mut count = Unrepresentable::default();
            for v in group {
                let past = tier.of(v);
                if let Some(past) = past {
                    count.add(past);
                }
                gatherer.observe(&mut state, v, past).expect("no value here is refused");
            }
            gatherer.close_group(state, &mut charge);
            counts.push(count);
        }
        let bounds = gatherer.finish();
        assert_eq!(bounds.every.is_some(), counts.iter().any(|c| c.format > 0), "round {round}");
        assert_eq!(bounds.displayable.is_some(), counts.iter().any(|c| c.engine > 0));
        kept[0] += usize::from(bounds.every.is_some());
        kept[1] += usize::from(bounds.displayable.is_some());
        let column = ColumnStatistics {
            declared_type: None,
            collation: None,
            null_counts: vec![0; groups.len()],
            bounds: Some(bounds),
            datafusion_bounds: None,
            dictionary: None,
            sums: None,
            value_bytes: vec![0; groups.len()],
            unrepresentable: Some(counts.clone()),
        };
        for (v, view) in views.into_iter().enumerate() {
            let taken = |group: &Vec<String>| -> Vec<(String, Option<ValueKey>)> {
                group
                    .iter()
                    .filter(|value| takes(view, tier.of(value)))
                    .map(|value| (value.clone(), ValueKey::of(kind, value)))
                    .collect()
            };
            for (g, group) in groups.iter().enumerate() {
                let what = format!("round {round}, {view:?}, group {g}: {group:?}");
                let taken = taken(group);
                let stored = column.group_bounds(BoundsSet::Primary, view, g);
                let keys: Option<Vec<&ValueKey>> =
                    taken.iter().map(|(_, key)| key.as_ref()).collect();
                let Some(keys) = keys.filter(|keys| !keys.is_empty()) else {
                    assert!(stored.is_none(), "{what}: no value to bound, or one unkeyed");
                    continue;
                };
                let b = stored.unwrap_or_else(|| panic!("{what}: keyed values unbounded"));
                bounded[v] += 1;
                let (low, high) =
                    (ValueKey::of(kind, &b.min).unwrap(), ValueKey::of(kind, &b.max).unwrap());
                assert!(
                    keys.iter().all(|key| low.compare(key).is_le() && high.compare(key).is_ge()),
                    "{what}: {b:?}"
                );
                assert!(keys.iter().any(|key| low.compare(key).is_eq()), "{what}: min {b:?}");
                assert!(keys.iter().any(|key| high.compare(key).is_eq()), "{what}: max {b:?}");
                assert_eq!(
                    column.null_count(g, view),
                    Some(match view {
                        StatisticsView::Every => 0,
                        StatisticsView::Representable => counts[g].format,
                        StatisticsView::Displayable => counts[g].format + counts[g].engine,
                    }),
                    "{what}: NULLs"
                );
            }
            let row: Vec<Option<ValueKey>> =
                groups.iter().flat_map(taken).map(|(_, key)| key).collect();
            let expected = match row.iter().map(Option::as_ref).collect::<Option<Vec<&ValueKey>>>()
            {
                None => Sortedness::Unsorted,
                Some(keys) if keys.windows(2).all(|w| w[1].compare(w[0]).is_ge()) => {
                    Sortedness::Ascending
                }
                Some(keys) if keys.windows(2).all(|w| w[1].compare(w[0]).is_le()) => {
                    Sortedness::Descending
                }
                Some(_) => Sortedness::Unsorted,
            };
            let found = column.sortedness(BoundsSet::Primary, view);
            assert_eq!(found, Some(expected), "round {round}, {view:?}: {groups:?}");
            ordered[v] += usize::from(expected != Sortedness::Unsorted && row.len() > 3);
        }
    }
    assert!(bounded.iter().all(|&n| n > 600), "groups bounded per view: {bounded:?}");
    assert!(ordered.iter().all(|&n| n > 30), "blocks ordered per view: {ordered:?}");
    assert!(kept.iter().all(|&n| n > 200), "views kept: {kept:?}");
}

/// The kinds the join test's columns are compared by, a bytewise kind of
/// each canonical form and three keyed ones, two of whose types cannot
/// hold every value.
const JOIN_KINDS: [CompareKind; 6] = [
    CompareKind::Text { length: None },
    CompareKind::PaddedText { length: None },
    CompareKind::Bytea,
    CompareKind::Int { bytes: 8 },
    CompareKind::Date,
    CompareKind::Timestamp { with_tz: false },
];

/// The type the typed read emits a [`JOIN_KINDS`] column as.
fn join_type(kind: &CompareKind) -> DataType {
    match kind {
        CompareKind::Int { .. } => DataType::Int64,
        CompareKind::Bytea => DataType::Binary,
        CompareKind::Date => DataType::Date32,
        CompareKind::Timestamp { .. } => {
            DataType::Timestamp(arrow::datatypes::TimeUnit::Microsecond, None)
        }
        _ => DataType::Utf8View,
    }
}

/// A block's columns: one per [`JOIN_KINDS`], each keeping bounds, a
/// dictionary and text bytes — the integer sums too, and the date and
/// the timestamp count what their types cannot hold — and an untracked
/// one.
fn join_columns() -> Vec<Option<ColumnGatherer>> {
    join_columns_under(PostgresInvalidValues::Default)
}

/// [`join_columns`], each told what to do with a field its type's `*_in`
/// refuses.
fn join_columns_under(invalid: PostgresInvalidValues) -> Vec<Option<ColumnGatherer>> {
    let plan =
        |kind: &CompareKind| ComparisonPlan::Compared { kind: kind.clone(), divergence: None };
    JOIN_KINDS
        .iter()
        .map(|kind| {
            let plan = plan(kind);
            let bounds = plan.bounds_kinds();
            let data_type = join_type(kind);
            Some(ColumnGatherer::new(
                None,
                None,
                bounds,
                &plan,
                &NestedPlan::Scalar,
                &data_type,
                pgdt_codec::internal::scalar_tier(&data_type),
                invalid,
            ))
        })
        .chain([None])
        .collect()
}

/// One field as `COPY` writes it: a backslash doubled.
fn escaped(text: &str) -> Vec<u8> {
    text.replace('\\', "\\\\").into_bytes()
}

/// One random block's rows as the mapping pass hands them over.
struct RandomBlock {
    group_size: u64,
    lines: Vec<Vec<u8>>,
    offsets: Vec<u64>,
    end: u64,
    short: bool,
}

/// A random block whose groups a row can straddle or skip, whose bytewise
/// values share heads past the cap, and which holds NULLs, values that do
/// not key or are not text, dictionaries on both sides of their cap, and
/// sorted columns. Every fourth round is short values in wide groups, so a
/// dictionary on each side of a cut can pass the count cap between them.
fn random_block(rng: &mut Rng, round: usize) -> RandomBlock {
    let short = round % 4 == 1;
    let group_size = if short { 4096 } else { [16u64, 64, 256, 700, 4096][rng.below(5) as usize] };
    let sorted = round.is_multiple_of(3);
    let rows = if short { 60 + rng.below(140) } else { rng.below(120) } as usize;
    // A small pool per column for some rounds, so dictionaries fit.
    let pool = 2 + rng.below(if round.is_multiple_of(2) { 6 } else { 200 });
    let columns: Vec<Vec<Option<Vec<u8>>>> = JOIN_KINDS
        .iter()
        .map(|kind| {
            let base = value(rng, kind);
            let values: Vec<String> = (0..pool)
                .map(|_| {
                    let v = match kind {
                        CompareKind::Bytea if short => {
                            render_bytea(&(rng.below(1 << 16) as u16).to_be_bytes())
                        }
                        CompareKind::Date | CompareKind::Timestamp { .. } => value(rng, kind),
                        _ if short => rng.below(1000).to_string(),
                        _ => value(rng, kind),
                    };
                    match kind {
                        CompareKind::Text { .. } | CompareKind::PaddedText { .. }
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
                // A value that does not key sorts last.
                let key = |v: &String| ValueKey::of(kind, v);
                drawn.sort_by(|a, b| match (key(a), key(b)) {
                    (Some(a), Some(b)) => a.compare(&b),
                    (a, b) => a.is_none().cmp(&b.is_none()),
                });
                if rng.below(2) == 0 {
                    drawn.reverse();
                }
            }
            drawn
                .into_iter()
                .map(|v| match rng.below(40) {
                    0..=4 => None,
                    5 if !sorted && !short => Some(vec![0xff, b'a']),
                    // Read by `int4in`, a blank around the number, and
                    // keyed by no reader here.
                    6 if !sorted && matches!(kind, CompareKind::Int { .. }) => Some(b" 7".to_vec()),
                    // Read by `byteain`, and placed by no bound here.
                    6 if !sorted && matches!(kind, CompareKind::Bytea) => Some(escaped("\\xAB")),
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
    RandomBlock { group_size, lines, offsets, end, short }
}

/// Every observer's charge is released into the block it became, pieces
/// included, and nothing else is left in the account.
fn assert_only_retained(account: &StatisticsAccount, statistics: &BlockStatistics, at: &str) {
    let retained = statistics.heap_bytes();
    let expected = StatisticsTerms { retained, ..StatisticsTerms::default() };
    assert_eq!(account.held().now, expected, "{at}");
}

/// Gathering at `group_size`, merging past `cap` and short of `min_rows`,
/// every block recording the same request so that only its statistics
/// tell two apart.
fn sized(group_size: u64, cap: Option<usize>, min_rows: Option<u64>) -> Sizing {
    bounded(group_size, cap, min_rows, None)
}

/// [`sized`], under a density maximum as well.
fn bounded(
    group_size: u64,
    cap: Option<usize>,
    min_rows: Option<u64>,
    max_rows: Option<u64>,
) -> Sizing {
    Sizing { group_size, cap, min_rows, max_rows, record: GroupSizing::Stated }
}

/// `block` handed to one observer sized by `sizing`.
fn gathered_serially(block: &RandomBlock, sizing: Sizing) -> BlockStatistics {
    let account = Arc::new(StatisticsAccount::default());
    let charge = Charge::new(Arc::clone(&account), Term::Gathering);
    let mut serial = Gatherer::block(sizing, join_columns(), charge);
    for (line, &offset) in block.lines.iter().zip(&block.offsets) {
        serial.observe_row(offset, line);
    }
    let statistics = Box::new(serial)
        .finish(block.end)
        .gathered()
        .expect("an unbounded account declines nothing");
    assert_only_retained(&account, &statistics, "serial");
    statistics
}

/// `block` handed to pieces cut at random, sized by `sizing`, folded in file order a window
/// of pieces at a time — made before any of them is folded, as the leader
/// makes them — with an empty piece now and then and one left unfolded
/// past the block's end. Answers the statistics, and how many cuts fell
/// inside a group.
fn gathered_in_pieces(
    rng: &mut Rng,
    block: &RandomBlock,
    sizing: Sizing,
) -> (BlockStatistics, usize) {
    let group_size = sizing.group_size;
    let account = Arc::new(StatisticsAccount::default());
    let charge = Charge::new(Arc::clone(&account), Term::Gathering);
    let mut observer: Box<dyn BlockObserver> =
        Box::new(Gatherer::block(sizing, join_columns(), charge));
    let cut_odds = 1 + rng.below(30);
    let window_pieces = 1 + rng.below(4) as usize;
    let mut window = vec![observer.piece()];
    let mut straddles = 0;
    for (r, (line, &offset)) in block.lines.iter().zip(&block.offsets).enumerate() {
        if r > 0 && rng.below(cut_odds) == 0 {
            if window.len() == window_pieces {
                for piece in window.drain(..) {
                    observer.absorb(piece);
                }
                if rng.below(4) == 0 {
                    observer.absorb(observer.piece());
                }
            }
            window.push(observer.piece());
            // Only the rows' own groups say where a cut fell; the block
            // may since have merged, which moves no row between groups
            // it straddled at the base size.
            if block.offsets[r - 1] / group_size == offset / group_size {
                straddles += 1;
            }
        }
        window.last_mut().expect("a window holds a piece").observe_row(offset, line);
    }
    for piece in window {
        observer.absorb(piece);
    }
    let past_the_end = (rng.below(2) == 0).then(|| observer.piece());
    let statistics =
        observer.finish(block.end).gathered().expect("an unbounded account declines nothing");
    drop(past_the_end);
    assert_only_retained(&account, &statistics, "pieces");
    (statistics, straddles)
}

/// **A block observed in pieces, joined in file order, gathers exactly what
/// one observer handed every row gathers** — over [`random_block`]'s
/// blocks, whose groups straddle the cuts. The fixture sweep does not
/// guard a join's order step, a block of `pg_dump` output rarely turning
/// on it: this test does.
#[test]
fn pieces_joined_in_file_order_gather_what_one_pass_gathers() {
    let mut rng = Rng(0x0001_0105);
    let (mut straddles, mut ordered, mut inexact, mut dictionaries, mut overflowed) =
        (0, 0, 0, 0, 0);
    let (mut summed, mut unsummed, mut measured) = (0, 0, 0);
    let mut views = [0; 2];
    for round in 0..600 {
        let block = random_block(&mut rng, round);
        let exact = sized(block.group_size, None, None);
        let serial = gathered_serially(&block, exact);
        let (joined, straddled) = gathered_in_pieces(&mut rng, &block, exact);
        straddles += straddled;
        assert_eq!(joined, serial, "round {round}");

        for (kind, column) in JOIN_KINDS.iter().zip(&joined.columns) {
            let Some(column) = column else { continue };
            let bounds = column.bounds.as_ref().unwrap();
            if bounds.sortedness != Sortedness::Unsorted && block.lines.len() > 10 {
                ordered += 1;
            }
            inexact += bounds.groups.iter().flatten().filter(|b| !b.max_exact).count();
            if let Some(dictionary) = &column.dictionary {
                dictionaries += dictionary.groups.iter().flatten().filter(|g| g.len() > 1).count();
                if block.short {
                    overflowed += dictionary.groups.iter().filter(|g| g.is_none()).count();
                }
            }
            if let Some(counts) = &column.unrepresentable {
                assert_eq!(counts.len(), joined.groups.len(), "round {round}: a count a group");
                views[0] += usize::from(bounds.every.is_some());
                views[1] += usize::from(bounds.displayable.is_some());
            }
            match &column.sums {
                Some(sums) if sums.iter().any(|&sum| sum != 0) => summed += 1,
                None if matches!(kind, CompareKind::Int { .. }) => unsummed += 1,
                _ => {}
            }
            measured += usize::from(column.value_bytes.iter().any(|&bytes| bytes > 0));
        }
    }
    assert!(summed > 100, "only {summed} columns summed");
    assert!(views[0] > 300, "only {} columns kept every value apart", views[0]);
    assert!(views[1] > 300, "only {} columns kept the displayable apart", views[1]);
    assert!(unsummed > 20, "only {unsummed} columns lost their sums to a value not their type");
    assert!(measured > 500, "only {measured} columns measured");
    assert!(straddles > 500, "only {straddles} cuts fell inside a group");
    assert!(ordered > 200, "only {ordered} columns ordered");
    assert!(inexact > 500, "only {inexact} truncated upper bounds");
    assert!(dictionaries > 500, "only {dictionaries} dictionaries of several entries");
    assert!(overflowed > 50, "only {overflowed} dictionaries past their count cap");
}

/// **A block past its cap gathers exactly what gathering at the size it
/// reaches gathers**, handed its rows by one observer or by pieces folded
/// in file order, and holds no more groups than the cap — over
/// [`random_block`]'s blocks at caps of a handful of groups, so a block
/// merges many times, with an odd count mid-scan, between windows of
/// pieces and while a row skips groups.
#[test]
fn a_capped_block_gathers_what_the_size_it_reaches_gathers() {
    let mut rng = Rng(0x0020_0003);
    let (mut coarsened, mut dropped_entries, mut overflowed) = (0, 0, 0);
    for round in 0..600 {
        let block = random_block(&mut rng, round);
        let cap = 1 + rng.below(6) as usize;
        let sizing = sized(block.group_size, Some(cap), None);
        let capped = gathered_serially(&block, sizing);
        assert!(capped.groups.len() <= cap, "round {round}: {} groups", capped.groups.len());
        let exact = gathered_serially(&block, sized(capped.group_size, None, None));
        assert_eq!(capped, exact, "round {round}: serial at cap {cap}");
        let (joined, _) = gathered_in_pieces(&mut rng, &block, sizing);
        assert_eq!(joined, capped, "round {round}: pieces at cap {cap}");

        if capped.group_size > block.group_size {
            coarsened += 1;
            let base = gathered_serially(&block, sized(block.group_size, None, None));
            for (fine, coarse) in base.columns.iter().zip(&capped.columns) {
                let (Some(fine), Some(coarse)) = (fine, coarse) else { continue };
                let entries = |c: &ColumnStatistics| c.dictionary.as_ref().unwrap().entries.len();
                if entries(coarse) < entries(fine) {
                    dropped_entries += 1;
                }
                let lost = |c: &ColumnStatistics| {
                    c.dictionary.as_ref().unwrap().groups.iter().filter(|g| g.is_none()).count()
                };
                if lost(coarse) > 0 && lost(fine) == 0 {
                    overflowed += 1;
                }
            }
        }
    }
    assert!(coarsened > 300, "only {coarsened} blocks merged");
    assert!(dropped_entries > 30, "only {dropped_entries} dictionaries let entries go");
    assert!(overflowed > 20, "only {overflowed} merged groups passed the count cap");
}

/// The size [`density_merges`] must choose, written the way
/// `scripts/row_density.py` chooses it: every size from the gathered one
/// to a single group, and the first whose upper middle group holds the
/// minimum, else the single group.
fn chosen_merges(rows: &[u64], min_rows: u64) -> u32 {
    if rows.is_empty() {
        return 0;
    }
    let mut ladder = vec![rows.to_vec()];
    while ladder.last().unwrap().len() > 1 {
        let last = ladder.last().unwrap();
        let coarser =
            (0..last.len()).step_by(2).map(|i| last[i..last.len().min(i + 2)].iter().sum());
        ladder.push(coarser.collect());
    }
    let median = |level: &Vec<u64>| {
        let mut sorted = level.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    };
    let reaches = ladder.iter().position(|level| median(level) >= min_rows);
    reaches.unwrap_or(ladder.len() - 1) as u32
}

/// **The density rule chooses what the ladder of sizes chooses**: the
/// finest size whose upper middle group holds the minimum, and the single
/// group where no size reaches it.
#[test]
fn density_merges_choose_the_finest_size_whose_median_holds_the_minimum() {
    assert_eq!(density_merges(vec![], 5, None), 0);
    assert_eq!(density_merges(vec![3], 5, None), 0, "one group is as coarse as a block gets");
    assert_eq!(density_merges(vec![5, 0, 5], 5, None), 0, "the second smallest of three");
    assert_eq!(density_merges(vec![0, 5, 0, 5], 5, None), 0, "the third smallest of four");
    assert_eq!(density_merges(vec![0, 0, 0, 5], 5, None), 1, "three of four fall short");
    assert_eq!(density_merges(vec![1, 1, 1, 1, 1], 5, None), 3, "never reached: one group");
    assert_eq!(density_merges(vec![9, 9, 9, 9], 0, None), 0, "a minimum of zero merges nothing");
    let mut rng = Rng(0x0020_0004);
    for round in 0..2000 {
        let groups = 1 + rng.below(40) as usize;
        let skew = 1 + rng.below(4);
        let rows: Vec<u64> =
            (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
        let min_rows = rng.below(30);
        assert_eq!(
            density_merges(rows.clone(), min_rows, None),
            chosen_merges(&rows, min_rows),
            "round {round}: {rows:?} at {min_rows}"
        );
    }
}

/// **The predicate is monotone in size**, which is what lets a block the
/// cap has already coarsened read its minimum from the size the cap left:
/// once the upper middle group holds the minimum, no coarser size falls
/// back below it. The odd tail is the shape that breaks it for the
/// nearest-rank median — `[m, m, m, m, 0, 0, 0]` meets the minimum, the
/// pairs the cap would leave it do not, and the block cascades to one
/// group — so it is asserted at both sizes, and the sweep holds every
/// level of the ladder.
#[test]
fn the_density_predicate_is_monotone_in_size() {
    assert_eq!(density_merges(vec![5, 5, 5, 5, 0, 0, 0], 5, None), 0, "the odd tail, gathered");
    assert_eq!(density_merges(vec![10, 10, 0, 0], 5, None), 0, "the same block, once capped");
    let mut rng = Rng(0x0020_0011);
    let mut reached_early = 0;
    for round in 0..4000 {
        let groups = 1 + rng.below(40) as usize;
        let skew = 1 + rng.below(4);
        let rows: Vec<u64> =
            (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
        let min_rows = rng.below(30);
        let first = chosen_merges(&rows, min_rows) as usize;
        let mut level = rows.clone();
        for merges in 0..groups {
            // Every level at or past the first one that holds is asked
            // again from there, as a capped block's `fit_density` does.
            let from_here = density_merges(level.clone(), min_rows, None) as usize;
            assert_eq!(
                merges + from_here,
                first.max(merges),
                "round {round}: {rows:?} at {min_rows}, from {merges} merges in"
            );
            level = level.chunks(2).map(|pair| pair.iter().sum()).collect();
        }
        reached_early += usize::from(first > 0 && first < groups);
    }
    assert!(reached_early > 500, "only {reached_early} rounds coarsened short of one group");
}

/// **A block sized by its density minimum gathers exactly what gathering
/// at the size it reaches gathers**, handed its rows by one observer or by
/// pieces folded in file order, with or without a cap ahead of it — and
/// uncapped, that size is the one the ladder over the gathered size's rows
/// chooses, so a block no merge reached is at the size it was gathered at.
///
/// **Capped, the size is the coarser of the cap's and that one**, which is
/// the guarantee the monotone predicate buys: a block the cap coarsened
/// past the size its own base distribution reached keeps the cap's size,
/// and one the cap left finer reaches exactly the size it would have
/// uncapped.
#[test]
fn a_block_sized_by_its_minimum_gathers_what_the_size_it_reaches_gathers() {
    let mut rng = Rng(0x0020_0005);
    let (mut coarsened, mut after_cap, mut finer_than_cap) = (0, 0, 0);
    for round in 0..600 {
        let block = random_block(&mut rng, round);
        let min_rows = rng.below(40);
        let cap = (rng.below(2) == 0).then(|| 1 + rng.below(6) as usize);
        let sizing = sized(block.group_size, cap, Some(min_rows));
        let chosen = gathered_serially(&block, sizing);
        let exact = gathered_serially(&block, sized(chosen.group_size, None, None));
        assert_eq!(chosen, exact, "round {round}: serial at {min_rows} rows, cap {cap:?}");
        let (joined, _) = gathered_in_pieces(&mut rng, &block, sizing);
        assert_eq!(joined, chosen, "round {round}: pieces at {min_rows} rows, cap {cap:?}");

        let base = gathered_serially(&block, sized(block.group_size, None, None));
        let rows: Vec<u64> = base.groups.iter().map(|g| g.rows).collect();
        match cap {
            None => {
                let merges = chosen_merges(&rows, min_rows);
                assert_eq!(chosen.group_size, block.group_size << merges, "round {round}");
                coarsened += usize::from(merges > 0);
            }
            Some(cap) => {
                let capped = gathered_serially(&block, sized(block.group_size, Some(cap), None));
                let uncapped = block.group_size << chosen_merges(&rows, min_rows);
                assert_eq!(
                    chosen.group_size,
                    uncapped.max(capped.group_size),
                    "round {round}: cap {cap}, {min_rows} rows"
                );
                after_cap += usize::from(chosen.group_size > capped.group_size);
                finer_than_cap += usize::from(uncapped < capped.group_size);
            }
        }
    }
    assert!(coarsened > 100, "only {coarsened} uncapped blocks coarsened");
    assert!(after_cap > 30, "only {after_cap} capped blocks coarsened past their cap");
    assert!(finer_than_cap > 30, "only {finer_than_cap} capped blocks reached it finer");
}

/// The size [`density_merges`] must choose under a maximum as well as a
/// minimum: the ladder walked as [`chosen_merges`] walks it, stopping at
/// the last size whose 90th-percentile group is within the maximum.
fn chosen_merges_between(rows: &[u64], min_rows: u64, max_rows: u64) -> u32 {
    let mut level = rows.to_vec();
    let mut merges = 0;
    while level.len() > 1 && min_rows_group(&level) < min_rows {
        let coarser: Vec<u64> = level.chunks(2).map(|pair| pair.iter().sum()).collect();
        if max_rows_group(&coarser) > max_rows {
            break;
        }
        (level, merges) = (coarser, merges + 1);
    }
    merges
}

/// **A stated maximum stops the merging, whatever the minimum asks.** A
/// block already past the maximum at the size it holds, its groups pairing
/// evenly, merges nothing — nothing here makes a block finer. Past the
/// maximum, a size whose groups all pair stays past it at the next: of `G`
/// groups, `⌊G/10⌋+1` past it leave at least `⌊G/20⌋+1` pairs past it. A
/// size whose last group stands unpaired is exempt, and can let one merge
/// through (`an_unpaired_last_group_lets_a_block_past_the_maximum_merge`).
#[test]
fn a_stated_maximum_stops_the_merging_whatever_the_minimum_asks() {
    assert_eq!(density_merges(vec![1, 1, 1, 1], 5, Some(2)), 1, "one merge fits, two do not");
    assert_eq!(density_merges(vec![1, 1, 1, 1], 5, Some(9)), 2, "the minimum is reached");
    assert_eq!(density_merges(vec![9, 9, 9, 9], 5, Some(2)), 0, "past the maximum already");
    assert_eq!(density_merges(vec![1, 1, 1, 1], 5, None), 2, "no maximum stops nothing");
    let mut rng = Rng(0x0020_0050);
    let (mut stopped, mut agreed_with_minimum) = (0, 0);
    for round in 0..4000 {
        let groups = 1 + rng.below(40) as usize;
        let skew = 1 + rng.below(4);
        let rows: Vec<u64> =
            (0..groups).map(|_| if rng.below(skew) == 0 { 0 } else { rng.below(12) }).collect();
        let (min_rows, max_rows) = (rng.below(30), rng.below(60));
        let merges = density_merges(rows.clone(), min_rows, Some(max_rows));
        assert_eq!(
            merges,
            chosen_merges_between(&rows, min_rows, max_rows),
            "round {round}: {rows:?} between {min_rows} and {max_rows}"
        );
        let alone = chosen_merges(&rows, min_rows);
        assert!(merges <= alone, "round {round}: a maximum never merges further");
        stopped += usize::from(merges < alone);
        agreed_with_minimum += usize::from(merges == alone);
        // Monotone from every level past the maximum whose groups pair.
        let mut level = rows.clone();
        while level.len() > 1 {
            let coarser: Vec<u64> = level.chunks(2).map(|pair| pair.iter().sum()).collect();
            if level.len().is_multiple_of(2) && max_rows_group(&level) > max_rows {
                assert!(
                    max_rows_group(&coarser) > max_rows,
                    "round {round}: {rows:?} came back within {max_rows} from {level:?}"
                );
            }
            level = coarser;
        }
    }
    assert!(stopped > 300, "only {stopped} rounds were stopped by their maximum");
    assert!(agreed_with_minimum > 300, "only {agreed_with_minimum} rounds reached it");
}

/// **`KD43`, pinned**: nineteen groups, two of them past a maximum of 5,
/// put the block past it, and the merge that pairs them leaves ten whose
/// 90th-percentile group is empty, so the merge goes through and the block
/// holds a group of 20. Closing the deficiency makes this block merge
/// nothing, and flips the assertion.
#[test]
fn an_unpaired_last_group_lets_a_block_past_the_maximum_merge() {
    let rows: Vec<u64> = [10, 10].into_iter().chain([0; 17]).collect();
    assert!(max_rows_group(&rows) > 5, "the block is past the maximum as gathered");
    assert_eq!(density_merges(rows, 1, Some(5)), 1);
}

/// **A block sized between its bounds gathers exactly what gathering at
/// the size it reaches gathers**, serially and by pieces, and that size is
/// the one the ladder over its base distribution chooses — the maximum
/// stopping where the minimum would have gone further.
#[test]
fn a_block_sized_between_its_bounds_gathers_what_the_size_it_reaches_gathers() {
    let mut rng = Rng(0x0020_0051);
    let (mut stopped, mut coarsened) = (0, 0);
    for round in 0..600 {
        let block = random_block(&mut rng, round);
        let (min_rows, max_rows) = (rng.below(40), 1 + rng.below(30));
        let sizing = bounded(block.group_size, None, Some(min_rows), Some(max_rows));
        let chosen = gathered_serially(&block, sizing);
        let exact = gathered_serially(&block, sized(chosen.group_size, None, None));
        assert_eq!(chosen, exact, "round {round}: serial between {min_rows} and {max_rows}");
        let (joined, _) = gathered_in_pieces(&mut rng, &block, sizing);
        assert_eq!(joined, chosen, "round {round}: pieces");

        let base = gathered_serially(&block, sized(block.group_size, None, None));
        let rows: Vec<u64> = base.groups.iter().map(|g| g.rows).collect();
        let merges = chosen_merges_between(&rows, min_rows, max_rows);
        assert_eq!(chosen.group_size, block.group_size << merges, "round {round}: {rows:?}");
        stopped += usize::from(merges < chosen_merges(&rows, min_rows));
        coarsened += usize::from(merges > 0);
    }
    assert!(stopped > 50, "only {stopped} blocks were stopped by their maximum");
    assert!(coarsened > 50, "only {coarsened} blocks coarsened at all");
}

/// Every row of `block` handed to one observer bounded by `allowance`,
/// answering what it finished with and the account it charged.
fn gathered_within(
    block: &RandomBlock,
    sizing: Sizing,
    allowance: Option<u64>,
) -> (BlockGathered, Arc<StatisticsAccount>) {
    let account = Arc::new(StatisticsAccount::bounded_by(allowance));
    let charge = Charge::new(Arc::clone(&account), Term::Gathering);
    let mut serial = Gatherer::block(sizing, join_columns(), charge);
    for (line, &offset) in block.lines.iter().zip(&block.offsets) {
        serial.observe_row(offset, line);
    }
    (Box::new(serial).finish(block.end), account)
}

/// **A block whose statistics pass the pass's allowance declines**: it
/// answers the allowance rather than statistics, frees what it had
/// gathered — the account is empty after it, and never reached the peak
/// an unbounded pass over the block reaches — and the scan reads on
/// (`docs/design/decisions.md`, "D85").
///
/// **Not vacuous**: the same block, the same rows and an allowance that
/// fits gathers exactly what an unbounded account gathers, so the bound
/// and nothing else is what declined it.
#[test]
fn a_block_past_its_allowance_declines_and_frees_what_it_held() {
    let mut rng = Rng(0x0020_0071);
    let (mut declined, mut gathered) = (0, 0);
    for round in 0..200 {
        let block = random_block(&mut rng, round);
        let sizing = sized(block.group_size, None, None);
        // The unbounded run is the control: what the block gathers, and
        // the peak the account reaches gathering it — the interning maps
        // included, which the retained heap alone does not carry.
        let (control, free) = gathered_within(&block, sizing, None);
        let whole = control.gathered().expect("an unbounded account declines nothing");
        let peak = free.held().peak;

        let allowance = peak / 4;
        let (short, account) = gathered_within(&block, sizing, Some(allowance));
        assert_eq!(short, BlockGathered::Declined { allowance }, "round {round}");
        assert_eq!(
            account.held().now,
            StatisticsTerms::default(),
            "round {round}: a decline holds nothing"
        );
        assert!(
            account.held().peak < peak,
            "round {round}: {} reached the unbounded peak, {peak}",
            account.held().peak
        );
        declined += 1;

        // Room for the block and its interning maps several times over.
        let (roomy, _) = gathered_within(&block, sizing, Some(peak * 64 + (1 << 20)));
        assert_eq!(
            roomy.gathered().as_ref(),
            Some(&whole),
            "round {round}: an allowance that fits"
        );
        gathered += 1;
    }
    assert_eq!((declined, gathered), (200, 200));
}

/// **A piece that declined declines its block**, the block having lost
/// that piece's rows; and a block that declined while a window ran drops
/// every piece it is handed, a piece made after the decline gathering
/// nothing
/// (`docs/design/decisions.md`, "D85").
#[test]
fn a_declined_piece_declines_its_block_and_a_declined_block_its_pieces() {
    let mut rng = Rng(0x0020_0072);
    let block = random_block(&mut rng, 2);
    let sizing = sized(block.group_size, None, None);
    let whole = gathered_serially(&block, sizing);
    let allowance = whole.heap_bytes() / 4;

    let account = Arc::new(StatisticsAccount::bounded_by(Some(allowance)));
    let charge = Charge::new(Arc::clone(&account), Term::Gathering);
    let mut observer: Box<dyn BlockObserver> =
        Box::new(Gatherer::block(sizing, join_columns(), charge));
    // One piece a row, folded a window of four at a time, as the leader
    // folds them.
    let mut window: Vec<Box<dyn BlockObserver>> = Vec::new();
    for (line, &offset) in block.lines.iter().zip(&block.offsets) {
        window.push(observer.piece());
        window.last_mut().expect("just pushed").observe_row(offset, line);
        if window.len() == 4 {
            for piece in window.drain(..) {
                observer.absorb(piece);
            }
        }
    }
    for piece in window.drain(..) {
        observer.absorb(piece);
    }
    // A piece made after the block declined holds nothing, whatever it is
    // handed.
    let mut late = observer.piece();
    for (line, &offset) in block.lines.iter().zip(&block.offsets) {
        late.observe_row(offset, line);
    }
    let before = account.held().now;
    drop(late);
    assert_eq!(account.held().now, before, "a piece of a declined block holds nothing");

    assert_eq!(observer.finish(block.end), BlockGathered::Declined { allowance });
    assert_eq!(account.held().now, StatisticsTerms::default());
}

/// A block of `rows` rows, `refused` holding a `date` its month lacks,
/// which `date_in` refuses (I61), and every other row a day it holds: the
/// lines, and where each starts.
fn refusing_block(rows: usize, refused: &[usize]) -> (Vec<Vec<u8>>, Vec<u64>) {
    let lines: Vec<Vec<u8>> = (0..rows)
        .map(|r| {
            let day = if refused.contains(&r) { "2020-02-30" } else { "2020-02-28" };
            format!("\\N\t\\N\t\\N\t{r}\t{day}\t\\N\tuntracked").into_bytes()
        })
        .collect();
    let mut offsets = Vec::with_capacity(rows);
    let mut end = 0u64;
    for line in &lines {
        offsets.push(end);
        end += line.len() as u64 + 1;
    }
    (lines, offsets)
}

/// The refusal `gathered` answers, as its line, offset, column and value.
fn refusal(gathered: BlockGathered) -> (u64, u64, usize, String) {
    match gathered {
        BlockGathered::Refused(FieldRefusal { line, offset, column, value, .. }) => {
            (line, offset, column, value.expect("a value, not a NULL"))
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// **A refused field's line is `COPY`'s number for its row however the
/// block was cut**: the block observes a prefix of its rows itself and
/// pieces cut at random the rest, folded a window at a time, and the
/// first of two refused rows is named by its place among all the block's
/// rows, `1` being the first.
///
/// **Told to ignore them, the block records the same first and counts
/// every one**, the two rows being one when they coincide.
#[test]
fn a_refusal_is_numbered_by_its_row_in_the_block_however_it_was_cut() {
    let mut rng = Rng(0x0031_0122);
    for round in 0..600 {
        let invalid = match round % 2 {
            0 => PostgresInvalidValues::Default,
            _ => PostgresInvalidValues::Ignore,
        };
        let rows = 1 + rng.below(150) as usize;
        let first = rng.below(rows as u64) as usize;
        let second = first + rng.below((rows - first) as u64) as usize;
        let (lines, offsets) = refusing_block(rows, &[first, second]);
        let account = Arc::new(StatisticsAccount::default());
        let charge = Charge::new(Arc::clone(&account), Term::Gathering);
        let columns = join_columns_under(invalid);
        let mut observer: Box<dyn BlockObserver> =
            Box::new(Gatherer::block(sized(64, None, None), columns, charge));
        let prefix = rng.below(rows as u64 + 1) as usize;
        for (line, &offset) in lines.iter().zip(&offsets).take(prefix) {
            observer.observe_row(offset, line);
        }
        let cut_odds = 1 + rng.below(20);
        let window_pieces = 1 + rng.below(4) as usize;
        let mut window = vec![observer.piece()];
        for (line, &offset) in lines.iter().zip(&offsets).skip(prefix) {
            if rng.below(cut_odds) == 0 {
                if window.len() == window_pieces {
                    for piece in window.drain(..) {
                        observer.absorb(piece);
                    }
                }
                window.push(observer.piece());
            }
            window.last_mut().expect("a window holds a piece").observe_row(offset, line);
        }
        for piece in window {
            observer.absorb(piece);
        }
        let end = offsets[rows - 1] + lines[rows - 1].len() as u64 + 1;
        let expected = (first as u64 + 1, offsets[first], 4, "2020-02-30".to_string());
        if invalid == PostgresInvalidValues::Default {
            assert_eq!(observer.take_ignored(), None, "round {round}");
            assert_eq!(refusal(observer.finish(end)), expected, "round {round}");
            continue;
        }
        let ignored = observer.take_ignored().expect("the block went past one");
        let [recorded] = &ignored.columns[..] else {
            panic!("round {round}: one column refuses, recorded {ignored:?}");
        };
        let FieldRefusal { line, offset, column, value, .. } = recorded.first.clone();
        assert_eq!((line, offset, column, value.unwrap()), expected, "round {round}");
        assert_eq!(recorded.count, 1 + u64::from(second != first), "round {round}");
        assert!(observer.finish(end).gathered().is_some(), "round {round}");
    }
}

/// **A piece made before its block declined refuses, numbered past every
/// row before it**, the rows of the pieces that declined among them: a
/// declined observer reads no field, and counts its rows all the same.
#[test]
fn a_refusal_past_a_decline_counts_the_declined_rows() {
    let (lines, offsets) = refusing_block(10, &[8]);
    let account = Arc::new(StatisticsAccount::bounded_by(Some(0)));
    let charge = Charge::new(Arc::clone(&account), Term::Gathering);
    let mut observer: Box<dyn BlockObserver> =
        Box::new(Gatherer::block(sized(64, None, None), join_columns(), charge));
    // Three pieces made before any is handed a row, as the leader makes a
    // window's: three rows, five, then the refused row and one after it.
    let mut window: Vec<Box<dyn BlockObserver>> = (0..3).map(|_| observer.piece()).collect();
    for (r, (line, &offset)) in lines.iter().zip(&offsets).enumerate() {
        let piece = match r {
            0..3 => 0,
            3..8 => 1,
            _ => 2,
        };
        window[piece].observe_row(offset, line);
    }
    let mut pieces = window.into_iter();
    observer.absorb(pieces.next().expect("three pieces"));
    let mut late = observer.piece();
    late.observe_row(offsets[8], &lines[8]);
    drop(late);
    for piece in pieces {
        observer.absorb(piece);
    }
    let end = offsets[9] + lines[9].len() as u64 + 1;
    assert_eq!(refusal(observer.finish(end)), (9, offsets[8], 4, "2020-02-30".to_string()));
    assert_eq!(account.held().now, StatisticsTerms::default());
}

/// **A full map grows into the table [`grown_map_heap`] charges**, and
/// every table's size follows from its capacity as [`map_heap`] reads it:
/// the arithmetic the account's interned term is, against the capacities
/// the standard library's map actually reports as it fills. That the
/// arithmetic is the allocation is the instrument build's to show
/// (`crates/pgdt/tests/statistics_account.rs`).
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

/// **A map made to hold `k` entries allocates the table a full map of
/// `k − 1` grows into**, which is what [`DictionaryGatherer::renumber`]
/// charges ahead of rebuilding its interning map, and reports a capacity
/// [`map_heap`] reads back as that table. A map pruned in place does not,
/// which is why the map is rebuilt: whether an erase hands its slot back
/// depends on the hash seeding, so a pruned map's capacity may fall or
/// stand, but under every seeding the table [`map_heap`] reads from it is
/// larger than the one a rebuild allocates for the entries it kept.
#[test]
fn a_map_made_to_hold_its_entries_allocates_the_table_it_is_charged() {
    for k in 1..5000usize {
        let map: HashMap<String, u32> = HashMap::with_capacity(k);
        let table = grown_map_heap::<String, u32>(k - 1);
        assert_eq!(map_heap::<String, u32>(map.capacity()), table, "{k} entries");
    }
    let mut pruned: HashMap<String, u32> = (0..100).map(|i| (i.to_string(), i)).collect();
    pruned.retain(|_, id| *id % 10 == 0);
    assert!(
        map_heap::<String, u32>(pruned.capacity())
            > grown_map_heap::<String, u32>(pruned.len() - 1),
        "a pruned table is no larger than the table its entries are rebuilt into"
    );
    pruned.retain(|_, _| false);
    assert!(map_heap::<String, u32>(pruned.capacity()) > 0, "an emptied map lets its table go");
}

/// **A vector pushed or extended through [`push_charged`] and
/// [`reserve_charged`] grows into exactly the capacity it was charged**,
/// the charge ahead of each growth being the capacity after it
/// (`docs/design/runtime-invariants.md`, "RT12").
#[test]
fn a_vector_grows_into_the_capacity_it_is_charged() {
    let mut charge = Charge::new(Arc::default(), Term::Gathering);
    let (mut groups, mut bounds): (Vec<RowGroup>, Vec<Option<Bounds>>) = (vec![], vec![]);
    let mut growths = 0;
    for i in 0..100_000u64 {
        let before = groups.capacity();
        if i % 7 == 0 {
            reserve_charged(&mut groups, (i % 13) as usize, &mut charge);
            groups.extend((0..i % 13).map(|rows| RowGroup { rows, bytes: 0 }));
        } else {
            push_charged(&mut groups, RowGroup { rows: i, bytes: i }, &mut charge);
        }
        push_charged(&mut bounds, None, &mut charge);
        growths += usize::from(groups.capacity() != before);
        assert_eq!(charge.charged().0, vec_heap(&groups) + vec_heap(&bounds), "push {i}");
    }
    assert!(growths > 10, "only {growths} growths");
}

fn cmp_text(a: &str, b: &str) -> Ordering {
    a.as_bytes().cmp(b.as_bytes())
}

#[test]
fn a_long_texts_stored_bounds_are_on_the_right_side_of_it() {
    let long = format!("{}é{}", "a".repeat(250), "z".repeat(40));
    let lower = text_prefix(&long, DICTIONARY_ENTRY_MAX_BYTES);
    assert!(lower.len() <= DICTIONARY_ENTRY_MAX_BYTES);
    assert_ne!(cmp_text(lower, &long), Ordering::Greater);
    let upper = text_upper(&long, false).unwrap();
    assert!(upper.len() <= DICTIONARY_ENTRY_MAX_BYTES);
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
    let lower = text_prefix(&text, DICTIONARY_ENTRY_MAX_BYTES);
    assert!(lower.len() <= DICTIONARY_ENTRY_MAX_BYTES);
    assert!(decode_bytea(lower).unwrap() <= value);
    let upper = bytea_upper(&text).unwrap();
    assert!(upper.len() <= DICTIONARY_ENTRY_MAX_BYTES);
    assert!(decode_bytea(&upper).unwrap() > value);
    assert!(bytea_upper(&render_bytea(&[0xFF; 300])).is_none());
}

/// **A `bytea` column gathers alike in `hex` and in `escape`**: bounds
/// and row order over random groups, sorted in some rounds, of values
/// short, either side of [`BYTEA_ESCAPE_HEAD_BYTES`] and past it, half
/// sharing a head that long, over bytes each escaping differently.
#[test]
fn a_bytea_column_gathers_alike_in_either_output_form() {
    use pgdt_codec::decode::render_bytea_escape;

    let mut rng = Rng(0x000b_17ea);
    let byte = |rng: &mut Rng| [0x00, 0x41, 0x5c, 0x7f, 0xff][rng.below(5) as usize];
    let (mut bounded, mut ordered) = (0usize, 0usize);
    for round in 0..300 {
        let shared: Vec<u8> = (0..BYTEA_ESCAPE_HEAD_BYTES).map(|_| byte(&mut rng)).collect();
        let mut groups: Vec<Vec<Vec<u8>>> = (0..1 + rng.below(4))
            .map(|_| {
                (0..rng.below(6))
                    .map(|_| {
                        let len = match rng.below(3) {
                            0 => rng.below(4) as usize,
                            1 => BYTEA_ESCAPE_HEAD_BYTES - 2 + rng.below(5) as usize,
                            _ => 250 + rng.below(40) as usize,
                        };
                        let mut v: Vec<u8> = (0..len).map(|_| byte(&mut rng)).collect();
                        if rng.below(2) == 0 {
                            v.splice(0..0, shared.iter().copied());
                        }
                        v
                    })
                    .collect()
            })
            .collect();
        if round % 3 == 0 {
            let mut all = groups.concat();
            all.sort();
            groups = vec![all];
        }
        let gathered = |render: fn(&[u8]) -> String| {
            let mut gatherer = BoundsGatherer::new(CompareKind::Bytea);
            let mut charge = Charge::new(Arc::default(), Term::Gathering);
            for group in &groups {
                let mut state = gatherer.fresh_group();
                for v in group {
                    gatherer
                        .observe(&mut state, &render(v), None)
                        .expect("no value here is refused");
                }
                gatherer.close_group(state, &mut charge);
            }
            gatherer.finish()
        };
        let hex = gathered(render_bytea);
        assert_eq!(gathered(render_bytea_escape), hex, "round {round}");
        bounded += hex.groups.iter().flatten().count();
        ordered += usize::from(hex.sortedness != Sortedness::Unsorted);
    }
    assert!(bounded > 300 && ordered > 50, "{bounded} groups bounded, {ordered} ordered");
}
