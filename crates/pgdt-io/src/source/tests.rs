use crate::test_support::*;
use crate::*;

use super::*;

/// `LocalFileSource` never overrides [`ByteRangeSource::stored_size`], so
/// its default — mirroring [`ByteRangeSource::size`] — is what a
/// non-decompressing source is meant to answer.
#[tokio::test]
async fn stored_size_defaults_to_size() {
    let (_file, source) = source_of(b"0123456789abcdef");
    assert_eq!(source.stored_size().await.unwrap(), source.size().await.unwrap());
}

/// Every source answers its addressable length exactly; nothing
/// overrides the default.
#[test]
fn size_is_exact_defaults_true() {
    let (_file, source) = source_of(b"0123456789abcdef");
    assert!(source.size_is_exact());
}

/// A source that has not thought about being read concurrently advises
/// one partition, so a scheduler cannot split it on the assumption that
/// silence means consent.
#[test]
fn a_source_that_does_not_advise_gets_one_partition() {
    let advice = BareSource.partitions(0..1 << 30);
    assert_eq!(advice.max_partitions(), Some(1));
    assert_eq!(advice.boundaries(), &PartitionBoundaries::At(Vec::new()));
    assert_eq!(advice.partition_bytes(), 0);
}

/// **Silence recommends the serial path**, the same convention
/// `partitions` above answers with — and [`LocalFileSource`] inherits it
/// rather than overriding (`docs/design/decisions.md`, "D2").
#[test]
fn a_source_that_does_not_advise_recommends_the_serial_path() {
    assert_eq!(BareSource.default_workers(), 1);
    let (_file, source) = source_of(b"0123456789abcdef");
    assert_eq!(source.default_workers(), 1);
}
