use super::*;

/// The engine a build links is the one its feature names, read back from the
/// engine itself: the backend figure's legs are told apart by this.
#[test]
fn the_engine_is_the_one_the_feature_names() {
    let engine = engine();
    let named = [("zlib-rs", cfg!(feature = "zlib-rs")), ("zlib-ng", cfg!(feature = "zlib-ng"))];
    for (name, on) in named {
        assert_eq!(engine.contains(name), on, "{name} in {engine:?}");
    }
    assert!(engine.starts_with("1."), "a zlib API version: {engine:?}");
}
