use super::*;
use crate::test_support::{forward, gzip, gzip_members, payload, spacing};

fn pass_index() -> Index {
    let a = payload(500_000, 83);
    forward(&gzip_members(&[&a, &a[..1000]], 6), 40_000, 1 << 20, 1 << 20).unwrap().index
}

#[test]
fn an_index_rebuilt_from_its_parts_is_the_same_index() {
    let index = pass_index();
    let rebuilt = Index::from_parts(
        index.spacing(),
        index.checkpoints().to_vec(),
        index.member_count(),
        index.end(),
    )
    .unwrap();
    assert_eq!(rebuilt, index);
    let clone = index.clone();
    assert!(Arc::ptr_eq(&clone.inner, &index.inner), "a clone shares, never copies");
}

#[test]
fn parts_no_pass_builds_are_refused() {
    let index = pass_index();
    let (s, members, end) = (index.spacing(), index.member_count(), index.end());
    let good = index.checkpoints().to_vec();
    let counted = |checkpoints: Vec<Checkpoint>, members: u64, end: Option<End>| {
        matches!(Index::from_parts(s, checkpoints, members, end), Err(Error::InvalidIndex { .. }))
    };
    let refused =
        |checkpoints: Vec<Checkpoint>, end: Option<End>| counted(checkpoints, members, end);
    assert!(refused(Vec::new(), None));
    let mut late = good.clone();
    late[0].bit = 8;
    assert!(refused(late, end));
    let mut backward = good.clone();
    backward.swap(2, 3);
    assert!(refused(backward, end));
    let mut window = good.clone();
    let i = window.iter().position(|c| matches!(c.start, Start::Block { .. })).unwrap();
    window[i].start = Start::Block { window: 7 };
    assert!(refused(window, end));
    let mut unchecked = good.clone();
    unchecked[i].crc = None;
    assert!(refused(unchecked, end));
    let early = End { plaintext: good.last().unwrap().plaintext - 1, ..end.unwrap() };
    assert!(refused(good.clone(), Some(early)));
    assert!(counted(good.clone(), 1, end));
    assert!(counted(good.clone(), 1, None));
    let mut inside = good;
    let m = inside.iter().rposition(|c| c.start == Start::Member).unwrap();
    inside[m].bit += 3;
    assert!(refused(inside, end));
}

#[test]
fn units_are_found_by_plaintext() {
    let index = pass_index();
    let total = match index.size() {
        Size::Exact(n) => n,
        Size::Open { .. } => panic!("a finished pass"),
    };
    for unit in 0..index.unit_count() {
        let range = index.unit(unit).unwrap().plaintext_range();
        if range.is_empty() {
            continue;
        }
        assert_eq!(index.unit_at(range.start), Some(unit));
        assert_eq!(index.unit_at(range.end - 1), Some(unit));
    }
    assert_eq!(index.unit_at(total), None);
    assert_eq!(index.unit(index.unit_count()), None);
}

#[test]
fn an_unfinished_index_is_open_and_its_last_unit_undecodable() {
    let index = pass_index();
    let checkpoints = index.checkpoints()[..4].to_vec();
    let mut open = checkpoints.clone();
    open[3].crc = None;
    let partial = Index::from_parts(spacing(40_000), open, 1, None).unwrap();
    assert_eq!(partial.size(), Size::Open { at_least: checkpoints[3].plaintext });
    assert_eq!(partial.unit_count(), 3);
    assert!(partial.unit(3).is_none());
    assert_eq!(partial.unit(2), index.unit(2));
}

#[test]
fn a_unit_s_compressed_range_reaches_the_byte_holding_its_last_bit() {
    let file = gzip(&payload(300_000, 89), 6);
    let index = forward(&file, 20_000, 1 << 20, 1 << 20).unwrap().index;
    for unit in 0..index.unit_count() {
        let task = index.unit(unit).unwrap();
        let range = task.compressed_range();
        assert_eq!(range.start, task.checkpoint().bit / 8);
        match index.checkpoints().get(unit + 1) {
            Some(next) => assert_eq!(range.end, next.bit.div_ceil(8)),
            None => assert_eq!(range.end, file.len() as u64),
        }
    }
}
