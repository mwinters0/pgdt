use super::*;

#[test]
fn uuid_round_trips() {
    for text in ["00000000-0000-0000-0000-000000000000", "a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11"] {
        let bytes = decode_uuid(text).expect(text);
        assert_eq!(render_uuid(&bytes), text);
    }
    assert_eq!(decode_uuid("not-a-uuid"), None);
}
