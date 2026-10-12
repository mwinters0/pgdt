//! A fixture copied out of the tree and mapped into a colocated cache, as a
//! parse leaves one, behind `mapping`: what every test reading a map starts
//! from, each stating the request it maps at.

use std::path::{Path, PathBuf};

use pgdt_dump::ScanOptions;
use pgdt_dump::cache::{self, CacheMode, CacheStatus};
use pgdt_io::LocalFileSource;
use pgdt_mapping::map_file;
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::{DataBlock, SpanBody};

/// `fixture` copied into a directory of its own under `dir`, beside the
/// complete cache a parse gathering `request` leaves, so the committed tree is
/// never written into. One directory per copy: every major has a
/// `default.sql`, and a cache left by another's copy describes another file.
pub async fn parsed_copy(fixture: &Path, dir: &Path, request: &StatisticsRequest) -> PathBuf {
    let dir = tempfile::tempdir_in(dir).unwrap().keep();
    let copy = dir.join(fixture.file_name().unwrap());
    std::fs::copy(fixture, &copy).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(cache::colocated_path(&copy));
    let run = map_file(&source, &ScanOptions::default(), &cache, request).await.unwrap();
    assert!(!run.interrupted);
    copy
}

/// The complete cache beside `dump` keeping no statistics: the census a typed
/// plan needs and nothing gathered, as a query's own mapping pass leaves a
/// cache (`docs/design/decisions.md`, "D35").
pub async fn ungathered(dump: &Path) {
    let source = LocalFileSource::open(dump).unwrap();
    let path = cache::colocated_path(dump);
    let CacheStatus::Valid { mut index, .. } = cache::load(&path, &source).await.unwrap() else {
        panic!("the parse left a complete cache")
    };
    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            block.statistics = None;
        }
    }
    cache::save(&path, &source, &index).await.unwrap();
}
