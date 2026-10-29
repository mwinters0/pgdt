use super::*;
use crate::engine::{Engine, Event};
use crate::test_support::{gzip, gzip_members, payload};

/// Every block boundary the engine stops at in `file`.
fn boundaries(file: &[u8]) -> Vec<u64> {
    let mut engine = Engine::at_member(0, 0).unwrap();
    let (mut found, mut out) = (Vec::new(), vec![0u8; 1 << 16]);
    loop {
        let at = engine.compressed_position() as usize;
        let step = engine.step(&file[at..], &mut out).unwrap();
        if let Some(Event::Block { bit }) = step.event {
            found.push(bit);
        }
        if !step.progressed() {
            break;
        }
    }
    found
}

fn held(file: &[u8]) -> Input {
    let mut input = Input::new(0, file.len() as u64);
    input.push(file);
    input
}

/// **From any bit, the search reaches the first dynamic or stored block that
/// is not its member's last**, past the few false candidates a decode from
/// each refuses.
#[test]
fn the_first_findable_block_after_any_bit_is_found() {
    let plaintext = payload(800_000, 7);
    for level in [0, 6, 9] {
        let file = gzip(&plaintext, level);
        let input = held(&file);
        // The stream's first block, which no boundary reports, begins after
        // `flate2`'s ten-byte header.
        let truth = [vec![80], boundaries(&file)].concat();
        let findable: Vec<u64> =
            truth.iter().copied().filter(|&b| matches!(input.peek(b) & 7, 0b100 | 0b000)).collect();
        assert!(findable.len() > 4, "level {level}: too few blocks");
        let mut false_candidates = std::collections::BTreeSet::new();
        let end = 8 * file.len() as u64;
        for from in (0..end).step_by(8 * 997) {
            let expected = findable.iter().copied().find(|&b| b >= from);
            let mut at = from;
            let found = loop {
                let mut inflater = Inflater::new();
                match find(&input, at, end, &mut inflater) {
                    Found::At(Candidate::Block { bit, stored: None, .. })
                        if truth.contains(&bit) =>
                    {
                        break Some(bit);
                    }
                    // A stored header's alias, ahead of the header itself.
                    Found::At(Candidate::Block { bit, stored: Some(len), .. })
                        if let Some(&header) =
                            truth.iter().find(|&&t| t >= bit && stored_length_at(t) == len) =>
                    {
                        break Some(header);
                    }
                    Found::At(Candidate::Block { bit, .. }) => {
                        false_candidates.insert(bit);
                        at = bit + 1;
                    }
                    Found::At(Candidate::Member { offset }) => {
                        assert_eq!(offset, 0, "level {level}: a member start mid-file");
                        at = 1;
                    }
                    Found::Nothing => break None,
                    Found::Input(_) => unreachable!("the whole file is held"),
                }
            };
            assert_eq!(found, expected, "level {level}, from bit {from}");
        }
        // Rare enough that a chunk's search meets few: each costs a decode
        // that soon fails.
        let per_bit = false_candidates.len() as f64 / end as f64;
        assert!(per_bit < 1e-5, "level {level}: {} false candidates", false_candidates.len());
    }
}

#[test]
fn a_member_start_is_found_at_its_byte() {
    let (a, b) = (payload(100_000, 11), payload(50_000, 13));
    let file = gzip_members(&[&a, &b], 6);
    let second = gzip(&a, 6).len() as u64;
    let input = held(&file);
    let mut inflater = Inflater::new();
    // From past the first member's last findable block, the next member's
    // header is the first thing found.
    let last = *boundaries(&gzip(&a, 6)).last().unwrap();
    assert_eq!(
        find(&input, last + 1, 8 * file.len() as u64, &mut inflater),
        Found::At(Candidate::Member { offset: second })
    );
}

#[test]
fn a_search_short_of_its_input_asks_for_more() {
    let file = gzip(&payload(200_000, 17), 6);
    let mut input = Input::new(0, file.len() as u64);
    input.push(&file[..1000]);
    let mut inflater = Inflater::new();
    // Past the first member's header, nothing in the first thousand bytes.
    let found = find(&input, 81, 8 * file.len() as u64, &mut inflater);
    assert!(matches!(found, Found::Input(bit) if bit < 8000), "{found:?}");
}
