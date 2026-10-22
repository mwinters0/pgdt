//! A source whose extent is open until a read reaches its end
//! ([`pgdt_io::Extent::Open`]) — what a single gzip member is before its
//! first pass ends, built here over a plain file so every site reading an
//! extent meets one before any such source exists.
//!
//! **The claim**: an open extent changes how a pass learns where the file
//! ends, never what it maps. A pass over one writes the map a pass over the
//! plain file writes, at any worker count, the interior split never cutting
//! it; a pass stopped short saves a map ending at its watermark, an open
//! extent beside it, which a dump refuses as incomplete and a later pass
//! finishes into the same map.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::SystemTime;

use bytes::Bytes;
use pgdt_core::{Error, Result};
use pgdt_dump::cache::{self, CacheMode, CacheStatus, LoadedCache};
use pgdt_dump::{ReadOptions, ScanOptions, preamble_only};
use pgdt_fixtures::arranged;
use pgdt_io::{ByteRangeSource, Extent, LocalFileSource};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{DiagnosticKind, DumpIndex, PostgresInvalidValues, SpanBody};
use pgdt_query::Dump;
use tokio_util::sync::CancellationToken;

mod common;

/// A plain file that states no end until a read reaches it: `Open` at the
/// furthest byte read so far, `Exact` from the read that meets the end on.
struct OpenUntilEnd {
    inner: LocalFileSource,
    len: u64,
    seen: AtomicU64,
    reached: AtomicBool,
}

impl OpenUntilEnd {
    fn over(path: &Path) -> Arc<dyn ByteRangeSource> {
        Arc::new(Self {
            inner: LocalFileSource::open(path).unwrap(),
            len: std::fs::metadata(path).unwrap().len(),
            seen: AtomicU64::new(0),
            reached: AtomicBool::new(false),
        })
    }

    fn note(&self, through: u64) {
        self.seen.fetch_max(through, Ordering::SeqCst);
        if through >= self.len {
            self.reached.store(true, Ordering::SeqCst);
        }
    }
}

impl ByteRangeSource for OpenUntilEnd {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            let bytes = self.inner.read_range(offset, len).await?;
            self.note(offset + len as u64);
            Ok(bytes)
        })
    }

    /// Short only at the end, which it then states.
    fn read_at_most(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            let len = (len as u64).min(self.len.saturating_sub(offset)) as usize;
            if len == 0 {
                self.note(self.len);
                return Ok(Bytes::new());
            }
            let bytes = self.inner.read_range(offset, len).await?;
            self.note(offset + len as u64);
            Ok(bytes)
        })
    }

    fn extent(&self) -> Pin<Box<dyn Future<Output = Result<Extent>> + Send + '_>> {
        Box::pin(async move {
            Ok(if self.reached.load(Ordering::SeqCst) {
                Extent::Exact(self.len)
            } else {
                Extent::Open { at_least: self.seen.load(Ordering::SeqCst) }
            })
        })
    }

    fn stored_size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.inner.stored_size()
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }
}

/// `source` mapped whole at the data level under `jobs` and `read`, its cache
/// beside `dump`.
async fn mapped(
    dump: &Path,
    source: Arc<dyn ByteRangeSource>,
    jobs: usize,
    read: ReadOptions,
) -> DumpIndex {
    let mode = CacheMode::enabled(cache::colocated_path(dump));
    let run = map_file(
        mode.open_source(source).await.unwrap(),
        read,
        &arranged(jobs),
        PostgresInvalidValues::Default,
        &StatisticsRequest::DATA,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!run.interrupted);
    run.index
}

/// The cache beside `dump`, loaded against the plain file.
async fn loaded(dump: &Path) -> LoadedCache {
    let plain = LocalFileSource::open(dump).unwrap();
    match pgdt_dump::internal::load(&cache::colocated_path(dump), &plain).await.unwrap() {
        CacheStatus::Loaded(loaded) => loaded,
        other => panic!("a saved cache loads, got {other:?}"),
    }
}

/// **The map does not depend on when the end became known.** At one worker
/// and at four, which the interior split would cut a plain file at, and in a
/// chunk that ends nowhere in particular, a pass over the open source writes
/// the plain file's map, and saves the exact extent it reached.
#[tokio::test]
async fn a_pass_over_an_open_extent_maps_what_the_plain_file_maps() {
    for (schema, flag_set) in [("types", "default"), ("statistics", "default")] {
        let fixture = common::fixture(16, schema, flag_set);
        let (_dir, dump) = common::sandboxed(&fixture, "dump.sql");
        let len = std::fs::metadata(&dump).unwrap().len();
        let plain: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
        let expected = mapped(&dump, plain, 1, ReadOptions::default()).await;

        let odd = ReadOptions { chunk_size_bytes: 977, ..ReadOptions::default() };
        for (jobs, read) in [(1, ReadOptions::default()), (4, ReadOptions::default()), (4, odd)] {
            std::fs::remove_file(cache::colocated_path(&dump)).unwrap();
            let label = format!("{schema}/{flag_set} at {jobs} job(s), {read:?}");
            let index = mapped(&dump, OpenUntilEnd::over(&dump), jobs, read).await;
            assert_eq!(index, expected, "{label}");
            let saved = loaded(&dump).await;
            assert_eq!(saved.extent, Extent::Exact(len), "{label}");
            assert!(saved.is_complete(), "{label}");
            assert_eq!(saved.index, expected, "{label}");
        }
    }
}

/// **A first pass stopped short saves what it knows and no more**: its map
/// ends at its watermark with no span claiming the bytes after it, and its
/// extent is open — a cache no dump reads through, coverage stated without a
/// total — and a later pass finishes it into the map a single pass writes.
#[tokio::test]
async fn a_pass_stopped_short_saves_an_open_extent_and_resumes() {
    let fixture = common::fixture(16, "types", "default");
    let (_dir, dump) = common::sandboxed(&fixture, "dump.sql");
    let plain: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&dump).unwrap());
    let expected = mapped(&dump, plain, 1, ReadOptions::default()).await;
    std::fs::remove_file(cache::colocated_path(&dump)).unwrap();

    // The preamble alone, read in chunks far shorter than the file, so the
    // scan stops before any read meets the end.
    let small = ReadOptions { chunk_size_bytes: 512, ..ReadOptions::default() };
    let mode = CacheMode::enabled(cache::colocated_path(&dump));
    preamble_only(
        mode.open_source(OpenUntilEnd::over(&dump)).await.unwrap(),
        &ScanOptions { read: small, ..ScanOptions::default() },
    )
    .await
    .unwrap();

    let partial = loaded(&dump).await;
    let Extent::Open { at_least } = partial.extent else {
        panic!("a pass that never read the end records an open extent: {:?}", partial.extent);
    };
    let through = partial.index.scanned_through;
    assert!(through > 0 && at_least >= through, "{through} of at least {at_least}");
    assert!(!partial.is_complete());
    assert_eq!(partial.index.spans.last().map(|span| span.end), Some(through));
    assert!(!partial.index.spans.iter().any(|span| matches!(span.body, SpanBody::Unscanned)));
    assert!(
        !partial
            .index
            .diagnostics
            .iter()
            .any(|d| matches!(d.kind(), DiagnosticKind::TilingBroken { .. })),
        "a map ending at its watermark tiles what it covers: {:?}",
        partial.index.diagnostics
    );

    let Err(refused) =
        Dump::opened(mode.open_source(OpenUntilEnd::over(&dump)).await.unwrap()).await
    else {
        panic!("a map short of the end is refused");
    };
    assert!(
        matches!(refused, Error::MapIncomplete { scanned_through, size: None } if scanned_through == through),
        "{refused}"
    );
    assert!(refused.to_string().contains("a source whose end has not been read yet"), "{refused}");

    let index = mapped(&dump, OpenUntilEnd::over(&dump), 4, small).await;
    assert_eq!(index, expected);
    assert!(loaded(&dump).await.is_complete());
}
