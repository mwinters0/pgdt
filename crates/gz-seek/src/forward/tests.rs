use super::*;
use crate::test_support::{drive, forward, gzip, gzip_members, payload, spacing};

const SPACING: u64 = 64 * 1024;

/// Every level `flate2` writes a different mix of block kinds at: stored
/// alone, the fastest, the default, the slowest.
const LEVELS: [u32; 4] = [0, 1, 6, 9];

#[test]
fn a_pass_yields_the_plaintext_and_checks_each_unit() {
    let plaintext = payload(1_500_000, 7);
    for level in LEVELS {
        let pass = forward(&gzip(&plaintext, level), SPACING, 1 << 20, 1 << 20).unwrap();
        assert_eq!(pass.plaintext, plaintext, "level {level}");
        assert_eq!(pass.index.size(), Size::Exact(plaintext.len() as u64));
        // Not more: zlib-rs's level 1 writes one block nearly to the end.
        assert!(pass.index.checkpoints().len() >= 2, "level {level} placed too few checkpoints");
        for unit in 0..pass.index.unit_count() {
            let task = pass.index.unit(unit).unwrap();
            let range = task.plaintext_range();
            let bytes = &plaintext[range.start as usize..range.end as usize];
            // Someone else's CRC-32, so the engine's is checked too.
            let mut crc = flate2::Crc::new();
            crc.update(bytes);
            assert_eq!(task.checkpoint().crc, Some(crc.sum()), "level {level} unit {unit}");
        }
    }
}

#[test]
fn every_checkpoint_follows_the_placement_rule_and_holds_its_window() {
    let plaintext = payload(1_500_000, 11);
    for level in LEVELS {
        let pass = forward(&gzip(&plaintext, level), SPACING, 1 << 20, 1 << 20).unwrap();
        let checkpoints = pass.index.checkpoints();
        assert_eq!(
            checkpoints[0],
            Checkpoint { crc: checkpoints[0].crc, ..Checkpoint::member(0, 0) }
        );
        for pair in checkpoints.windows(2) {
            let mark = (pair[0].plaintext / SPACING + 1) * SPACING;
            assert!(pair[1].plaintext >= mark, "level {level}: {pair:?} placed before its mark");
            assert!(matches!(pair[1].start, Start::Block { .. }), "level {level}: one member");
        }
        for (checkpoint, window) in checkpoints.iter().zip(&pass.windows) {
            let at = checkpoint.plaintext as usize;
            assert_eq!(window.len(), checkpoint.window_len());
            assert_eq!(window[..], plaintext[at - window.len()..at], "level {level} at {at}");
        }
    }
}

#[test]
fn the_index_is_one_whatever_the_buffers() {
    let plaintext = payload(400_000, 13);
    for level in [0, 6] {
        let file = gzip(&plaintext, level);
        let reference = forward(&file, 16 * 1024, file.len(), 1 << 20).unwrap();
        for (input, out) in [(1, 1 << 16), (7, 13), (4096, 1), (65_536, 100_003), (file.len(), 3)] {
            let pass = forward(&file, 16 * 1024, input, out).unwrap_or_else(|e| {
                panic!("level {level}, buffers {input}/{out}, {} bytes: {e:?}", file.len())
            });
            assert_eq!(pass.index, reference.index, "level {level}, buffers {input}/{out}");
            assert_eq!(pass.windows, reference.windows, "level {level}, buffers {input}/{out}");
            assert_eq!(pass.plaintext, plaintext, "level {level}, buffers {input}/{out}");
        }
    }
}

#[test]
fn every_member_start_is_a_checkpoint_with_no_window() {
    let (a, b) = (payload(300_000, 3), payload(200_000, 5));
    let parts: [&[u8]; 4] = [&a, b"", &b, b"tail"];
    let file = gzip_members(&parts, 6);
    let pass = forward(&file, SPACING, 1 << 20, 1 << 20).unwrap();
    assert_eq!(pass.plaintext, [&a[..], &b, b"tail"].concat());
    let members: Vec<_> = pass
        .index
        .checkpoints()
        .iter()
        .filter(|c| c.start == Start::Member)
        .map(|c| c.plaintext)
        .collect();
    let (a, b) = (a.len() as u64, b.len() as u64);
    assert_eq!(members, [0, a, a, a + b]);
    let mut offset = 0;
    let mut starts = Vec::new();
    for part in parts {
        starts.push(offset * 8);
        offset += gzip(part, 6).len() as u64;
    }
    let bits: Vec<_> = pass
        .index
        .checkpoints()
        .iter()
        .filter(|c| c.start == Start::Member)
        .map(|c| c.bit)
        .collect();
    assert_eq!(bits, starts);
    // A window never reaches back past its member's start.
    for (c, w) in pass.index.checkpoints().iter().zip(&pass.windows) {
        if c.plaintext > a && c.plaintext < a + 32 * 1024 && c.start != Start::Member {
            assert_eq!(w.len() as u64, c.plaintext - a);
        }
    }
}

#[test]
fn a_damaged_trailer_is_refused() {
    let plaintext = payload(100_000, 17);
    let file = gzip(&plaintext, 6);
    let n = file.len();
    let mut crc = file.clone();
    crc[n - 8] ^= 1;
    assert!(matches!(
        forward(&crc, SPACING, 4096, 4096),
        Err(Error::MemberCheck { offset: 0, .. })
    ));
    let mut len = file.clone();
    len[n - 1] ^= 1;
    assert!(matches!(
        forward(&len, SPACING, 4096, 4096),
        Err(Error::MemberLength { offset: 0, .. })
    ));
    let mut data = file;
    data[n / 2] ^= 0x10;
    assert!(forward(&data, SPACING, 4096, 4096).is_err(), "a flipped bit inside the deflate data");
}

#[test]
fn a_cut_file_is_truncated_wherever_it_was_cut() {
    let file = gzip_members(&[&payload(50_000, 19), &payload(20_000, 23)], 6);
    for cut in [0, 1, 5, 9, 10, 200, file.len() / 2, file.len() - 9, file.len() - 1] {
        let result = forward(&file[..cut], SPACING, 4096, 4096);
        assert!(matches!(result, Err(Error::Truncated { .. })), "cut at {cut}: {:?}", result.err());
    }
}

#[test]
fn what_follows_the_last_member_must_be_a_member() {
    let mut file = gzip(&payload(10_000, 29), 6);
    let end = file.len() as u64;
    file.extend_from_slice(&[0, 0, 0, 0]);
    assert_eq!(forward(&file, SPACING, 4096, 4096).err(), Some(Error::NotGzip { offset: end }));
    assert_eq!(
        forward(b"not gzip at all", SPACING, 4096, 4096).err(),
        Some(Error::NotGzip { offset: 0 })
    );
}

#[test]
fn a_resumed_pass_builds_the_uninterrupted_index() {
    let plaintext = payload(600_000, 31);
    let tail = payload(100_000, 37);
    for file in
        [gzip(&plaintext, 6), gzip(&plaintext, 0), gzip_members(&[&plaintext, &tail, &tail], 6)]
    {
        let whole = forward(&file, 32 * 1024, 1 << 20, 1 << 20).unwrap();
        for stop in [1, 2, 5, whole.index.checkpoints().len() - 1] {
            let first = drive(
                Forward::new(spacing(32 * 1024)).unwrap(),
                &file,
                5000,
                7000,
                Vec::new(),
                Some(stop),
            )
            .unwrap();
            assert!(!first.index.is_complete());
            let last = first.index.checkpoints().len() - 1;
            let at = first.index.checkpoints()[last];
            let resumed = Forward::resume(&first.index, &first.windows[last]).unwrap();
            assert_eq!(resumed.position(), at.plaintext);
            let mut windows = first.windows.clone();
            if at.start == Start::Member {
                windows.pop();
            }
            let rest = drive(resumed, &file, 3000, 9000, windows, None).unwrap();
            assert_eq!(rest.index, whole.index, "stopped after checkpoint {stop}");
            assert_eq!(rest.windows, whole.windows, "stopped after checkpoint {stop}");
            assert_eq!(rest.plaintext, whole.plaintext[at.plaintext as usize..]);
        }
    }
}

#[test]
fn a_resume_refuses_what_no_pass_left() {
    let file = gzip(&payload(200_000, 41), 6);
    let whole = forward(&file, SPACING, 1 << 20, 1 << 20).unwrap();
    assert!(matches!(Forward::resume(&whole.index, &[]), Err(Error::InvalidIndex { .. })));
    let partial =
        drive(Forward::new(spacing(SPACING)).unwrap(), &file, 4096, 4096, Vec::new(), Some(2))
            .unwrap();
    assert!(matches!(Forward::resume(&partial.index, &[]), Err(Error::WindowLength { .. })));
}

#[test]
fn an_empty_member_and_an_empty_file() {
    let pass = forward(&gzip(b"", 6), SPACING, 4096, 4096).unwrap();
    assert_eq!(pass.index.size(), Size::Exact(0));
    assert_eq!(pass.index.unit_count(), 1);
    assert_eq!(forward(b"", SPACING, 4096, 4096).err(), Some(Error::Truncated { offset: 0 }));
}
