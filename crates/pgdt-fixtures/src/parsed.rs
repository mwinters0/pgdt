//! A fixture copied out of the tree and mapped into a colocated cache, as a
//! parse leaves one, behind `mapping`: what every test reading a map starts
//! from, each stating the request it maps at; and the arrangement a test's
//! mapping pass reads under.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pgdt_dump::ReadOptions;
use pgdt_dump::cache::{self, CacheLoad, CacheMode, SourceWatch, StrictIdentity};
use pgdt_io::{Allowance, Arrangement, ArrangementRequest, ByteRangeSource, Jobs, LocalFileSource};
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{DataBlock, PostgresInvalidValues, SpanBody};
use tokio_util::sync::CancellationToken;

/// `jobs` workers stated under no allowance: what a test's mapping pass
/// (`pgdt_mapping::map_file`) reads under where it states no allowance of
/// its own — the serial path stating no budget at one, the library's default
/// budget above it, and a statistics allowance declining nothing
/// (`docs/design/decisions.md`, "D1"). The arrangement's only constructor is
/// its resolution, so this is one, of `Allowance::NoneFound`.
pub fn arranged(jobs: usize) -> Arrangement {
    let request = ArrangementRequest {
        jobs: Jobs::Stated(jobs),
        ..ArrangementRequest::recommended(jobs, None)
    };
    Arrangement::resolve(&Allowance::NoneFound, request)
}

/// `fixture` copied into a directory of its own under `dir`, beside the
/// complete cache a parse gathering `request` leaves, so the committed tree is
/// never written into. One directory per copy: every major has a
/// `default.sql`, and a cache left by another's copy describes another file.
pub async fn parsed_copy(fixture: &Path, dir: &Path, request: &StatisticsRequest) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source: Arc<dyn ByteRangeSource> = Arc::new(LocalFileSource::open(&copy).unwrap());
    let cache = CacheMode::enabled(cache::colocated_path(&copy));
    let run = map_file(
        cache.open_source(Arc::clone(&source)).await.unwrap(),
        ReadOptions::default(),
        &arranged(1),
        PostgresInvalidValues::Default,
        request,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!run.interrupted);
    copy
}

/// The complete cache beside `dump` keeping no statistics: the census a typed
/// plan needs and nothing gathered, as a query's own mapping pass leaves a
/// cache (`docs/design/decisions.md`, "D35").
pub async fn ungathered(dump: &Path) {
    let source = LocalFileSource::open(dump).unwrap();
    let mode = CacheMode::enabled(cache::colocated_path(dump));
    let CacheLoad::Index(mut index) = mode.load(&source).await.unwrap() else {
        panic!("the parse left a cache")
    };
    assert!(index.is_complete(source.size().await.unwrap()), "the parse left a complete cache");
    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            block.statistics = None;
        }
    }
    let watch = SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap();
    mode.save(&watch, &source, &index).await.unwrap();
}
