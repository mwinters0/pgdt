use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use crate::test_support::*;

use super::*;
use crate::CompressionIndex;
use crate::source::compressed::CompressedFileSource;

/// The three answers an origin exists to give, from one probe of a local
/// file: the bytes as stored, the weak identity, and the leading magic.
#[tokio::test]
async fn an_origin_probe_answers_size_identity_and_the_leading_bytes() {
    let (file, _source) = source_of(b"\xfd7zXZ\x00 and then some more bytes");
    let origin = Origin::local(file.path());
    let probed = origin.probe().await.unwrap();
    assert_eq!(probed.stored_size(), std::fs::metadata(file.path()).unwrap().len());
    assert_eq!(
        probed.modified(),
        Some(std::fs::metadata(file.path()).unwrap().modified().unwrap())
    );
    assert_eq!(probed.leading(), b"\xfd7zXZ\x00");
}

/// A source shorter than the magic brings back what it has, which is not
/// an error and matches nothing.
#[tokio::test]
async fn an_origin_probe_of_a_short_source_brings_back_what_there_is() {
    let (file, _source) = source_of(b"ab");
    let origin = Origin::local(file.path());
    let probed = origin.probe().await.unwrap();
    assert_eq!(probed.leading(), b"ab");
    assert_eq!(probed.stored_size(), 2);
}

/// **The probe runs once.** Rewriting the file underneath a probed origin
/// changes nothing it answers — which is the property a source whose
/// probe is a round trip depends on, and the reason the claim and
/// recognition see one consistent set of answers.
#[tokio::test]
async fn an_origin_probes_at_most_once() {
    let (mut file, _source) = source_of(b"plain");
    let origin = Origin::local(file.path());
    let first = origin.probe().await.unwrap().clone();
    file.write_all(b" and rather more content than there was before").unwrap();
    file.flush().unwrap();
    assert_eq!(origin.probe().await.unwrap(), &first);
}

/// **A failed probe is not cached**, so the open that follows a claim
/// still reaches the failure — and an origin is not poisoned by having
/// been asked too early.
#[tokio::test]
async fn a_failed_origin_probe_is_not_remembered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-yet.sql");
    let origin = Origin::local(&path);
    assert!(origin.probe().await.is_err(), "nothing is there yet");
    std::fs::write(&path, b"now it is").unwrap();
    assert_eq!(origin.probe().await.unwrap().stored_size(), 9);
}

/// An origin names itself the way the message that refuses it does.
#[tokio::test]
async fn an_origin_displays_as_its_path() {
    let (file, _source) = source_of(b"x");
    assert_eq!(Origin::local(file.path()).to_string(), file.path().display().to_string());
}

/// Recognition with nothing claimed, unwrapped: `KnownCompression::Unknown`
/// claims nothing recognition could contradict.
async fn recognize(path: &Path) -> Arc<dyn ByteRangeSource> {
    match open_local(&Origin::local(path), KnownCompression::Unknown).await.unwrap() {
        Recognized::Source(source) => source,
        Recognized::Mismatch => panic!("`Unknown` claims nothing to contradict"),
    }
}

/// A genuinely `.xz`-compressed file is recognised whatever it is named
/// (`docs/design/decisions.md`, "D14"): the temp file `xz_compress`
/// returns carries no `.xz` suffix, and `open_local` still hands back a
/// source whose `compression_index()` answers `Some`, which of what
/// `open_local` returns only `CompressedFileSource` does.
#[tokio::test]
async fn open_local_recognizes_xz_content_with_no_xz_name() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    assert_ne!(compressed.path().extension(), Some(std::ffi::OsStr::new("xz")));

    let source = recognize(compressed.path()).await;
    assert!(source.compression_index().is_some(), "content-sniffed as .xz");
    assert_eq!(source.size().await.unwrap(), payload.len() as u64);
    let got = source.read_range(0, payload.len()).await.unwrap();
    assert_eq!(&got[..], &payload[..]);
}

/// The other direction: a file *named* `.xz` whose bytes are not, which
/// content sniffing opens plain (`docs/design/decisions.md`, "D14").
#[tokio::test]
async fn open_local_opens_a_dot_xz_named_file_with_plain_content_as_plain() {
    let mut file = tempfile::Builder::new().suffix(".xz").tempfile().unwrap();
    file.write_all(b"not actually compressed").unwrap();
    file.flush().unwrap();

    let source = recognize(file.path()).await;
    assert!(source.compression_index().is_none(), "no compression layer — read as plain");
    let got = source.read_range(0, 23).await.unwrap();
    assert_eq!(&got[..], b"not actually compressed");
}

/// A file too short to hold the six-byte magic must not be mistaken for
/// `.xz`, and must not error just for being short.
#[tokio::test]
async fn open_local_treats_a_file_shorter_than_the_magic_as_plain() {
    let (_file, plain) = source_of(b"ab");
    let path = plain.path().to_path_buf();
    let source = recognize(&path).await;
    assert!(source.compression_index().is_none());
    assert_eq!(source.size().await.unwrap(), 2);
}

/// A table a previous walk produced is handed back and used: the source
/// reads correctly and answers the same table it was given.
#[tokio::test]
async fn open_local_builds_an_xz_source_from_a_handed_back_table() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let table = seek_table_of(&CompressedFileSource::open(compressed.path()).unwrap());

    let source = match open_local(
        &Origin::local(compressed.path()),
        KnownCompression::Compressed(CompressionIndex::Xz(table.clone())),
    )
    .await
    .unwrap()
    {
        Recognized::Source(source) => source,
        Recognized::Mismatch => panic!("the file's own table must describe it"),
    };
    assert_eq!(seek_table_of(&*source), table);
    assert_eq!(source.size().await.unwrap(), payload.len() as u64);
    let got = source.read_range(9_000, 4000).await.unwrap();
    assert_eq!(&got[..], &payload[9_000..13_000]);
}

/// **The walk is actually skipped**, tested behaviourally rather than by
/// instrumentation (`docs/design/decisions.md`, "D18"): the table handed
/// in names a check algorithm this file's streams do not use, which
/// `validate` does not police. The check's size moves the payload's end,
/// so a decode from the handed table fails where a silent re-walk would
/// have read the payload back happily.
#[tokio::test]
async fn a_handed_back_table_is_used_rather_than_re_walked() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let mut table = seek_table_of(&CompressedFileSource::open(compressed.path()).unwrap());
    let real = table.0.streams[0].check;
    let wrong =
        if real == xz_seek::Check::Crc32 { xz_seek::Check::Crc64 } else { xz_seek::Check::Crc32 };
    for stream in &mut table.0.streams {
        stream.check = wrong;
    }

    let source = match open_local(
        &Origin::local(compressed.path()),
        KnownCompression::Compressed(CompressionIndex::Xz(table)),
    )
    .await
    .unwrap()
    {
        Recognized::Source(source) => source,
        Recognized::Mismatch => panic!("`validate` does not police the check algorithm"),
    };
    assert!(
        source.read_range(0, 4000).await.is_err(),
        "a decode under the wrong check must fail — a silent re-walk would have succeeded"
    );
}

/// A table that does not describe this file is refused, and the refusal
/// is an outcome rather than an error: nothing was read and the caller
/// still chooses whether to pay the walk.
#[tokio::test]
async fn open_local_reports_a_table_that_does_not_describe_the_file() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    // A second, shorter file's table: `validate` compares the table's
    // recorded compressed length against this source's.
    let other = xz_compress(&payload[..5_000], &["--block-size=4096"]);
    let other_table = seek_table_of(&CompressedFileSource::open(other.path()).unwrap());

    assert!(matches!(
        open_local(
            &Origin::local(compressed.path()),
            KnownCompression::Compressed(CompressionIndex::Xz(other_table))
        )
        .await
        .unwrap(),
        Recognized::Mismatch
    ));
}

/// Both directions of "the cache describes a different file": a
/// compression index for a file whose bytes are plain, and a plain cache
/// for a file whose bytes are `.xz`.
#[tokio::test]
async fn open_local_reports_a_compression_claim_the_file_contradicts() {
    let payload = xz_test_payload();
    let compressed = xz_compress(&payload, &["--block-size=4096"]);
    let table = seek_table_of(&CompressedFileSource::open(compressed.path()).unwrap());
    let (plain_file, _plain) = source_of(&payload);

    assert!(matches!(
        open_local(
            &Origin::local(plain_file.path()),
            KnownCompression::Compressed(CompressionIndex::Xz(table))
        )
        .await
        .unwrap(),
        Recognized::Mismatch
    ));
    assert!(matches!(
        open_local(&Origin::local(compressed.path()), KnownCompression::Plain).await.unwrap(),
        Recognized::Mismatch
    ));
}

/// A plain claim about a plain file is simply right, and costs nothing.
#[tokio::test]
async fn open_local_accepts_a_plain_claim_about_a_plain_file() {
    let (file, _plain) = source_of(b"0123456789abcdef");
    let source =
        match open_local(&Origin::local(file.path()), KnownCompression::Plain).await.unwrap() {
            Recognized::Source(source) => source,
            Recognized::Mismatch => panic!("a plain file is what the claim said"),
        };
    assert!(source.compression_index().is_none());
    assert_eq!(source.size().await.unwrap(), 16);
}

#[cfg(feature = "http")]
#[tokio::test]
async fn open_local_refuses_a_remote_origin_rather_than_asserting_it_is_local() {
    let origin = Origin::resolve("http://example.com/koji.dump").unwrap();
    let Err(err) = open_local(&origin, KnownCompression::Unknown).await else {
        panic!("`open_local` answered for a URL");
    };
    let err = err.to_string();
    assert!(err.contains("not a file on this filesystem"), "{err}");
}
