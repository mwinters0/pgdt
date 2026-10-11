use super::*;

/// The library's own default is the serial path, on both option structs
/// (`docs/design/decisions.md`, "D1").
#[test]
fn the_library_defaults_to_serial() {
    assert_eq!(
        pgdt_io::Parallelism::default(),
        pgdt_io::Parallelism::Serial { memory_bytes: None }
    );
    assert!(pgdt_dump::ScanOptions::default().parallelism.is_serial());
    assert!(QueryOptions::default().parallelism.is_serial());
    // And the default states no budget, which is not the same fact as
    // stating the number every pool falls back to.
    assert_eq!(pgdt_io::Parallelism::default().memory_bytes(), None);
}
