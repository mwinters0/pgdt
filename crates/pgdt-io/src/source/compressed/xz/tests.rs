use crate::test_support::*;

use super::*;

/// **The seek table is held once, at a stated cost per entry** — the check
/// for what is left of `KD26`.
///
/// Two properties, neither checked anywhere else. The **share**:
/// [`XzCodec::over`] takes the layout's own `Arc` through
/// `xz_seek::Layout::index_shared`, so the codec's table and the layout's
/// are one allocation, and the walking constructor goes through `over`. The
/// **per-entry cost** is a property of a vendored struct, so a re-sync
/// that grew either entry fails here rather than silently moving a
/// published number.
#[test]
fn the_seek_table_is_held_once_at_a_stated_cost_per_entry() {
    assert_eq!(std::mem::size_of::<xz_seek::StreamEntry>(), 80);
    assert_eq!(std::mem::size_of::<xz_seek::BlockEntry>(), 32);

    let compressed = xz_compress(&xz_test_payload(), &["--block-size=4096"]);
    let file = std::fs::File::open(compressed.path()).unwrap();
    let stored_size = file.metadata().unwrap().len();

    // The walking constructor: the walk built the table and the layout
    // hands its own handle out.
    let walked = XzCodec::walk(compressed.path(), &file, stored_size).unwrap();
    assert!(
        std::ptr::eq(&*walked.table, walked.layout.index()),
        "the codec's table must be the layout's own, not a clone of it",
    );

    // The cached one: a table handed in is the layout's table too, so the
    // caller's copy is dropped rather than becoming a third.
    let CompressionIndex::Xz(persisted) = walked.index() else { panic!("an `.xz` codec's index") };
    let cached = XzCodec::over(persisted, stored_size).unwrap();
    assert!(
        std::ptr::eq(&*cached.table, cached.layout.index()),
        "a cached table must be shared with the layout it was handed to",
    );
}

/// **The codec's units are the file's blocks**, as the arms read them: in
/// file order, tiling the plaintext, each compressed extent reaching its
/// block's check, and the widest of them what a fetch of one makes.
#[test]
fn the_codecs_units_are_the_files_blocks() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let file = std::fs::File::open(compressed.path()).unwrap();
    let stored_size = file.metadata().unwrap().len();
    let codec = XzCodec::walk(compressed.path(), &file, stored_size).unwrap();

    assert_eq!(codec.name(), "xz");
    assert_eq!(codec.plaintext_size(), payload.len() as u64);
    assert_eq!(codec.compressed_size(), stored_size);
    assert_eq!(codec.unit_count(), codec.table.block_count());
    assert!(codec.unit_count() > 1, "{} blocks", codec.unit_count());
    let units: Vec<_> = (0..codec.unit_count()).map(|i| codec.plaintext(i).unwrap()).collect();
    assert_eq!(units.first().map(|u| u.start), Some(0));
    assert!(units.windows(2).all(|w| w[0].end == w[1].start), "{units:?}");
    assert_eq!(units.last().map(|u| u.end), Some(codec.plaintext_size()));
    assert_eq!(codec.plaintext(codec.unit_count()), None);
    assert_eq!(codec.units_in(4096..4097), 1..2);
    let widest = (0..codec.unit_count())
        .map(|i| codec.compressed(i).map(|c| c.end - c.start).unwrap())
        .max();
    assert_eq!(widest, Some(codec.widest_compressed()));
    assert!(codec.pulling_footprint() > codec.lending_footprint(), "the input chunk");

    // A unit decoded whole is its plaintext, through the bridge.
    let mut out = vec![0u8; 4096];
    codec.decode(1, &file, &mut out).unwrap();
    assert_eq!(&out[..], &payload[4096..8192]);
}

/// **A table that does not describe the file is refused as such**, the error
/// [`refuses_table`] names and recognition turns into a mismatch rather than
/// a walk; any other codec error is not that.
#[test]
fn a_table_for_another_file_is_refused_as_not_describing_it() {
    let one = xz_compress(&xz_test_payload(), &["--block-size=4096"]);
    let file = std::fs::File::open(one.path()).unwrap();
    let stored_size = file.metadata().unwrap().len();
    let CompressionIndex::Xz(table) =
        XzCodec::walk(one.path(), &file, stored_size).unwrap().index()
    else {
        panic!("an `.xz` codec's index");
    };

    let error = XzCodec::over(table, stored_size + 1).err().expect("a longer file is refused");
    assert!(refuses_table(&error), "{error}");
    assert!(error.to_string().starts_with("xz error: "), "{error}");
    assert!(!refuses_table(&Error::codec(XZ, xz_seek::Error::Truncated { compressed_offset: 0 })));
}
