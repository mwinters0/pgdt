//! What each read loop announces about how long it holds its reads
//! (`docs/design/architecture.md`, "Execution model and API surface").
//!
//! **The claim these tests exist for**: the buffer pool's wait is safe only
//! because the loops that announce [`HolderClass::Transient`] hold one buffer
//! at a time, and the one that pins chunks into batches announces
//! [`HolderClass::Retaining`] instead. That mapping is invisible in the pool —
//! nothing in a `read_range` call carries it — so it is asserted here, against
//! the loops themselves, rather than left to be re-derived from three call
//! sites in two modules.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::SystemTime;

use bytes::Bytes;
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ByteRangeSource, HolderClass, LocalFileSource, QueryOptions, ScanOptions, build_index,
    table_stream,
};

mod common;
use common::edge_cases;

/// A `ByteRangeSource` wrapper that records every holder class announced to
/// it, in order, and otherwise is the local file underneath.
struct RecordingSource {
    inner: LocalFileSource,
    announced: Mutex<Vec<HolderClass>>,
}

impl RecordingSource {
    fn wrap(inner: LocalFileSource) -> Self {
        Self { inner, announced: Mutex::new(Vec::new()) }
    }

    /// The announcements so far, with consecutive repeats collapsed: a loop
    /// states its class once, but a query runs two loops and a resumed one may
    /// run the same loop twice, and what these tests are about is the
    /// *sequence* of classes rather than how many loops stated each.
    fn classes(&self) -> Vec<HolderClass> {
        let announced = self.announced.lock().unwrap();
        let mut out: Vec<HolderClass> = Vec::new();
        for class in announced.iter() {
            if out.last() != Some(class) {
                out.push(*class);
            }
        }
        out
    }
}

impl ByteRangeSource for RecordingSource {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = pgdump_query::Result<Bytes>> + Send + '_>> {
        self.inner.read_range(offset, len)
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = pgdump_query::Result<u64>> + Send + '_>> {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> Pin<Box<dyn Future<Output = pgdump_query::Result<Option<SystemTime>>> + Send + '_>> {
        self.inner.modified()
    }

    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }

    fn hint_holder_class(&self, class: HolderClass) {
        self.announced.lock().unwrap().push(class);
        self.inner.hint_holder_class(class);
    }
}

/// A structural scan consumes each chunk before it reads the next — the carry
/// copies what it keeps and an `Event` borrows only for the callback — so it
/// announces the class that may wait for a pooled slot.
#[tokio::test]
async fn a_structural_scan_is_transient() {
    let source = RecordingSource::wrap(LocalFileSource::open(edge_cases()).unwrap());
    build_index(&source, &ScanOptions::default()).await.unwrap();
    assert_eq!(source.classes(), vec![HolderClass::Transient]);
}

/// A query is two loops and they are not the same class: the mapping pass
/// builds spans and may wait, the replay pins every chunk a batch has taken a
/// view into and must not.
#[tokio::test]
async fn a_query_maps_transient_and_replays_retaining() {
    let source = RecordingSource::wrap(LocalFileSource::open(edge_cases()).unwrap());
    let mut stream = table_stream(
        &source,
        "public.no_column_list",
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        CacheMode::Disabled,
    );
    let mut rows = 0usize;
    while let Some(batch) = stream.next().await {
        rows += batch.unwrap().num_rows();
    }
    assert!(rows > 0, "the fixture's `public.no_column_list` has rows");
    assert_eq!(source.classes(), vec![HolderClass::Transient, HolderClass::Retaining]);
}
