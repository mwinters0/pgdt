use std::num::NonZeroU64;

use super::*;
use crate::index::{Checkpoint, Index};
use crate::test_support::{Pass, forward, gzip, gzip_members, payload, slice};

fn passes() -> Vec<(String, Vec<u8>, Pass)> {
    let plaintext = payload(900_000, 43);
    let mut out = Vec::new();
    for level in [0, 1, 6, 9] {
        let file = gzip(&plaintext, level);
        let pass = forward(&file, 16 * 1024, 1 << 20, 1 << 20).unwrap();
        out.push((format!("level {level}"), file, pass));
    }
    let tail = payload(70_000, 47);
    let file = gzip_members(&[&plaintext, b"", &tail, &tail], 6);
    let pass = forward(&file, 16 * 1024, 1 << 20, 1 << 20).unwrap();
    out.push(("four members".to_owned(), file, pass));
    out
}

fn plaintext_of<'a>(pass: &'a Pass, task: &UnitTask) -> &'a [u8] {
    slice(&pass.plaintext, task.plaintext_range())
}

#[test]
fn every_unit_decodes_alone_from_exactly_its_range() {
    for (name, file, pass) in passes() {
        assert!(pass.index.unit_count() >= 2, "{name}");
        for unit in 0..pass.index.unit_count() {
            let task = pass.index.unit(unit).unwrap();
            let mut out = vec![0u8; plaintext_of(&pass, &task).len()];
            task.decode(&pass.windows[unit], slice(&file, task.compressed_range()), &mut out)
                .unwrap_or_else(|e| panic!("{name} unit {unit}: {e}"));
            assert!(out == plaintext_of(&pass, &task), "{name} unit {unit}");
        }
    }
}

#[test]
fn a_unit_read_in_pieces_is_the_unit() {
    for (name, file, pass) in passes() {
        for unit in [0, 1, pass.index.unit_count() / 2, pass.index.unit_count() - 1] {
            let task = pass.index.unit(unit).unwrap();
            let mut read = task.begin(&pass.windows[unit]).unwrap();
            let mut out = Vec::new();
            let (mut buf, mut feed) = ([0u8; 5], 1);
            while !read.is_finished() {
                let at = read.compressed_position() as usize;
                let input = &file[at..(at + feed).min(file.len())];
                let progress = read.read(input, &mut buf).unwrap();
                out.extend_from_slice(&buf[..progress.produced]);
                assert_eq!(read.position(), task.plaintext_range().start + out.len() as u64);
                feed = feed % 11 + 1;
            }
            assert!(out == plaintext_of(&pass, &task), "{name} unit {unit}");
        }
    }
}

#[test]
fn a_unit_reads_no_further_than_its_end() {
    let (_, file, pass) = passes().remove(2);
    let task = pass.index.unit(3).unwrap();
    let start = task.compressed_range().start as usize;
    let mut out = vec![0u8; plaintext_of(&pass, &task).len()];
    task.decode(&pass.windows[3], &file[start..], &mut out).unwrap();
    assert!(out == plaintext_of(&pass, &task));
}

#[test]
fn a_damaged_unit_is_refused() {
    for (name, file, pass) in passes() {
        let unit = pass.index.unit_count() / 2;
        let task = pass.index.unit(unit).unwrap();
        let range = task.compressed_range();
        let mut damaged = slice(&file, range.clone()).to_vec();
        let middle = damaged.len() / 2;
        damaged[middle] ^= 0x04;
        let mut out = vec![0u8; plaintext_of(&pass, &task).len()];
        assert!(task.decode(&pass.windows[unit], &damaged, &mut out).is_err(), "{name}");
        // A window of the right length but the wrong bytes, where some
        // back-reference reads it: stored blocks reference nothing.
        if task.window_len() > 0 && name != "level 0" {
            let wrong = vec![b'x'; task.window_len()];
            assert!(task.decode(&wrong, slice(&file, range), &mut out).is_err(), "{name}");
        }
    }
}

#[test]
fn a_recorded_crc_is_compared() {
    let (_, file, pass) = passes().remove(2);
    let mut checkpoints = pass.index.checkpoints().to_vec();
    let unit = checkpoints.len() / 2;
    let stored = checkpoints[unit].crc.unwrap();
    checkpoints[unit].crc = Some(stored ^ 1);
    let index = Index::from_parts(
        pass.index.spacing(),
        checkpoints,
        pass.index.member_count(),
        pass.index.end(),
    )
    .unwrap();
    let task = index.unit(unit).unwrap();
    let mut out = vec![0u8; plaintext_of(&pass, &task).len()];
    let err = task.decode(&pass.windows[unit], slice(&file, task.compressed_range()), &mut out);
    assert_eq!(
        err,
        Err(Error::UnitCheck {
            plaintext: task.plaintext_range().start,
            stored: stored ^ 1,
            computed: stored
        })
    );
}

#[test]
fn a_unit_ending_elsewhere_than_its_index_says_is_refused() {
    let (_, file, pass) = passes().remove(2);
    // A unit whose end is a block boundary far enough into its member that a
    // shifted end keeps a full window, with room either side to shift into.
    let c = pass.index.checkpoints();
    let unit = (0..c.len() - 2)
        .find(|&k| {
            c[k + 1].plaintext > 40_000
                && c[k + 1].plaintext - c[k].plaintext > 1
                && c[k + 2].plaintext - c[k + 1].plaintext > 1
        })
        .expect("a unit ending at an interior block boundary");
    for shift in [-1i64, 1] {
        let mut checkpoints = c.to_vec();
        checkpoints[unit + 1].plaintext =
            checkpoints[unit + 1].plaintext.wrapping_add_signed(shift);
        let index = Index::from_parts(
            pass.index.spacing(),
            checkpoints,
            pass.index.member_count(),
            pass.index.end(),
        )
        .unwrap();
        let task = index.unit(unit).unwrap();
        let len = (task.plaintext_range().end - task.plaintext_range().start) as usize;
        let mut out = vec![0u8; len];
        let err = task.decode(&pass.windows[unit], slice(&file, task.compressed_range()), &mut out);
        assert!(matches!(err, Err(Error::IndexMismatch { .. })), "shift {shift}: {err:?}");
    }
}

#[test]
fn a_short_supply_is_truncated_and_a_wrong_window_length_refused() {
    let (_, file, pass) = passes().remove(4);
    for unit in 0..pass.index.unit_count() {
        let task = pass.index.unit(unit).unwrap();
        let range = task.compressed_range();
        let mut out = vec![0u8; plaintext_of(&pass, &task).len()];
        let short = &file[range.start as usize..range.end as usize - 1];
        let err = task.decode(&pass.windows[unit], short, &mut out);
        assert!(matches!(err, Err(Error::Truncated { .. })), "unit {unit}: {err:?}");
    }
    let task = pass.index.unit(1).unwrap();
    assert!(task.window_len() > 0);
    assert!(matches!(task.begin(&[]), Err(Error::WindowLength { .. })));
    let mut out = vec![0u8; 3];
    assert!(matches!(
        task.decode(&pass.windows[1], &file, &mut out),
        Err(Error::BufferSize { .. })
    ));
}

#[test]
fn a_unit_of_whole_members_needs_no_crc_of_its_own() {
    let parts: Vec<Vec<u8>> = (0..6).map(|i| payload(9_000 + i * 100, 53 + i as u64)).collect();
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    let file = gzip_members(&refs, 6);
    let pass = forward(&file, 1 << 30, 1 << 20, 1 << 20).unwrap();
    // One unit of all six members, as a walk would coalesce them.
    let end = pass.index.end().unwrap();
    let index = Index::from_parts(
        NonZeroU64::new(1 << 30).unwrap(),
        vec![Checkpoint::member(0, 0)],
        pass.index.member_count(),
        Some(end),
    )
    .unwrap();
    let task = index.unit(0).unwrap();
    let mut out = vec![0u8; end.plaintext as usize];
    task.decode(&[], &file, &mut out).unwrap();
    assert_eq!(out, parts.concat());
    // Each member's own trailer is what checks it.
    let mut damaged = file.clone();
    let third = refs[..2].iter().map(|p| gzip(p, 6).len()).sum::<usize>() + gzip(refs[2], 6).len();
    damaged[third - 6] ^= 1;
    assert!(matches!(task.decode(&[], &damaged, &mut out), Err(Error::MemberCheck { .. })));
}
