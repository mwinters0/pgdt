use crate::test_support::*;
use crate::*;

use super::*;

/// `LocalFileSource` never overrides [`ByteRangeSource::stored_size`], so
/// its default — the exact extent — is what a non-decompressing source is
/// meant to answer.
#[tokio::test]
async fn stored_size_defaults_to_the_exact_extent() {
    let (_file, source) = source_of(b"0123456789abcdef");
    assert_eq!(source.extent().await.unwrap(), Extent::Exact(16));
    assert_eq!(source.stored_size().await.unwrap(), 16);
}

/// The default short read answers within the exact extent: whole inside
/// it, short across its end, empty at or past it — never an error, which is
/// what `read_range` answers past the end.
#[tokio::test]
async fn read_at_most_defaults_to_a_read_within_the_exact_extent() {
    let (_file, source) = source_of(b"0123456789abcdef");
    assert_eq!(source.read_at_most(2, 4).await.unwrap().as_ref(), b"2345");
    assert_eq!(source.read_at_most(12, 8).await.unwrap().as_ref(), b"cdef");
    assert!(source.read_at_most(16, 8).await.unwrap().is_empty());
    assert!(source.read_at_most(40, 8).await.unwrap().is_empty());
    assert!(source.read_range(12, 8).await.is_err(), "the strict read stays strict");
}

/// A source answering an open extent that leaves the two defaults reading
/// it alone has broken their contract, and each refuses rather than
/// guessing an end.
#[tokio::test]
async fn the_defaults_refuse_an_open_extent() {
    struct Open;
    impl ByteRangeSource for Open {
        fn read_range(
            &self,
            _offset: u64,
            _len: usize,
        ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
            Box::pin(async { Ok(Bytes::new()) })
        }
        fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
            Box::pin(async { Ok(Extent::Open { at_least: 8 }) })
        }
        fn modified(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
            Box::pin(async { Ok(None) })
        }
    }
    assert!(Open.stored_size().await.is_err());
    assert!(Open.read_at_most(0, 4).await.is_err());
}

/// What each extent says of a scan's progress: an exact one is reached at
/// its length, an open one never, whatever has been read.
#[test]
fn an_open_extent_is_never_reached() {
    assert!(Extent::Exact(10).reached_by(10));
    assert!(!Extent::Exact(10).reached_by(9));
    assert!(!Extent::Open { at_least: 10 }.reached_by(u64::MAX));
    assert_eq!(Extent::Open { at_least: 10 }.known(), 10);
    assert_eq!(Extent::Open { at_least: 10 }.exact(), None);
    assert_eq!(Extent::Exact(10).to_string(), "10");
    assert_eq!(Extent::Open { at_least: 10 }.to_string(), "at least 10");
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
