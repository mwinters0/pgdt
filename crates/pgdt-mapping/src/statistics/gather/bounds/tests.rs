use super::{BYTEA_ESCAPE_HEAD_BYTES, BoundsGatherer, bytea_upper, text_upper};
use crate::statistics::account::{Charge, Term};
use crate::statistics::gather::clip::text_prefix;
use crate::statistics::test_support::{Rng, join_type, value};
use pgdt_codec::compare::CompareKind;
use pgdt_codec::decode::{decode_bytea, render_bytea};
use pgdt_codec::internal::ValueKeyExt;
use pgdt_codec::key::ValueKey;
use pgdt_model::internal::{ColumnStatisticsExt, ColumnStatisticsParts, UnrepresentableExt};
use pgdt_model::{
    ColumnStatistics, DICTIONARY_ENTRY_MAX_BYTES, Sortedness, Unrepresentable, UnrepresentableTier,
};
use std::cmp::Ordering;
use std::sync::Arc;

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
                ValueKey::keyed_by(kind, a)
                    .unwrap()
                    .compare(&ValueKey::keyed_by(kind, b).unwrap())
                    .unwrap()
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
            let low = ValueKey::keyed_by(kind, &b.min).expect("a stored min keys");
            let high = ValueKey::keyed_by(kind, &b.max).expect("a stored max keys");
            for v in group {
                let key = ValueKey::keyed_by(kind, v).unwrap();
                assert_ne!(
                    low.compare(&key).unwrap(),
                    Ordering::Greater,
                    "{kind:?}: min {:?} above {v:?}",
                    b.min
                );
                assert_ne!(
                    high.compare(&key).unwrap(),
                    Ordering::Less,
                    "{kind:?}: max {:?} below {v:?}",
                    b.max
                );
            }
            if b.max_exact {
                assert!(
                    group.iter().any(|v| ValueKey::keyed_by(kind, v)
                        .unwrap()
                        .compare(&high)
                        .unwrap()
                        == Ordering::Equal)
                );
            }
        }
        let keys: Vec<ValueKey> =
            all.iter().map(|v| ValueKey::keyed_by(kind, v).unwrap()).collect();
        let pairs = keys.windows(2).map(|w| w[1].compare(&w[0]).unwrap());
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
            let key = |v: &String| ValueKey::keyed_by(kind, v);
            values.sort_by(|a, b| match (key(a), key(b)) {
                (Some(a), Some(b)) => a.compare(&b).unwrap(),
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
        assert_eq!(bounds.every.is_some(), counts.iter().any(|c| c.format() > 0), "round {round}");
        assert_eq!(bounds.displayable.is_some(), counts.iter().any(|c| c.engine() > 0));
        kept[0] += usize::from(bounds.every.is_some());
        kept[1] += usize::from(bounds.displayable.is_some());
        let column = ColumnStatistics::from_parts(ColumnStatisticsParts {
            declared_type: None,
            collation: None,
            null_counts: vec![0; groups.len()],
            bounds: Some(bounds),
            datafusion_bounds: None,
            dictionary: None,
            sums: None,
            value_bytes: vec![0; groups.len()],
            unrepresentable: Some(counts.clone()),
        });
        for (v, view) in views.into_iter().enumerate() {
            let taken = |group: &Vec<String>| -> Vec<(String, Option<ValueKey>)> {
                group
                    .iter()
                    .filter(|value| takes(view, tier.of(value)))
                    .map(|value| (value.clone(), ValueKey::keyed_by(kind, value)))
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
                let (low, high) = (
                    ValueKey::keyed_by(kind, &b.min).unwrap(),
                    ValueKey::keyed_by(kind, &b.max).unwrap(),
                );
                assert!(
                    keys.iter().all(|key| low.compare(key).unwrap().is_le()
                        && high.compare(key).unwrap().is_ge()),
                    "{what}: {b:?}"
                );
                assert!(
                    keys.iter().any(|key| low.compare(key).unwrap().is_eq()),
                    "{what}: min {b:?}"
                );
                assert!(
                    keys.iter().any(|key| high.compare(key).unwrap().is_eq()),
                    "{what}: max {b:?}"
                );
                assert_eq!(
                    column.null_count(g, view),
                    Some(match view {
                        StatisticsView::Every => 0,
                        StatisticsView::Representable => counts[g].format(),
                        StatisticsView::Displayable => counts[g].format() + counts[g].engine(),
                    }),
                    "{what}: NULLs"
                );
            }
            let row: Vec<Option<ValueKey>> =
                groups.iter().flat_map(taken).map(|(_, key)| key).collect();
            let expected = match row.iter().map(Option::as_ref).collect::<Option<Vec<&ValueKey>>>()
            {
                None => Sortedness::Unsorted,
                Some(keys) if keys.windows(2).all(|w| w[1].compare(w[0]).unwrap().is_ge()) => {
                    Sortedness::Ascending
                }
                Some(keys) if keys.windows(2).all(|w| w[1].compare(w[0]).unwrap().is_le()) => {
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
