//! Source identity while a run is in flight, and the strictness that decides
//! which weak signals bind (`docs/design/decisions.md`, "D20", "D21").
//!
//! **Nothing here races a rewrite against a scan.** A test that edited a file
//! while a scan read it would pin a schedule rather than a rule
//! (`docs/design/decisions.md`, "D73"), so the source a run is handed is what
//! moves: [`Shifting`] wraps a real one and answers a different identity from
//! a chosen observation onward, which is exactly what the library sees through
//! the trait. The one property that *is* about the filesystem — a file
//! replaced by rename leaving the open descriptor on the intact inode — is
//! tested against the filesystem, since nothing else can establish it.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use futures::StreamExt;
use pgdump_query::cache::{
    CacheMode, CacheStatus, OriginMatch, SourceWatch, StrictIdentity, WeakIdentity,
};
use pgdump_query::{
    ByteRangeSource, Cancellation, DEFAULT_MEMORY_BUDGET, DiagnosticKind, Error, LocalFileSource,
    Parallelism, Partitioning, QueryOptions, Result, ScanOptions, StatisticsRequest, cache,
    map_file, preamble_only, table_stream,
};

mod common;
use common::{edge_cases, sandboxed_edge_cases as sandboxed};

/// A source whose identity moves under the run reading it: the first
/// `observations` answers about `stored_size`/`modified` are the file's, and
/// every one after that reports the file a second later. Reads are the real
/// file's unless [`Shifting::dropping`] asked otherwise, which is the point —
/// the bytes a run has already returned are exactly what a changed identity
/// condemns.
struct Shifting {
    inner: LocalFileSource,
    observations: usize,
    seen: AtomicUsize,
    /// Answer `None` for the modification time rather than a moving one — the
    /// source that cannot give the guarantee `StrictIdentity::time` asks for.
    silent: bool,
    /// Where a read is dropped in flight instead of returning bytes, the way
    /// a source whose wait is a request answers a cancellation
    /// (`docs/design/decisions.md`, "D26"). `None` for a source that always
    /// delivers, which is every other test here.
    dropping: Option<(u64, Arc<Cancellation>)>,
}

impl Shifting {
    fn new(path: PathBuf, observations: usize) -> Self {
        Self {
            inner: LocalFileSource::open(path).unwrap(),
            observations,
            seen: AtomicUsize::new(0),
            silent: false,
            dropping: None,
        }
    }

    fn silent(path: PathBuf) -> Self {
        Self { silent: true, ..Self::new(path, usize::MAX) }
    }

    /// The same moving identity, over a source that answers a cancellation by
    /// abandoning every read at or past `trip` — so the run ends through
    /// `stream::cancelled_read` rather than through a polled check point.
    fn dropping(path: PathBuf, observations: usize, trip: u64, cancel: Arc<Cancellation>) -> Self {
        Self { dropping: Some((trip, cancel)), ..Self::new(path, observations) }
    }

    /// Whether this observation is past the point the identity moves. Counted
    /// on `modified`, which every identity observation makes exactly once.
    fn shifted(&self) -> bool {
        self.seen.load(Ordering::SeqCst) > self.observations
    }
}

impl ByteRangeSource for Shifting {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        if let Some((trip, cancel)) = &self.dropping
            && offset >= *trip
        {
            cancel.cancel();
            return Box::pin(std::future::ready(Err(Error::ScanCancelled {
                scanned_through: offset,
            })));
        }
        self.inner.read_range(offset, len)
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.inner.size()
    }

    /// The three the leader reads, forwarded so a scan through this wrapper
    /// is cut exactly as it would be through the file itself — the default
    /// `partitions` declines to advise, and a source that declines is never
    /// split, so a dropped read would never reach a worker.
    fn hint_read_size(&self, len: usize) {
        self.inner.hint_read_size(len);
    }

    fn partitions(&self, range: std::ops::Range<u64>) -> Partitioning {
        self.inner.partitions(range)
    }

    fn hint_parallelism(&self, parallelism: Parallelism) {
        self.inner.hint_parallelism(parallelism);
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move {
            if self.silent {
                return Ok(None);
            }
            self.seen.fetch_add(1, Ordering::SeqCst);
            let real = self.inner.modified().await?;
            Ok(real.map(|t| if self.shifted() { t + Duration::from_secs(1) } else { t }))
        })
    }
}

/// The three states the comparison has, at the level the library sees them.
#[tokio::test]
async fn a_watch_answers_the_identity_it_was_opened_on() {
    let source = LocalFileSource::open(edge_cases()).unwrap();
    let watch = SourceWatch::open(&source, StrictIdentity::ADVISORY).await.unwrap();
    watch.check(&source).await.expect("an unchanged source passes every check");

    // The first observation is the baseline, so the shift lands on the check.
    let shifting = Shifting::new(edge_cases(), 1);
    let watch = SourceWatch::open(&shifting, StrictIdentity::ADVISORY).await.unwrap();
    let err = watch.check(&shifting).await.expect_err("a moved identity is an error by default");
    let Error::SourceChangedWhileRead { differences } = &err else {
        panic!("the in-flight refusal, got {err:?}")
    };
    assert!(differences.contains("modification time"), "it says what moved: {differences}");
    let sentence = err.to_string();
    assert!(
        sentence.contains("while it was being read")
            && sentence.contains("nothing was saved")
            && sentence.contains("no cache was removed"),
        "the sentence says what happened and what it cost: {sentence}"
    );

    // `none` is the only opt-out, and it makes the same condition a warning.
    let shifting = Shifting::new(edge_cases(), 1);
    let watch = SourceWatch::open(&shifting, StrictIdentity::NONE).await.unwrap();
    watch.check(&shifting).await.expect("`none` turns the abort into a diagnostic");
}

/// **A run whose source moves saves nothing and removes nothing.** The save
/// that would have banked the map is where the change is noticed, so what is
/// already at the cache path is still there, byte for byte
/// (`docs/design/decisions.md`, "D20").
#[tokio::test]
async fn an_aborted_run_leaves_the_cache_on_disk_exactly_as_it_was() {
    let (dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);

    // A real, complete cache to be preserved, written from an unmoving source.
    let source = LocalFileSource::open(&dump).unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(!before.is_empty());

    // The load is one observation and the first save is the next, so a run
    // whose identity moves after the baseline aborts at that save.
    let shifting = Shifting::new(dump.clone(), 1);
    let err = map_file(
        &shifting,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .expect_err("a run whose source moved underneath it does not finish");
    assert!(matches!(err, Error::SourceChangedWhileRead { .. }), "got {err:?}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "the cache on disk is untouched");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        2,
        "the dump and its cache, and no half-written file beside them"
    );
}

/// **A run that never saves is still checked, once, when it finishes.** A
/// disabled cache reaches no save at all, and a warm query answers wholly out
/// of the cache — the two cases the save's own check cannot cover.
#[tokio::test]
async fn a_run_that_saves_nothing_is_checked_when_it_finishes() {
    let (_dir, dump) = sandboxed();

    // `--dtcache none`: the load is not an observation (nothing is read), so
    // the baseline is the only one before the end-of-run check.
    let shifting = Shifting::new(dump.clone(), 1);
    let err = map_file(
        &shifting,
        &ScanOptions::default(),
        &CacheMode::DISABLED,
        &StatisticsRequest::NONE,
    )
    .await
    .expect_err("a disabled-cache run still says the file moved");
    assert!(matches!(err, Error::SourceChangedWhileRead { .. }), "got {err:?}");

    // A warm query: the map is complete, so pass 1 extends nothing and pass 2
    // replays out of the cache. The rows come out and the failure lands after
    // them, which is the most that is recoverable by then.
    let path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .unwrap();

    let shifting = Shifting::new(dump.clone(), 1);
    let mut stream = table_stream(
        &shifting,
        "public.widgets",
        ScanOptions::default(),
        QueryOptions::default(),
        None,
        CacheMode::enabled(&path),
    );
    let mut rows = 0;
    let mut failure = None;
    while let Some(batch) = stream.next().await {
        match batch {
            Ok(batch) => rows += batch.num_rows(),
            Err(err) => failure = Some(err),
        }
    }
    assert!(rows > 0, "the rows are emitted before the run's last word");
    assert!(
        matches!(failure, Some(Error::SourceChangedWhileRead { .. })),
        "the warm query ends in the refusal, got {failure:?}"
    );

    // `preamble_only` over that same complete cache saves nothing either.
    let shifting = Shifting::new(dump.clone(), 1);
    let err = preamble_only(&shifting, &ScanOptions::default(), &CacheMode::enabled(&path))
        .await
        .expect_err("a preamble-only run over a complete cache is checked too");
    assert!(matches!(err, Error::SourceChangedWhileRead { .. }), "got {err:?}");
}

/// **A run ended by a dropped read is checked too.** The interrupt arriving
/// by another door is still an interrupt (`stream::cancelled_read`), and the
/// two arms that catch one leave `map_file` without passing the end-of-run
/// check the polled path takes — so each asks the source itself. Under
/// `--dtcache none` there is no save to inherit the question from, which is
/// the case this pins; with a cache enabled the save asks it anyway, except
/// in the prepass, which saves nothing at all
/// (`map_file::a_dropped_read_inside_the_prepass_banks_and_writes_nothing`).
///
/// Both arms, because only one of them is reachable at a time: a read the
/// mapping pass makes itself is caught at a check point of its own, so what
/// unwinds out of it is a read the **leader** dispatched. That wants a source
/// advising a partitioning, which is why the second half wraps one that
/// forwards `partitions`.
#[tokio::test]
async fn a_run_ended_by_a_dropped_read_is_checked_too() {
    let (_dir, dump) = sandboxed();

    // Every read drops, so the interrupt lands on the prepass's first one.
    let cancel = Arc::new(Cancellation::new());
    let shifting = Shifting::dropping(dump, 1, 0, Arc::clone(&cancel));
    let options = ScanOptions { cancel: Some(cancel), ..ScanOptions::default() };
    let err = map_file(&shifting, &options, &CacheMode::DISABLED, &StatisticsRequest::NONE)
        .await
        .expect_err("a dropped prepass read still says the file moved");
    assert!(matches!(err, Error::SourceChangedWhileRead { .. }), "got {err:?}");

    // And a read the leader dispatched, which unwinds out of the mapping pass
    // rather than being caught inside it. One `COPY` block, the trip on its
    // first data byte — which is the leader's own first read.
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("wide.sql");
    let mut file = b"COPY public.t (a) FROM stdin;\n".to_vec();
    let data_offset = file.len() as u64;
    for i in 0..2000 {
        file.extend_from_slice(format!("{i}\n").as_bytes());
    }
    file.extend_from_slice(b"\\.\n\nSELECT 1;\n");
    std::fs::write(&dump, &file).unwrap();
    let cancel = Arc::new(Cancellation::new());
    let shifting = Shifting::dropping(dump, 1, data_offset, Arc::clone(&cancel));
    let options = ScanOptions {
        chunk_size_bytes: 64,
        cancel: Some(cancel),
        parallelism: Parallelism::workers(4, DEFAULT_MEMORY_BUDGET),
        ..ScanOptions::default()
    };
    let err = map_file(&shifting, &options, &CacheMode::DISABLED, &StatisticsRequest::NONE)
        .await
        .expect_err("a dropped region read still says the file moved");
    assert!(matches!(err, Error::SourceChangedWhileRead { .. }), "got {err:?}");
}

/// **The check reads the descriptor, not the path**, which is what makes it
/// the right analogue of a remote precondition rather than merely the
/// available one: a dump replaced by rename leaves the open source reading the
/// intact inode it opened, and nothing about that run is wrong.
#[tokio::test]
async fn a_dump_replaced_by_rename_under_an_open_source_is_not_a_change() {
    let (dir, dump) = sandboxed();
    let source = LocalFileSource::open(&dump).unwrap();

    // A different file of a different length, renamed over the dump's name
    // while the source above holds the original open.
    let replacement = dir.path().join("replacement");
    std::fs::write(&replacement, b"-- an entirely different dump\n").unwrap();
    std::fs::rename(&replacement, &dump).unwrap();

    let run = map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(cache::colocated_path(&dump)),
        &StatisticsRequest::NONE,
    )
    .await
    .expect("the descriptor still reads the file this run opened");
    assert!(run.index.blocks().next().is_some(), "it read the dump, not the replacement");
}

/// Seconds and nanoseconds since the Unix epoch, as
/// `Error::StrictIdentityUnmet` spells a modification time — the one
/// rendering the library can give, nothing it links carrying a calendar.
fn epoch_stamp(t: SystemTime) -> String {
    let since = t.duration_since(std::time::UNIX_EPOCH).unwrap();
    format!("{}.{:09}", since.as_secs(), since.subsec_nanos())
}

/// `--strict-identity=time` promotes the advisory modification-time
/// diagnostic to a refusal, and **absence is a failure under it**: a source
/// that offers no modification time cannot give the guarantee that was asked
/// for, and silence is what strict identity exists to refuse. Either way the
/// clause names what it *saw* — the times compared, or which side is silent —
/// so the refusal can be checked against the file without a second run.
#[tokio::test]
async fn strict_time_refuses_a_moved_mtime_and_a_missing_one() {
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let recorded = std::fs::metadata(&dump).unwrap().modified().unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .unwrap();

    let future = SystemTime::now() + Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&dump).unwrap().set_modified(future).unwrap();
    let moved = std::fs::metadata(&dump).unwrap().modified().unwrap();

    // Strict first: a run that reads the cache saves it again, recording the
    // mtime it now sees, so the advisory case would otherwise settle the very
    // difference the strict one is about.
    let strict =
        CacheMode::enabled(&path).with_strict_identity(StrictIdentity::binding(true, false));
    let source = LocalFileSource::open(&dump).unwrap();
    let err = map_file(&source, &ScanOptions::default(), &strict, &StatisticsRequest::NONE)
        .await
        .expect_err("`time` binds the modification time");
    let Error::StrictIdentityUnmet { unmet, .. } = &err else {
        panic!("the strict refusal, got {err:?}")
    };
    assert!(unmet.contains("has moved since"), "it says which way it failed: {unmet}");
    assert!(
        unmet.contains(&epoch_stamp(recorded)) && unmet.contains(&epoch_stamp(moved)),
        "it names the time the cache recorded and the one the source now reports: {unmet}"
    );

    // The default reads it: the mtime is advisory and the map is reused.
    let source = LocalFileSource::open(&dump).unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .expect("an mtime is too weak to invalidate a cache by default");

    // A source with no modification time at all: the cache it wrote records
    // none, the live source offers none, and `time` refuses both silences.
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let silent = Shifting::silent(dump.clone());
    map_file(
        &silent,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .unwrap();
    let silent = Shifting::silent(dump);
    let err = map_file(
        &silent,
        &ScanOptions::default(),
        &CacheMode::enabled(&path).with_strict_identity(StrictIdentity::binding(true, false)),
        &StatisticsRequest::NONE,
    )
    .await
    .expect_err("absence is a failure under a selected term");
    let Error::StrictIdentityUnmet { unmet, .. } = &err else {
        panic!("the strict refusal, got {err:?}")
    };
    assert!(unmet.contains("modification time to compare"), "it names the silence: {unmet}");
    assert!(
        unmet.contains("neither it nor the source"),
        "with both sides silent it says so rather than blaming one: {unmet}"
    );

    // Silence on *one* side: a cache written from a source that had an mtime,
    // read back through one that offers none. The clause says which side went
    // quiet and names the time the other one has.
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    let recorded = std::fs::metadata(&dump).unwrap().modified().unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .unwrap();
    let silent = Shifting::silent(dump);
    let err = map_file(
        &silent,
        &ScanOptions::default(),
        &CacheMode::enabled(&path).with_strict_identity(StrictIdentity::binding(true, false)),
        &StatisticsRequest::NONE,
    )
    .await
    .expect_err("a source that has gone silent cannot give the guarantee either");
    let Error::StrictIdentityUnmet { unmet, .. } = &err else {
        panic!("the strict refusal, got {err:?}")
    };
    assert!(
        unmet.contains("the source carries no modification time")
            && unmet.contains(&epoch_stamp(recorded)),
        "it names the silent side and the time the other one kept: {unmet}"
    );
}

/// `location` binds where a source was fetched from, and a local cache
/// records no origin — so selecting it changes nothing here, which is a
/// property to pin rather than a gap to apologize for.
#[tokio::test]
async fn location_binds_nothing_on_a_local_source() {
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let source = LocalFileSource::open(&dump).unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .unwrap();

    let future = SystemTime::now() + Duration::from_secs(3600);
    std::fs::File::options().write(true).open(&dump).unwrap().set_modified(future).unwrap();
    let source = LocalFileSource::open(&dump).unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(&path).with_strict_identity(StrictIdentity::binding(false, true)),
        &StatisticsRequest::NONE,
    )
    .await
    .expect("`location` alone binds nothing a local source has");
}

// ---------------------------------------------------------------------------
// A source that was fetched from somewhere (`docs/design/decisions.md`, "D87"
// and "D21")
// ---------------------------------------------------------------------------

/// A source that reports having been fetched from an origin, with a stated
/// entity tag — every `ByteRangeSource` may, so the identity rules below are
/// pinned here with no server in the way. The bytes and the stored size are a
/// real local file's throughout, which is what keeps the *refusing* half of
/// identity out of these tests.
struct Fetched {
    inner: LocalFileSource,
    origin: String,
    etag: Option<String>,
    /// What `modified` answers, so a test can make the modification time
    /// agree while the tag does not.
    modified: Option<SystemTime>,
}

impl Fetched {
    fn new(path: &std::path::Path, origin: &str, etag: Option<&str>) -> Self {
        Self {
            inner: LocalFileSource::open(path).unwrap(),
            origin: origin.to_string(),
            etag: etag.map(ToOwned::to_owned),
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_784_764_800)),
        }
    }
}

impl ByteRangeSource for Fetched {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        self.inner.read_range(offset, len)
    }

    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        self.inner.size()
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move { Ok(self.modified) })
    }

    fn remote_identity(&self) -> Option<pgdump_query::RemoteIdentity> {
        Some(pgdump_query::RemoteIdentity::new(self.origin.clone(), self.etag.clone()))
    }
}

/// Map `dump` through `source`, saving to `path` under `strict`.
async fn mapped(
    source: &dyn ByteRangeSource,
    path: &std::path::Path,
    strict: StrictIdentity,
) -> Result<()> {
    map_file(
        source,
        &ScanOptions::default(),
        &CacheMode::enabled(path).with_strict_identity(strict),
        &StatisticsRequest::NONE,
    )
    .await
    .map(drop)
}

/// A cache written for one origin and read against another is **advisory**:
/// the map loads, and the difference is a warning
/// (`docs/design/decisions.md`, "D87"). The same stored size is
/// what makes the case reachable at all — two same-named dumps of equal size
/// from different hosts is exactly the collision the derived cache path
/// admits.
#[tokio::test]
async fn a_cache_written_for_another_origin_loads_with_a_warning() {
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let here = Fetched::new(&dump, "https://one.example/koji.dump", Some("\"abc\""));
    mapped(&here, &path, StrictIdentity::ADVISORY).await.unwrap();

    let elsewhere = Fetched::new(&dump, "https://two.example/koji.dump", Some("\"abc\""));
    let status = cache::load(&path, &elsewhere).await.unwrap();
    let CacheStatus::Valid { origin, weak, .. } = status else {
        panic!("an origin difference must not invalidate the cache, got {status:?}")
    };
    assert_eq!(weak, WeakIdentity::Agrees, "the entity tag is unchanged");
    let OriginMatch::Differs { cached, live } = origin else {
        panic!("the two origins differ, got {origin:?}")
    };
    assert_eq!(cached.as_deref(), Some("https://one.example/koji.dump"));
    assert_eq!(live.as_deref(), Some("https://two.example/koji.dump"));

    // And the advisory answer reaches a scan as a diagnostic rather than
    // stopping it.
    let run = map_file(
        &elsewhere,
        &ScanOptions::default(),
        &CacheMode::enabled(&path),
        &StatisticsRequest::NONE,
    )
    .await
    .expect("advisory by default");
    assert!(
        run.index.diagnostics.iter().any(|d| d.kind == DiagnosticKind::CacheOriginChanged),
        "{:?}",
        run.index.diagnostics
    );
}

/// `--strict-identity=location` promotes that diagnostic to a refusal, and the
/// refusal names both origins rather than only the verdict.
#[tokio::test]
async fn location_refuses_a_cache_written_for_another_origin() {
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let here = Fetched::new(&dump, "https://one.example/koji.dump", Some("\"abc\""));
    mapped(&here, &path, StrictIdentity::ADVISORY).await.unwrap();

    let elsewhere = Fetched::new(&dump, "https://two.example/koji.dump", Some("\"abc\""));
    let err = mapped(&elsewhere, &path, StrictIdentity::binding(false, true)).await.unwrap_err();
    let Error::StrictIdentityUnmet { term, unmet, .. } = &err else {
        panic!("`location` refuses a differing origin, got {err:?}")
    };
    assert_eq!(*term, "location");
    assert!(unmet.contains("one.example"), "{unmet}");
    assert!(unmet.contains("two.example"), "{unmet}");
}

/// A cache written for a source that was fetched from nowhere, read against
/// one that was, differs in origin — one side records none, which is a
/// statement rather than silence (D87).
#[tokio::test]
async fn a_local_cache_read_over_a_fetched_source_differs_in_origin() {
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let local = LocalFileSource::open(&dump).unwrap();
    mapped(&local, &path, StrictIdentity::ADVISORY).await.unwrap();

    let fetched = Fetched::new(&dump, "https://one.example/koji.dump", Some("\"abc\""));
    let status = cache::load(&path, &fetched).await.unwrap();
    let CacheStatus::Valid { origin, .. } = status else { panic!("still usable, got {status:?}") };
    assert_eq!(
        origin,
        OriginMatch::Differs {
            cached: None,
            live: Some("https://one.example/koji.dump".to_string())
        }
    );
}

/// The entity tag is the stronger of the two modification signals, so where
/// both sides carry one it settles the question — here saying the object
/// changed while the modification time says it did not
/// (`docs/design/decisions.md`, "D21").
#[tokio::test]
async fn an_entity_tag_outranks_a_modification_time_that_agrees_with_nothing() {
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let first = Fetched::new(&dump, "https://one.example/koji.dump", Some("\"abc\""));
    mapped(&first, &path, StrictIdentity::ADVISORY).await.unwrap();

    // Same origin, same `Last-Modified`, a tag the server has changed.
    let second = Fetched::new(&dump, "https://one.example/koji.dump", Some("\"def\""));
    let status = cache::load(&path, &second).await.unwrap();
    let CacheStatus::Valid { weak, origin, .. } = status else {
        panic!("a tag difference must not invalidate the cache, got {status:?}")
    };
    assert_eq!(origin, OriginMatch::Agrees);
    assert_eq!(
        weak,
        WeakIdentity::TagDiffers { cached: "\"abc\"".to_string(), live: "\"def\"".to_string() }
    );

    let err = mapped(&second, &path, StrictIdentity::binding(true, false)).await.unwrap_err();
    let Error::StrictIdentityUnmet { term, unmet, .. } = &err else {
        panic!("`time` binds the entity tag too, got {err:?}")
    };
    assert_eq!(*term, "time");
    assert!(unmet.contains("entity tag"), "{unmet}");
}

/// Where either side carries no tag, the modification time is what is left —
/// so a source whose server stopped sending tags is still compared rather
/// than being treated as silent.
#[tokio::test]
async fn a_missing_entity_tag_falls_back_to_the_modification_time() {
    let (_dir, dump) = sandboxed();
    let path = cache::colocated_path(&dump);
    let tagged = Fetched::new(&dump, "https://one.example/koji.dump", Some("\"abc\""));
    mapped(&tagged, &path, StrictIdentity::ADVISORY).await.unwrap();

    let mut untagged = Fetched::new(&dump, "https://one.example/koji.dump", None);
    let status = cache::load(&path, &untagged).await.unwrap();
    assert!(
        matches!(status, CacheStatus::Valid { weak: WeakIdentity::Agrees, .. }),
        "the two `Last-Modified`s agree, got {status:?}"
    );

    untagged.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_784_768_400));
    let status = cache::load(&path, &untagged).await.unwrap();
    assert!(
        matches!(status, CacheStatus::Valid { weak: WeakIdentity::Differs { .. }, .. }),
        "the times differ, got {status:?}"
    );
}
