use super::*;
use crate::index::{Size, Start};
use crate::test_support::{bgzf, forward, gzip, payload, slice, spacing};

/// Walk `file` answering each request with exactly its bytes.
fn walk_exactly(file: &[u8], spacing: u64) -> Result<Index> {
    let mut walk = Walk::new(file.len() as u64, self::spacing(spacing));
    let mut reads = 0;
    loop {
        let r = walk.request();
        let bytes = &file[r.offset as usize..(r.offset as usize + r.len).min(file.len())];
        reads += 1;
        if let WalkStep::Done(index) = walk.supply(r.offset, bytes)? {
            assert!(reads > 1);
            return Ok(index);
        }
    }
}

/// Walk `file` with one supply of the whole of it.
fn walk_at_once(file: &[u8], spacing: u64) -> Result<Index> {
    let mut walk = Walk::new(file.len() as u64, self::spacing(spacing));
    match walk.supply(0, file)? {
        WalkStep::Done(index) => Ok(index),
        WalkStep::Need(r) => panic!("the whole file supplied, and {r:?} still asked"),
    }
}

#[test]
fn a_walk_indexes_by_headers_alone_and_every_unit_decodes() {
    let plaintext = payload(1_200_000, 59);
    let file = bgzf(&plaintext);
    let index = walk_exactly(&file, 256 * 1024).unwrap();
    assert_eq!(index, walk_at_once(&file, 256 * 1024).unwrap());
    assert_eq!(index.size(), Size::Exact(plaintext.len() as u64));
    assert_eq!(index.end().unwrap().compressed, file.len() as u64);
    let checkpoints = index.checkpoints();
    assert!(checkpoints.iter().all(|c| c.start == Start::Member && c.crc.is_none()));
    for unit in 0..index.unit_count() {
        let task = index.unit(unit).unwrap();
        let range = task.plaintext_range();
        if unit + 1 < index.unit_count() {
            assert!(range.end - range.start >= 256 * 1024, "unit {unit} under the spacing");
            assert!(range.end - range.start < 256 * 1024 + u64::from(BGZF_MAX_PLAINTEXT));
        }
        let mut out = vec![0u8; (range.end - range.start) as usize];
        task.decode(&[], slice(&file, task.compressed_range()), &mut out).unwrap();
        assert!(out == slice(&plaintext, range), "unit {unit}");
    }
}

#[test]
fn a_walk_reads_ahead_as_far_as_it_is_supplied() {
    let file = bgzf(&payload(500_000, 61));
    let mut walk = Walk::new(file.len() as u64, spacing(1));
    let mut supplies = 0;
    let index = loop {
        let r = walk.request();
        let end = (r.offset as usize + 100_000).min(file.len());
        supplies += 1;
        if let WalkStep::Done(index) = walk.supply(r.offset, &file[r.offset as usize..end]).unwrap()
        {
            break index;
        }
    };
    assert_eq!(index, walk_exactly(&file, 1).unwrap());
    assert!(supplies <= file.len() / 100_000 + 2, "{supplies} supplies");
    // At a spacing of one byte every member begins a unit, the empty last too.
    let members = index.checkpoints().len();
    assert!(members > 5);
}

#[test]
fn a_forward_pass_over_bgzf_agrees_with_the_walk() {
    let plaintext = payload(300_000, 67);
    let file = bgzf(&plaintext);
    let pass = forward(&file, 1, 1 << 20, 1 << 20).unwrap();
    assert_eq!(pass.plaintext, plaintext);
    let walked = walk_exactly(&file, 1).unwrap();
    let bits: Vec<u64> = walked.checkpoints().iter().map(|c| c.bit).collect();
    let members: Vec<u64> = pass
        .index
        .checkpoints()
        .iter()
        .filter(|c| c.start == Start::Member)
        .map(|c| c.bit)
        .collect();
    // Both place every member's start, the empty last's included.
    assert_eq!(members[..bits.len()], bits[..]);
    assert_eq!(pass.index.end(), walked.end());
}

#[test]
fn recognition_reads_the_bc_subfield() {
    let file = bgzf(b"hello");
    let len = bgzf_member_len(&file).unwrap();
    assert!(len <= file.len() as u64);
    assert_eq!(bgzf_member_len(&gzip(b"hello", 6)), None);
    assert_eq!(bgzf_member_len(&file[..BGZF_HEADER_LEN - 1]), None);
}

#[test]
fn a_walk_refuses_what_is_not_bgzf() {
    assert_eq!(
        walk_exactly(&gzip(&payload(1000, 71), 6), 1).err(),
        Some(Error::NotBgzf { offset: 0 })
    );
    assert_eq!(walk_exactly(&[0x42; 64], 1).err(), Some(Error::NotGzip { offset: 0 }));
    assert_eq!(walk_exactly(&[], 1).err(), Some(Error::Truncated { offset: 0 }));
    let file = bgzf(&payload(200_000, 73));
    let cut = &file[..file.len() / 2];
    assert!(matches!(walk_exactly(cut, 1), Err(Error::Truncated { .. })));
    let mut garbage = file.clone();
    garbage.extend_from_slice(&[0; 40]);
    assert_eq!(walk_exactly(&garbage, 1).err(), Some(Error::NotGzip { offset: file.len() as u64 }));
}

#[test]
fn a_supply_short_of_its_request_is_the_file_ending() {
    let file = bgzf(&payload(10_000, 79));
    let mut walk = Walk::new(file.len() as u64 + 100, spacing(1));
    let r = walk.request();
    assert_eq!(walk.supply(r.offset, &file[..10]).err(), Some(Error::Truncated { offset: 10 }));
}
