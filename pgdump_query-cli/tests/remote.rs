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
use pgdump_query::{
    ByteRangeSource, Cancellation, KnownCompression, Origin, Recognized, open, open_local,
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
    // where it can be produced.
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

#[test]
fn a_remote_source_with_no_dqcache_asks_for_one_rather_than_deriving_a_name() {
    let oracle = serving_dump();
    let out = run(&["parse", "--source", &oracle.url()]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains("--dqcache <path>"), "{err}");
    assert!(err.contains(&oracle.url()), "{err}");
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
    assert!(err.contains("interrupted while reading byte"), "{err}");
    assert!(err.contains(&oracle.url()), "{err}");
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
