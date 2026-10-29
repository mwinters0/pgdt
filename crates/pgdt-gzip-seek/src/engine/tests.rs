use super::*;
use crate::test_support::{gzip_members, payload};

/// Members decoded through every input size from one byte up, so that a
/// stream's end falls at every place in a supply: the inflater never takes a
/// trailer's byte, however the supplies were split (RT23).
#[test]
fn a_member_s_end_is_found_at_every_place_in_a_supply() {
    let parts = [payload(3000, 97), payload(1, 101), payload(20_000, 103)];
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    for level in [0, 6] {
        let file = gzip_members(&refs, level);
        for input in 1..=40 {
            let mut engine = Engine::at_member(0, 0).unwrap();
            let mut out = vec![0u8; 4096];
            let (mut plaintext, mut ends) = (Vec::new(), 0);
            loop {
                let at = engine.compressed_position() as usize;
                let supply = &file[at.min(file.len())..(at + input).min(file.len())];
                let step = engine.step(supply, &mut out).unwrap();
                plaintext.extend_from_slice(&out[..step.produced]);
                ends += usize::from(step.event == Some(Event::MemberEnd));
                if !step.progressed() {
                    break;
                }
            }
            assert!(engine.is_between(), "level {level}, input {input}");
            assert_eq!(engine.compressed_position(), file.len() as u64);
            assert_eq!(ends, parts.len());
            assert!(plaintext == parts.concat(), "level {level}, input {input}");
        }
    }
}

/// **A step appending to a vector's spare capacity is a step into a slice**:
/// the same plaintext and the same events at the same offsets, whatever the
/// supplies and the room each step is given, the final block's end inside a
/// step among them.
#[test]
fn a_step_appending_to_spare_capacity_decodes_as_one_into_a_slice() {
    let parts = [payload(3000, 97), payload(1, 101), payload(70_000, 103)];
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    for level in [0, 6, 9] {
        let file = gzip_members(&refs, level);
        for input in [1, 7, 777, 1 << 20] {
            for room in [1, 97, 4096] {
                let what = format!("level {level}, input {input}, room {room}");
                let decode = |appending: bool| {
                    let mut engine = Engine::at_member(0, 0).unwrap();
                    let (mut plaintext, mut slice) = (Vec::new(), vec![0u8; room]);
                    let mut events = Vec::new();
                    loop {
                        let at = engine.compressed_position() as usize;
                        let supply = &file[at.min(file.len())..(at + input).min(file.len())];
                        let step = if appending {
                            plaintext.reserve(room);
                            let before = plaintext.len();
                            let step = engine.step_appending(supply, &mut plaintext).unwrap();
                            assert_eq!(plaintext.len() - before, step.produced, "{what}");
                            step
                        } else {
                            let step = engine.step(supply, &mut slice).unwrap();
                            plaintext.extend_from_slice(&slice[..step.produced]);
                            step
                        };
                        events.extend(step.event.map(|event| (plaintext.len(), event)));
                        if !step.progressed() {
                            break;
                        }
                    }
                    (plaintext, events)
                };
                let (into_slice, appended) = (decode(false), decode(true));
                assert!(appended.0 == parts.concat(), "{what}");
                assert_eq!(appended.1, into_slice.1, "{what}");
            }
        }
    }
}
