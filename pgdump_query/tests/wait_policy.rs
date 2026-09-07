//! What each read loop permits the buffer pool to do to it
//! (`docs/design/architecture.md`, "Execution model and API surface").
//!
//! **The claim these tests exist for**: no read loop in the shipped build
//! grants [`WaitPolicy::MayWait`], so nothing here can block on a slot. The
//! wait is real, tested against a bare pool, and armed by the first holder
//! that needs it — `16.10.1`'s fused worker. Until then a loop that *could*
//! wait safely still does not, because the two failure directions are not
//! comparable: a bound that fails to bind costs memory and is visible, and a
//! wait granted wrongly is a hang with nothing to measure.
//!
//! What is asserted is the announcement, not the behaviour. Nothing in a
//! `read_range` call carries the policy, so the pool cannot see which loop
//! stated what — which is why it is recorded here, against the loops
//! themselves, rather than left to be re-derived from three call sites in two
//! modules.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::SystemTime;

use bytes::Bytes;
use futures::StreamExt;
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ByteRangeSource, LocalFileSource, QueryOptions, ScanOptions, WaitPolicy, build_index,
    table_stream,
};

mod common;
use common::edge_cases;

/// A `ByteRangeSource` wrapper that records every wait policy stated to it, in
/// order, and otherwise is the local file underneath.
struct RecordingSource {
    inner: LocalFileSource,
    stated: Mutex<Vec<WaitPolicy>>,
}

impl RecordingSource {
    fn wrap(inner: LocalFileSource) -> Self {
        Self { inner, stated: Mutex::new(Vec::new()) }
    }

    /// The announcements so far, with consecutive repeats collapsed: a loop
    /// states its policy once, but a query runs two loops and a resumed one
    /// may run the same loop twice, and what these tests are about is the
    /// *sequence* of policies rather than how many loops stated each.
    fn policies(&self) -> Vec<WaitPolicy> {
        let stated = self.stated.lock().unwrap();
        let mut out: Vec<WaitPolicy> = Vec::new();
        for policy in stated.iter() {
            if out.last() != Some(policy) {
                out.push(*policy);
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

    fn hint_wait_policy(&self, policy: WaitPolicy) {
        self.stated.lock().unwrap().push(policy);
        self.inner.hint_wait_policy(policy);
    }
}

/// A structural scan states the policy explicitly rather than falling through
/// to the default, so a source it shares with another loop is left saying what
/// this loop permits and not what the last one did.
#[tokio::test]
async fn a_structural_scan_grants_no_wait() {
    let source = RecordingSource::wrap(LocalFileSource::open(edge_cases()).unwrap());
    build_index(&source, &ScanOptions::default()).await.unwrap();
    assert_eq!(source.policies(), vec![WaitPolicy::NeverWait]);
}

/// A query is two loops over one source, and the second could not grant a wait
/// even if the build armed the bound: the replay pins every chunk a batch has
/// taken a view into. Both state the same thing today, which is the property
/// this records — `16.10.1` is what makes the sequence interesting.
#[tokio::test]
async fn a_query_grants_no_wait_in_either_loop() {
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
    assert_eq!(source.policies(), vec![WaitPolicy::NeverWait]);
}
