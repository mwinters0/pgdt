//! What each read loop permits the buffer pool to do to it
//! (`docs/design/architecture.md`, "Execution model and API surface").
//!
//! **The claim these tests exist for**: none of the three top-level read loops
//! grants [`WaitPolicy::MayWait`], so nothing a `build_index` or a
//! `table_stream` does can block on a slot. The one loop that grants it is the
//! leader's fused worker, which runs *inside* the mapping pass and restores
//! `NeverWait` on its way out — so the only invocation that reaches it is a
//! [`Parallelism::Workers`] scan over a region the source is willing to cut, and
//! the announcement sequence is what says so from outside. A loop that *could*
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
    ByteRangeSource, DEFAULT_MEMORY_BUDGET, LocalFileSource, Parallelism, QueryOptions,
    ScanOptions, WaitPolicy, build_index, map_file, table_stream,
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

    /// Forwarded, and it has to be: the default declines to advise, and a
    /// source that declines is never cut — so a wrapper that stopped here would
    /// make the leader decline every region and quietly turn the test below
    /// into the serial path.
    fn partitions(&self, range: std::ops::Range<u64>) -> pgdump_query::Partitioning {
        self.inner.partitions(range)
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
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

/// **A `parse` that states workers reaches the leader, and the sequence is how
/// that is visible from outside.** `map_forward` states `NeverWait` once and
/// then offers each open `COPY` region to `leader::scan_region`, which grants
/// `MayWait` for the window it schedules and restores `NeverWait` before
/// returning. So a scan that took `n` regions announces `NeverWait` and then
/// `n` grant-and-restore pairs — the restoration being what the last entry
/// checks, since a scheduler that forgot it would leave the enclosing loop
/// reading on under a permission it never granted — and a scan that declined
/// every region announces `[NeverWait]` alone.
///
/// It is also this file's answer to a trap `tests/map_file.rs` cannot spring on
/// its own: an index equality under `--jobs 8` proves nothing if the scheduler
/// declined every block, since that is the serial path compared to itself. The
/// small chunk is what makes a fixture's regions several partitions each — the
/// local source's partition is a fixed multiple of its read chunk, so at the
/// shipped 1 MiB every fixture region is inside one partition and every one of
/// them is declined.
#[tokio::test]
async fn a_parallel_scan_grants_the_wait_inside_the_mapping_pass_and_takes_it_back() {
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("dump.sql");
    std::fs::copy(edge_cases(), &dump).unwrap();
    let source = RecordingSource::wrap(LocalFileSource::open(&dump).unwrap());
    let options = ScanOptions {
        chunk_size: 64,
        parallelism: Parallelism::workers(8, DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    };
    map_file(&source, &options, &CacheMode::Disabled).await.unwrap();
    // `policies()` collapses consecutive repeats, so the sequence alternates by
    // construction and what is left to check is its ends and its length.
    let policies = source.policies();
    assert!(policies.len() >= 3 && policies.len() % 2 == 1, "{policies:?}");
    assert_eq!(policies.first(), Some(&WaitPolicy::NeverWait), "the mapping pass states its own");
    assert!(policies.contains(&WaitPolicy::MayWait), "a region was actually scheduled");
    assert_eq!(policies.last(), Some(&WaitPolicy::NeverWait), "the scheduler restored it");

    // The same scan at the shipped chunk size declines every one of this
    // fixture's blocks, so the leader is reached and grants nothing.
    let source = RecordingSource::wrap(LocalFileSource::open(&dump).unwrap());
    let options = ScanOptions {
        parallelism: Parallelism::workers(8, DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    };
    map_file(&source, &options, &CacheMode::Disabled).await.unwrap();
    assert_eq!(source.policies(), vec![WaitPolicy::NeverWait], "every region was declined");
}

/// A query is two loops over one source, and the second could not grant a wait
/// even if the build armed the bound: the replay pins every chunk a batch has
/// taken a view into. Both state the same thing under the library's serial
/// default, which is the property this records; a stated `Parallelism` is what
/// makes the sequence interesting, and that is the test above.
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
