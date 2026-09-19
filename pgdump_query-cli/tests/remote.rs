//! The remote source, read against the oracle
//! (`docs/design/roadmap-P14-remote-input.md`, "D3", "D6"–"D9", "D14", "D15",
//! "D17").
//!
//! **Here rather than in the library's own tests** because the oracle is here:
//! a `tests/*.rs` file is its own crate and this is the crate that enables the
//! `http` feature, so `cargo test --workspace` runs every assertion below
//! unconditionally.
//!
//! Two kinds of subject, deliberately mixed in one file because they share the
//! instrument: `pgdq` over a URL, which is what a user does, and a
//! `ByteRangeSource` over one, which is where a server's misbehaviour is
//! visible at all — a scan reports the first fault it meets and says nothing
//! about the request that produced it.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::oracle::Oracle;
use common::{fixture, run, run_ok, stderr_of};
use pgdump_query::cache::CacheMode;
use pgdump_query::{
    ByteRangeSource, Cancellation, KnownCompression, Origin, Parallelism, Recognized, ScanOptions,
    StatisticsRequest, open, open_local,
};

/// The dump every test here serves: real `pg_dump` output, so a scan over it
/// finds the same tables a local read finds.
fn dump_bytes() -> Vec<u8> {
    std::fs::read(fixture("16/edge_cases/default.sql")).unwrap()
}

/// An oracle serving that dump, with no knob set — the correct origin every
/// misbehaving case is read against.
fn serving_dump() -> Oracle {
    Oracle::serving(dump_bytes()).start()
}

/// The source `url` names, opened as a run opens it: probe, then recognition.
async fn source_of(url: &str) -> Arc<dyn ByteRangeSource> {
    let origin = Origin::resolve(url).expect("the URL resolves");
    match open(&origin, KnownCompression::Unknown).await.expect("the origin opens") {
        Recognized::Source(source) => source,
        Recognized::Mismatch => panic!("nothing was claimed, so nothing can be contradicted"),
    }
}

// ---------------------------------------------------------------------------
// What a `--source` argument names (D3, D17)
// ---------------------------------------------------------------------------

#[test]
fn a_bare_path_is_a_path() {
    let origin = Origin::resolve("fixtures/16/edge_cases/default.sql").unwrap();
    assert_eq!(
        origin.local_path().unwrap().to_str().unwrap(),
        "fixtures/16/edge_cases/default.sql"
    );
}

#[test]
fn an_absolute_path_is_a_path() {
    let origin = Origin::resolve("/var/tmp/koji.dump").unwrap();
    assert_eq!(origin.local_path().unwrap().to_str().unwrap(), "/var/tmp/koji.dump");
}

#[test]
fn a_file_url_is_the_path_it_names_percent_decoded() {
    let origin = Origin::resolve("file:///var/tmp/two%20words.dump").unwrap();
    assert_eq!(origin.local_path().unwrap().to_str().unwrap(), "/var/tmp/two words.dump");
}

#[test]
fn a_file_url_on_localhost_is_this_machine() {
    let origin = Origin::resolve("file://localhost/var/tmp/koji.dump").unwrap();
    assert_eq!(origin.local_path().unwrap().to_str().unwrap(), "/var/tmp/koji.dump");
}

#[test]
fn a_file_url_naming_another_host_is_refused_by_name() {
    let err = Origin::resolve("file://elsewhere/var/tmp/koji.dump").unwrap_err().to_string();
    assert!(err.contains("elsewhere"), "{err}");
    assert!(err.contains("`localhost`"), "{err}");
}

#[test]
fn an_http_url_is_remote() {
    let origin = Origin::resolve("http://example.com/koji.dump").unwrap();
    assert!(origin.local_path().is_none(), "a URL has no path beside it");
    assert_eq!(origin.to_string(), "http://example.com/koji.dump");
}

#[test]
fn another_scheme_is_refused_by_name_and_says_how_to_write_a_path() {
    let err = Origin::resolve("s3://bucket/koji.dump").unwrap_err().to_string();
    assert!(err.contains("`s3:`"), "{err}");
    assert!(err.contains("./s3://bucket/koji.dump"), "{err}");
}

#[test]
fn a_url_carrying_a_credential_is_refused_rather_than_stripped() {
    let err = Origin::resolve("https://user:secret@example.com/koji.dump").unwrap_err().to_string();
    assert!(err.contains("username or password"), "{err}");
    assert!(err.contains("presigned"), "{err}");
    // The refusal names the URL, and the one thing it must not repeat back is
    // the credential it has just said it will not send.
    assert!(err.contains("https://example.com/koji.dump"), "{err}");
    assert!(!err.contains("secret"), "{err}");
    assert!(!err.contains("user:"), "{err}");
}

#[tokio::test]
async fn open_local_refuses_a_remote_origin_rather_than_asserting_it_is_local() {
    let origin = Origin::resolve("http://example.com/koji.dump").unwrap();
    let Err(err) = open_local(&origin, KnownCompression::Unknown).await else {
        panic!("`open_local` answered for a URL");
    };
    let err = err.to_string();
    assert!(err.contains("not a file on this filesystem"), "{err}");
}

// ---------------------------------------------------------------------------
// The probe (D2) and what the source answers (D7)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_origin_probe_costs_one_round_trip() {
    let oracle = serving_dump();
    let origin = Origin::resolve(&oracle.url()).unwrap();
    let probe = origin.probe().await.unwrap();

    assert_eq!(probe.stored_size(), oracle.body().len() as u64);
    assert!(probe.modified().is_some(), "the oracle sends a Last-Modified");
    assert_eq!(probe.leading(), &dump_bytes()[..probe.leading().len()]);
    // One request, and a ranged GET rather than a HEAD: the 206 carries the
    // object's total size in `Content-Range`, so the size, the validators and
    // the magic bytes all arrive together.
    let requests = oracle.requests();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0].method, "GET");
    assert!(requests[0].range().is_some(), "{requests:?}");
}

#[tokio::test]
async fn a_second_probe_asks_nothing_further() {
    let oracle = serving_dump();
    let origin = Origin::resolve(&oracle.url()).unwrap();
    origin.probe().await.unwrap();
    origin.probe().await.unwrap();
    assert_eq!(oracle.request_count(), 1);
}

#[tokio::test]
async fn a_server_sending_no_modification_time_reads_as_silence() {
    // The backend cannot say "absent" for a `Last-Modified`: it substitutes
    // the Unix epoch (`docs/design/runtime-invariants.md`, "RT16"). No dump
    // was modified in 1970, so the epoch is read back as the silence it is.
    let oracle = Oracle::serving(dump_bytes()).without_last_modified().start();
    let origin = Origin::resolve(&oracle.url()).unwrap();
    assert_eq!(origin.probe().await.unwrap().modified(), None);

    let source = source_of(&oracle.url()).await;
    assert_eq!(source.modified().await.unwrap(), None);
}

#[tokio::test]
async fn the_source_answers_its_size_and_identity_without_asking_again() {
    let oracle = serving_dump();
    let source = source_of(&oracle.url()).await;
    let after_open = oracle.request_count();

    assert_eq!(source.size().await.unwrap(), oracle.body().len() as u64);
    assert_eq!(source.stored_size().await.unwrap(), oracle.body().len() as u64);
    assert!(source.modified().await.unwrap().is_some());
    assert!(source.size_is_exact());
    assert_eq!(oracle.request_count(), after_open, "the probe already answered all three");
}

#[tokio::test]
async fn the_source_declines_every_advisory_answer_it_has_no_reading_for() {
    let oracle = serving_dump();
    let source = source_of(&oracle.url()).await;
    let size = source.size().await.unwrap();

    assert_eq!(source.default_workers(), 1, "no block structure to cut at");
    assert!(source.default_worker_memory().is_none(), "no per-worker recommendation");
    assert!(source.block_decode_bytes().is_none(), "there is no block path to price");
    assert_eq!(source.partitions(0..size).max_partitions(), Some(1), "one partition");
    assert!(source.seek_table().is_none());
}

#[tokio::test]
async fn a_ranged_read_answers_exactly_what_was_asked_for() {
    let oracle = serving_dump();
    let source = source_of(&oracle.url()).await;
    let body = dump_bytes();

    let head = source.read_range(0, 32).await.unwrap();
    assert_eq!(&head[..], &body[..32]);
    let middle = source.read_range(100, 64).await.unwrap();
    assert_eq!(&middle[..], &body[100..164]);
    let tail = source.read_range(body.len() as u64 - 8, 8).await.unwrap();
    assert_eq!(&tail[..], &body[body.len() - 8..]);
}

#[tokio::test]
async fn a_read_past_the_end_is_an_unexpected_eof_naming_the_url() {
    let oracle = serving_dump();
    let source = source_of(&oracle.url()).await;
    let size = source.size().await.unwrap();
    let err = source.read_range(size - 4, 64).await.unwrap_err().to_string();
    assert!(err.contains("byte(s) of the 64 asked for"), "{err}");
    assert!(err.contains(&oracle.url()), "{err}");
}

#[tokio::test]
async fn the_query_string_of_a_presigned_url_survives_into_every_request() {
    let oracle = serving_dump();
    let url = format!("{}?X-Amz-Signature=deadbeef", oracle.url());
    let source = source_of(&url).await;
    source.read_range(0, 16).await.unwrap();

    let requests = oracle.requests();
    assert!(requests.len() >= 2, "{requests:?}");
    for request in &requests {
        assert_eq!(request.path, "/dump.sql?X-Amz-Signature=deadbeef", "{request:?}");
    }
}

// ---------------------------------------------------------------------------
// What a server's misbehaviour costs (D6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_range_ignoring_server_is_refused_rather_than_read_whole() {
    let oracle = Oracle::serving(dump_bytes()).ignoring_range().start();
    let origin = Origin::resolve(&oracle.url()).unwrap();
    let err = origin.probe().await.unwrap_err().to_string();
    assert!(err.contains(&oracle.url()), "{err}");
    assert!(err.to_lowercase().contains("range"), "{err}");
}

#[tokio::test]
async fn a_short_206_is_a_fault_rather_than_a_short_read() {
    // Honest: the server declares the prefix it sent. The client asked for a
    // span and got less of it, which is a fault it must notice — the bytes
    // that did arrive are not the bytes the caller addressed.
    let oracle = Oracle::serving(dump_bytes()).short_range_after(1, 8).start();
    let source = source_of(&oracle.url()).await;
    let err = source.read_range(0, 64).await.unwrap_err().to_string();
    assert!(err.contains(&oracle.url()), "{err}");
}

#[tokio::test]
async fn a_body_that_stops_short_of_its_declared_length_is_a_failure() {
    // The other truncation: `Content-Length` says one thing and the connection
    // closes with bytes owing. A reader that trusted the header would hand
    // back a short chunk as though it were whole.
    let oracle = Oracle::serving(dump_bytes()).without_etag().truncating_body_after(1, 4).start();
    let source = source_of(&oracle.url()).await;
    let err = source.read_range(0, 64).await.unwrap_err().to_string();
    assert!(err.contains(&oracle.url()), "{err}");
}

#[tokio::test]
async fn an_object_withdrawn_mid_scan_is_an_error_naming_the_url() {
    let oracle = Oracle::serving(dump_bytes()).not_found_after(1).start();
    let source = source_of(&oracle.url()).await;
    let err = source.read_range(0, 64).await.unwrap_err().to_string();
    assert!(err.contains(&oracle.url()), "{err}");
}

// ---------------------------------------------------------------------------
// The stalled origin: the deadline (D9) and the cancellation race (D16)
// ---------------------------------------------------------------------------

/// How long a stalling oracle stays quiet where nothing interrupts it: long
/// enough that a wait it ended by itself would be unmistakable, since every
/// assertion below is that something *else* ended the wait.
const STALL: Duration = Duration::from_secs(30);

/// The request each test below stalls: the probe is the first, so the first
/// `read_range` is the second.
const FIRST_READ: usize = 2;

#[tokio::test]
async fn a_stalled_request_is_abandoned_and_retried_rather_than_waited_out() {
    // The shipped read timeout is minutes of wall clock to a test, so the
    // deadline is asserted through a stated one — the same setting, valued
    // where it can be produced. **Not `start_paused`**: the oracle is a
    // blocking server on real time, so a virtual-time client sees every real
    // server delay as infinite and the retry this needs to succeed would race
    // the crate's three-minute deadline.
    let oracle = Oracle::serving(dump_bytes()).stalling_request(FIRST_READ, STALL).start();
    let origin = Origin::remote_with_read_timeout(&oracle.url().parse().unwrap(), ms(150)).unwrap();
    let source = match open(&origin, KnownCompression::Unknown).await.unwrap() {
        Recognized::Source(source) => source,
        Recognized::Mismatch => unreachable!(),
    };

    let started = Instant::now();
    let read = source.read_range(0, 32).await;
    // The retry is answered, so the read succeeds — which is the point: a
    // connection that has gone quiet costs a deadline, not the run.
    assert!(read.is_ok(), "{:?}", read.err());
    assert!(started.elapsed() < STALL, "the deadline fired rather than the stall ending");
    assert!(
        oracle.request_count() > FIRST_READ,
        "the client gave up on the stalled request and asked again"
    );
}

#[tokio::test]
async fn a_cancellation_drops_the_request_in_flight() {
    let oracle = Oracle::serving(dump_bytes()).stalling_request(FIRST_READ, STALL).start();
    let source = source_of(&oracle.url()).await;
    let cancel = Arc::new(Cancellation::new());
    source.hint_cancellation(Arc::clone(&cancel));

    let asker = tokio::spawn(async move {
        tokio::time::sleep(ms(100)).await;
        cancel.cancel();
    });
    let started = Instant::now();
    let err = source.read_range(0, 32).await.unwrap_err();
    asker.await.unwrap();

    assert!(matches!(err, pgdump_query::Error::ScanCancelled { .. }), "{err}");
    assert!(
        started.elapsed() < STALL,
        "the request was waited out rather than dropped: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_source_nobody_has_cancelled_reads_as_before() {
    let oracle = serving_dump();
    let source = source_of(&oracle.url()).await;
    let cancel = Arc::new(Cancellation::new());
    source.hint_cancellation(cancel);
    assert_eq!(source.read_range(0, 4).await.unwrap().len(), 4);
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

// ---------------------------------------------------------------------------
// End to end: the three commands over a URL
// ---------------------------------------------------------------------------

/// A cache path in a fresh tempdir. A remote source has nothing to sit beside,
/// so every command below is given one.
fn cache_in(dir: &tempfile::TempDir) -> String {
    dir.path().join("remote.dqcache").to_str().unwrap().to_string()
}

#[test]
fn parse_over_a_url_finds_what_a_local_parse_finds() {
    let oracle = serving_dump();
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(&dir);
    let remote = run_ok(&["parse", "--source", &oracle.url(), "--dqcache", &cache]);

    let local_dir = tempfile::tempdir().unwrap();
    let local_cache = local_dir.path().join("local.dqcache");
    let dump = fixture("16/edge_cases/default.sql");
    let local = run_ok(&[
        "parse",
        "--source",
        dump.to_str().unwrap(),
        "--dqcache",
        local_cache.to_str().unwrap(),
    ]);

    assert_eq!(strip_cache_line(&remote), strip_cache_line(&local));
}

/// The one line of a `parse` listing that names the cache path, which differs
/// between two runs by construction.
fn strip_cache_line(output: &str) -> String {
    output
        .lines()
        .filter(|line| !line.starts_with("wrote cache to "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn info_reports_the_cache_a_remote_parse_left() {
    let oracle = serving_dump();
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(&dir);
    run_ok(&["parse", "--source", &oracle.url(), "--dqcache", &cache]);
    let report = run_ok(&["info", "--source", &oracle.url(), "--dqcache", &cache]);
    assert!(report.contains("public.widgets"), "{report}");
}

#[test]
fn query_over_a_url_streams_the_same_rows_a_local_query_streams() {
    let oracle = serving_dump();
    let dump = fixture("16/edge_cases/default.sql");
    let remote = run_ok(&[
        "query",
        "--source",
        &oracle.url(),
        "--table",
        "public.widgets",
        "--dqcache",
        "none",
        "--no-columns",
    ]);
    let local = run_ok(&[
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dqcache",
        "none",
        "--no-columns",
    ]);
    assert_eq!(remote, local);
}

// ---------------------------------------------------------------------------
// The derived cache path, and the origin it records (D4, D18, D19)
// ---------------------------------------------------------------------------

/// Run `pgdq` with `dir` as the working directory — which is where a remote
/// dump's default cache goes, there being no place beside the object for one
/// to sit.
fn run_in(dir: &tempfile::TempDir, args: &[&str]) -> std::process::Output {
    common::pgdq().current_dir(dir.path()).args(args).output().expect("the pgdq binary runs")
}

fn run_ok_in(dir: &tempfile::TempDir, args: &[&str]) -> String {
    let out = run_in(dir, args);
    assert!(out.status.success(), "pgdq {args:?} failed: {}", stderr_of(&out));
    String::from_utf8(out.stdout).expect("pgdq writes UTF-8")
}

#[test]
fn a_url_naming_no_object_is_refused_by_name() {
    for url in ["http://example.com", "http://example.com/", "https://example.com/dumps/"] {
        let err = Origin::resolve(url).unwrap_err().to_string();
        assert!(err.contains("names no object"), "{url}: {err}");
    }
}

#[test]
fn a_remote_cache_defaults_to_the_urls_last_segment_in_the_working_directory() {
    let oracle = serving_dump();
    let dir = tempfile::tempdir().unwrap();
    let said = run_ok_in(&dir, &["parse", "--source", &oracle.url()]);

    // Predictable by reading the URL: `…/dump.sql` becomes `./dump.sql.dqcache`.
    assert!(said.contains("wrote cache to dump.sql.dqcache"), "{said}");
    assert!(dir.path().join("dump.sql.dqcache").exists(), "{:?}", std::fs::read_dir(dir.path()));

    // And the run that follows reads it: `info` never scans, so a report of
    // this dump's tables can only have come from the cache just written.
    let report = run_ok_in(&dir, &["info", "--source", &oracle.url()]);
    assert!(report.contains("public.widgets"), "{report}");
}

#[test]
fn two_hosts_serving_the_same_name_share_a_cache_and_the_origin_says_so() {
    // The consequence D4 states rather than defends away: two same-named
    // dumps of equal stored size from different hosts, cached in one working
    // directory, read each other's map. The default path is what makes that
    // reachable; the recorded origin is what makes it visible.
    let here = serving_dump();
    let elsewhere = serving_dump();
    let dir = tempfile::tempdir().unwrap();
    run_ok_in(&dir, &["parse", "--source", &here.url()]);

    let report = run_ok_in(&dir, &["info", "--source", &elsewhere.url()]);
    assert!(report.contains("public.widgets"), "the cache is still read: {report}");
    assert!(report.contains("fetched from somewhere else"), "and the origin is reported: {report}");

    // `location` promotes that to a refusal, naming both origins.
    let refused =
        run_in(&dir, &["info", "--source", &elsewhere.url(), "--strict-identity=location"]);
    assert!(!refused.status.success());
    let said = stderr_of(&refused);
    assert!(said.contains(&here.url()) && said.contains(&elsewhere.url()), "{said}");
}

#[test]
fn a_cache_recorded_against_another_object_names_the_url_that_refused_it() {
    // The cache path is derived, so "the cache at dump.sql.dqcache" does not
    // say which `dump.sql` this run asked for.
    let here = serving_dump();
    let mut longer = dump_bytes();
    longer.extend_from_slice(b"\n-- one more byte\n");
    let elsewhere = Oracle::serving(longer).start();
    let dir = tempfile::tempdir().unwrap();
    run_ok_in(&dir, &["parse", "--source", &here.url()]);

    let refused = run_in(&dir, &["parse", "--source", &elsewhere.url()]);
    assert!(!refused.status.success());
    let said = stderr_of(&refused);
    assert!(said.contains(&format!("Error: {}: the cache at", elsewhere.url())), "{said}");
    assert!(said.contains("dump.sql.dqcache"), "the cache is still named: {said}");
}

// ---------------------------------------------------------------------------
// The precondition on every ranged GET (D10, D11)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_ranged_get_after_the_probe_pins_the_object() {
    let oracle = serving_dump();
    let source = source_of(&oracle.url()).await;
    source.read_range(0, 32).await.unwrap();
    source.read_range(64, 32).await.unwrap();

    let requests = oracle.requests();
    assert_eq!(requests.len(), 3, "{requests:?}");
    // The probe establishes the baseline, so it carries none; every read after
    // it does, which is a cadence the local provider cannot afford.
    assert_eq!(requests[0].header("if-match"), None, "{requests:?}");
    for request in &requests[1..] {
        assert_eq!(request.header("if-match"), Some(oracle.etag()), "{request:?}");
    }
}

#[tokio::test]
async fn an_object_rewritten_under_a_read_is_refused_by_the_server() {
    let oracle = Oracle::serving(dump_bytes()).etag_changing_after(1).start();
    let source = source_of(&oracle.url()).await;
    let err = source.read_range(0, 32).await.unwrap_err();
    assert!(
        matches!(err, pgdump_query::Error::SourceChangedWhileRead { .. }),
        "a 412 is the remote analogue of an `fstat` that moved, not a network fault: {err:?}"
    );
    let said = err.to_string();
    assert!(said.contains("nothing was saved and no cache was removed"), "{said}");
}

#[tokio::test]
async fn a_server_sending_no_entity_tag_is_pinned_by_its_modification_time() {
    let oracle = Oracle::serving(dump_bytes()).without_etag().start();
    let source = source_of(&oracle.url()).await;
    source.read_range(0, 32).await.unwrap();
    let requests = oracle.requests();
    assert_eq!(requests[0].header("if-unmodified-since"), None, "the probe is the baseline");
    assert!(requests[1].header("if-unmodified-since").is_some(), "{requests:?}");

    // And it refuses when the object moves on: the oracle serves a newer
    // `Last-Modified` from the moment its version changes.
    let oracle = Oracle::serving(dump_bytes()).without_etag().etag_changing_after(1).start();
    let source = source_of(&oracle.url()).await;
    let err = source.read_range(0, 32).await.unwrap_err();
    assert!(matches!(err, pgdump_query::Error::SourceChangedWhileRead { .. }), "{err:?}");
}

#[tokio::test]
async fn a_server_that_states_neither_validator_is_read_unpinned() {
    // `object_store` substitutes the epoch for an absent `Last-Modified`
    // (`docs/design/runtime-invariants.md`, "RT16"), so asking a server to
    // confirm nothing has changed since 1970 would refuse every read.
    let oracle = Oracle::serving(dump_bytes()).without_etag().without_last_modified().start();
    let source = source_of(&oracle.url()).await;
    assert_eq!(source.read_range(0, 32).await.unwrap().len(), 32);
    for request in &oracle.requests() {
        assert_eq!(request.header("if-match"), None, "{request:?}");
        assert_eq!(request.header("if-unmodified-since"), None, "{request:?}");
    }
}

#[test]
fn strict_identity_none_stops_pinning_the_object() {
    // The only opt-out, and the whole of what it buys: a scan whose object is
    // rewritten under it reads on instead of stopping.
    let oracle = Oracle::serving(dump_bytes()).etag_changing_after(1).start();
    let dir = tempfile::tempdir().unwrap();
    let bound = run_in(&dir, &["parse", "--source", &oracle.url()]);
    assert!(!bound.status.success(), "{}", stderr_of(&bound));
    assert!(stderr_of(&bound).contains("while it was being read"), "{}", stderr_of(&bound));

    let oracle = Oracle::serving(dump_bytes()).etag_changing_after(1).start();
    let dir = tempfile::tempdir().unwrap();
    run_ok_in(&dir, &["parse", "--source", &oracle.url(), "--strict-identity=none"]);
}

#[test]
fn an_interrupt_during_a_remote_read_is_answered_at_once_and_exits_by_signal() {
    // The whole of D16 from the outside: a remote read's wait is the retry
    // schedule, so a Ctrl-C that only set a flag would be answered minutes
    // later. The request is dropped instead, and `parse` reports what a local
    // interrupt reports.
    let oracle = Oracle::serving(dump_bytes()).stalling_request(FIRST_READ, STALL).start();
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(&dir);
    let child = common::pgdq()
        .args(["parse", "--source", &oracle.url(), "--dqcache", &cache])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the pgdq binary runs");

    // Wait until the scan's first read is the one being stalled, so the signal
    // lands inside a wait rather than before one.
    let deadline = Instant::now() + Duration::from_secs(20);
    while oracle.request_count() < FIRST_READ && Instant::now() < deadline {
        std::thread::sleep(ms(10));
    }
    assert_eq!(oracle.request_count(), FIRST_READ, "the stalled read arrived");

    let signalled = Instant::now();
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("`kill` runs")
            .success()
    );
    let out = child.wait_with_output().expect("pgdq exits");
    assert!(signalled.elapsed() < STALL, "the interrupt waited the request out");

    // 130 is SIGINT's, which is what a script reads to tell an interrupt from a
    // failure.
    assert_eq!(out.status.code(), Some(130), "{}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    // The words a *local* interrupt uses: the dropped read is the interrupt
    // arriving by another door, not a failure of its own
    // (`docs/design/decisions.md`, "D26"). The stall is the prepass's own
    // read, so this is the nothing-banked branch: no cache is written, so
    // none is named, and the re-run starts rather than continues.
    assert!(err.contains("interrupted at byte 0"), "{err}");
    assert!(err.contains("nothing was scanned or written"), "{err}");
    assert!(!err.contains(&cache), "no cache was written, so none is named: {err}");
    assert!(!std::path::Path::new(&cache).exists(), "and none is on disk");
    assert!(err.contains(&oracle.url()), "{err}");
}

#[tokio::test]
async fn a_cancelled_remote_read_is_an_interrupted_run_rather_than_an_error() {
    // The library half of the test above, which is the half an embedder sees:
    // `map_file` answers a dropped read with the run it banked, exactly as it
    // answers the polled flag, so the two providers differ in how fast a
    // cancellation is noticed and not in what a caller gets back.
    let oracle = Oracle::serving(dump_bytes()).stalling_request(FIRST_READ, STALL).start();
    let source = source_of(&oracle.url()).await;
    let dir = tempfile::tempdir().unwrap();
    let mode = CacheMode::enabled(dir.path().join("remote.dqcache"));
    let cancel = Arc::new(Cancellation::new());
    let options = ScanOptions { cancel: Some(Arc::clone(&cancel)), ..ScanOptions::default() };

    // Cancel while the stalled read is in flight, so the flag lands inside a
    // wait rather than at a check point: the stall outlasts this sleep by two
    // orders of magnitude.
    let asker = tokio::spawn(async move {
        tokio::time::sleep(ms(300)).await;
        cancel.cancel();
    });

    let started = Instant::now();
    let run = pgdump_query::map_file(source.as_ref(), &options, &mode, &StatisticsRequest::NONE)
        .await
        .expect("a cancelled read is not an error");
    asker.await.unwrap();

    assert!(run.interrupted, "the run says it stopped short");
    assert!(!run.index.is_complete(dump_bytes().len() as u64), "the map is short of the file");
    assert!(started.elapsed() < STALL, "the request was waited out rather than dropped");
}

#[test]
fn a_network_failure_names_the_url_and_the_command_exits_non_zero() {
    let oracle = Oracle::serving(dump_bytes()).not_found_after(1).start();
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(&dir);
    let out = run(&["parse", "--source", &oracle.url(), "--dqcache", &cache]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains(&oracle.url()), "{err}");
}

#[test]
fn a_scheme_this_build_does_not_read_is_refused_before_anything_is_fetched() {
    let out = run(&["info", "--source", "ftp://example.com/koji.dump", "--dqcache", "x.dqcache"]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains("`ftp:`"), "{err}");
}

// ---------------------------------------------------------------------------
// A remote `.xz` (D1, D13, D20)
// ---------------------------------------------------------------------------

/// One `.xz` stream over `bytes`, split into several blocks. `xz` is not
/// `mise`-pinned, so a missing binary fails loudly here rather than the test
/// silently skipping (`docs/design/roadmap.md`, "A test may assume the tools
/// `mise` pins").
fn xz_stream(bytes: &[u8]) -> Vec<u8> {
    let mut input = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut input, bytes).unwrap();
    std::io::Write::flush(&mut input).unwrap();
    let out = std::process::Command::new("xz")
        .args(["--block-size=512", "-c"])
        .arg(input.path())
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// The dump every test above serves, compressed as **two concatenated
/// streams** of several blocks each. That shape is the one D1 is about: the
/// footer walk is a backward chain costing at least one round trip per
/// stream, so a file with more than one stream is what makes the walk's cost
/// visible at all.
fn xz_dump_bytes() -> Vec<u8> {
    let body = dump_bytes();
    let half = body.len() / 2;
    let mut out = xz_stream(&body[..half]);
    out.extend_from_slice(&xz_stream(&body[half..]));
    out
}

/// An oracle serving that, under a name whose last segment is what the
/// derived cache is called.
fn serving_xz_dump() -> Oracle {
    Oracle::serving(xz_dump_bytes()).start()
}

fn xz_url(oracle: &Oracle) -> String {
    oracle.url_for("dump.sql.xz")
}

#[tokio::test]
async fn a_remote_xz_source_reads_the_plain_bytes_the_file_holds() {
    let oracle = serving_xz_dump();
    let source = source_of(&xz_url(&oracle)).await;
    let plain = dump_bytes();

    let table = source.seek_table().expect("a fetched `.xz` source carries its table");
    assert_eq!(table.stream_count(), 2, "the fixture is two concatenated streams");
    assert!(table.block_count() > 2, "and several blocks: {table:?}");
    assert_eq!(source.size().await.unwrap(), plain.len() as u64, "the uncompressed length");
    assert_eq!(
        source.stored_size().await.unwrap(),
        oracle.body().len() as u64,
        "and the stored length is the compressed object's",
    );
    assert_eq!(&source.read_range(0, 64).await.unwrap()[..], &plain[..64]);
    assert_eq!(&source.read_range(900, 700).await.unwrap()[..], &plain[900..1_600]);
    assert_eq!(&source.read_range(0, plain.len()).await.unwrap()[..], &plain[..]);
}

#[tokio::test]
async fn a_remote_xz_read_with_no_room_for_a_block_answers_the_same_bytes() {
    // The piecewise arm over a real transport: the budget holds no decoded
    // block, so each one is fetched, skipped into and completed through
    // `xz_seek::BlockRead` (D20).
    let oracle = serving_xz_dump();
    let source = source_of(&xz_url(&oracle)).await;
    source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    let plain = dump_bytes();
    assert_eq!(&source.read_range(900, 700).await.unwrap()[..], &plain[900..1_600]);
}

#[tokio::test]
async fn a_fetched_piecewise_scan_inside_one_block_fetches_it_once() {
    // The piecewise arm keeps the block it is reading — the window it fetched
    // and the handle decoding out of it — so a forward scan inside one block
    // costs one ranged GET rather than one per read; both are dropped when the
    // scan leaves the block, and nothing is cached behind them, so a read
    // going back re-fetches (D21). That a block is decoded once as well as
    // fetched once is counted in `io.rs` under `introspect`, this oracle
    // seeing only the request stream.
    let oracle = serving_xz_dump();
    let source = source_of(&xz_url(&oracle)).await;
    source.hint_parallelism(Parallelism::Serial { memory_bytes: Some(1) });
    let plain = dump_bytes();
    let table = source.seek_table().expect("a fetched `.xz` source carries its table");
    let first = table.blocks[0].uncompressed_size as usize;
    let step = first / 8;
    assert!(step > 0, "the fixture's blocks hold several reads apiece: {first} byte(s)");

    let fetches = |from: usize| oracle.requests().len() - from;
    let mark = oracle.requests().len();
    for read in 0..4usize {
        let at = read * step;
        assert_eq!(&source.read_range(at as u64, step).await.unwrap()[..], &plain[at..at + step]);
    }
    assert_eq!(fetches(mark), 1, "four reads inside one block, one fetch: {:?}", oracle.requests());

    // Moving on fetches the next block, which is also what proves the window
    // is released rather than accumulated.
    let mark = oracle.requests().len();
    let second = table.blocks[1].uncompressed_offset as usize;
    assert_eq!(
        &source.read_range(second as u64, step).await.unwrap()[..],
        &plain[second..second + step]
    );
    assert_eq!(fetches(mark), 1, "the next block is one more fetch");

    // And this is the piecewise arm rather than the block cache: going back
    // fetches block 0 again, where a retained *block* would answer for free.
    let mark = oracle.requests().len();
    assert_eq!(&source.read_range(0, step).await.unwrap()[..], &plain[..step]);
    assert_eq!(fetches(mark), 1, "nothing is retained behind the last block");
}

#[tokio::test]
async fn every_fetch_a_footer_walk_makes_is_pinned_to_the_probes_version() {
    // 14.6's precondition rides on the fetch, so the walk inherits it without
    // knowing it exists (D10, D11).
    let oracle = serving_xz_dump();
    let source = source_of(&xz_url(&oracle)).await;
    assert!(source.seek_table().is_some());

    let requests = oracle.requests();
    assert!(requests.len() > 2, "the walk fetched more than the probe: {requests:?}");
    assert_eq!(requests[0].header("if-match"), None, "the probe establishes the baseline");
    for request in &requests[1..] {
        assert_eq!(request.header("if-match"), Some(oracle.etag()), "{request:?}");
    }
}

#[test]
fn parse_over_a_remote_xz_announces_the_walk_before_paying_for_it() {
    let oracle = serving_xz_dump();
    let dir = tempfile::tempdir().unwrap();
    let out = run_in(&dir, &["parse", "--source", &xz_url(&oracle)]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let said = stderr_of(&out);

    // What it is about to cost, and both ways out of it — never a refusal.
    assert!(said.contains("seek table build started"), "{said}");
    assert!(said.contains("one round trip per stream"), "{said}");
    assert!(said.contains("keep the cache"), "{said}");
    assert!(said.contains("parse a local copy"), "{said}");
    // And it lands before the walk's own fetches, not after them.
    let announced = said.find("seek table build started").unwrap();
    let finished = said.find("seek table build complete").expect("the walk reports what it cost");
    assert!(announced < finished, "{said}");
}

#[test]
fn a_cached_seek_table_spares_a_remote_xz_the_walk_entirely() {
    // The warm case D1 rests on: with the table in the cache, `info` costs the
    // origin probe and nothing else — no walk, no scan
    // (`docs/design/decisions.md`, "D18").
    let oracle = serving_xz_dump();
    let dir = tempfile::tempdir().unwrap();
    let said = run_in(&dir, &["parse", "--source", &xz_url(&oracle)]);
    assert!(said.status.success(), "{}", stderr_of(&said));
    assert!(
        String::from_utf8_lossy(&said.stdout).contains("wrote cache to dump.sql.xz.dqcache"),
        "{}",
        String::from_utf8_lossy(&said.stdout),
    );
    let after_parse = oracle.request_count();
    assert!(after_parse > 2, "the cold run walked and scanned: {after_parse}");

    let report = run_ok_in(&dir, &["info", "--source", &xz_url(&oracle)]);
    assert!(report.contains("public.widgets"), "{report}");
    assert_eq!(oracle.request_count() - after_parse, 1, "the origin probe, and nothing else");
}

#[test]
fn query_over_a_remote_xz_streams_the_same_rows_the_plain_dump_streams() {
    let oracle = serving_xz_dump();
    let dump = fixture("16/edge_cases/default.sql");
    let remote = run_ok(&[
        "query",
        "--source",
        &xz_url(&oracle),
        "--table",
        "public.widgets",
        "--dqcache",
        "none",
        "--no-columns",
    ]);
    let local = run_ok(&[
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dqcache",
        "none",
        "--no-columns",
    ]);
    assert_eq!(remote, local);
}

#[tokio::test]
async fn open_remote_reports_a_compression_claim_the_object_contradicts() {
    // Both directions of "the cache describes a different file" reach a remote
    // object now that a compressed one can be opened, and each is
    // `Recognized::Mismatch` rather than a silent fallback — exactly as
    // `open_local` reads them (`docs/design/decisions.md`, "D18").
    let compressed = serving_xz_dump();
    let origin = Origin::resolve(&xz_url(&compressed)).unwrap();
    assert!(
        matches!(open(&origin, KnownCompression::Plain).await.unwrap(), Recognized::Mismatch),
        "a plain claim over `.xz` bytes",
    );

    let mut file = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut file, &xz_dump_bytes()).unwrap();
    std::io::Write::flush(&mut file).unwrap();
    let table = pgdump_query::XzSource::open(file.path()).unwrap().seek_table().unwrap();

    let plain = serving_dump();
    let origin = Origin::resolve(&plain.url()).unwrap();
    assert!(
        matches!(open(&origin, KnownCompression::Xz(table)).await.unwrap(), Recognized::Mismatch),
        "a compression index over plain bytes",
    );
}
