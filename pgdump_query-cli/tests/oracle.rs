//! The oracle's own tests: proof that it misbehaves exactly as asked
//! (`docs/design/roadmap-P14-remote-input.md`, "D6").
//!
//! **The instrument precedes its subject**, which is why this file exists
//! before anything reads a URL
//! (`docs/design/roadmap-P14-remote-input.md`, "How it is sliced, and why in
//! that order"). Nothing here names a remote source, `object_store` or the
//! `http` feature: every assertion is against bytes on a socket, so the proof
//! that the server ignores a `Range` does not rest on the crate whose handling
//! of that case is the thing being tested.
//!
//! Each knob gets two assertions where the difference matters: that the
//! misbehaviour happens, and that it happens *only* from the request it was
//! addressed to. A knob that fired a request early would make every later test
//! read as passing for the wrong reason.

mod common;

use std::io::Write;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use common::oracle::{Oracle, RawResponse, http_date, parse_http_date, raw_get, raw_head};

/// A body long enough that a range is a proper subset of it and short enough
/// to compare whole.
const BODY: &[u8] = b"-- a dump that is not really a dump, 64 bytes of it, padded.\n\x41\x42\x43";

fn assert_status(response: &RawResponse, expected: u16) {
    assert_eq!(
        response.status,
        expected,
        "headers: {:?}, body: {:?}",
        response.headers,
        String::from_utf8_lossy(&response.body)
    );
}

// ---------------------------------------------------------------------------
// The control: an oracle with no knob set is a correct origin server
// ---------------------------------------------------------------------------

#[test]
fn a_ranged_get_answers_206_with_the_slice_and_the_total_size() {
    let oracle = Oracle::serving(BODY).start();
    let response = raw_get(&oracle, &["Range: bytes=10-19"]);

    assert_status(&response, 206);
    assert_eq!(response.body, &BODY[10..=19]);
    assert!(response.complete());
    assert_eq!(
        response.header("content-range"),
        Some(format!("bytes 10-19/{}", BODY.len()).as_str())
    );
    assert_eq!(response.content_length(), Some(10));
}

#[test]
fn an_open_ended_and_a_suffix_range_both_resolve_against_the_length() {
    let oracle = Oracle::serving(BODY).start();

    let open = raw_get(&oracle, &["Range: bytes=60-"]);
    assert_status(&open, 206);
    assert_eq!(open.body, &BODY[60..]);

    let suffix = raw_get(&oracle, &["Range: bytes=-3"]);
    assert_status(&suffix, 206);
    assert_eq!(suffix.body, &BODY[BODY.len() - 3..]);
    assert_eq!(
        suffix.header("content-range"),
        Some(format!("bytes {}-{}/{}", BODY.len() - 3, BODY.len() - 1, BODY.len()).as_str())
    );
}

#[test]
fn an_unranged_get_answers_200_with_the_whole_body() {
    let oracle = Oracle::serving(BODY).start();
    let response = raw_get(&oracle, &[]);

    assert_status(&response, 200);
    assert_eq!(response.body, BODY);
    assert_eq!(response.header("accept-ranges"), Some("bytes"));
}

#[test]
fn a_head_answers_the_size_and_the_validators_with_no_body() {
    let oracle = Oracle::serving(BODY).start();
    let response = raw_head(&oracle, &[]);

    assert_status(&response, 200);
    assert!(response.body.is_empty(), "a HEAD carries no body");
    assert_eq!(response.content_length(), Some(BODY.len()));
    assert_eq!(response.header("etag"), Some(oracle.etag()));
    assert!(response.header("last-modified").is_some());
}

/// `object_store` requires `Content-Length` unconditionally and refuses a
/// chunked or content-encoded response, so an oracle that ever omitted the
/// header would be exercising a case the crate rejects before any of our code
/// runs (`docs/design/roadmap-P14-remote-input.md`, "What the backend actually
/// does").
#[test]
fn every_response_declares_a_length_and_none_is_chunked() {
    let oracle = Oracle::serving(BODY)
        .etag_changing_after(3)
        .not_found_after(4)
        .short_range_after(1, 4)
        .start();

    for extra in [&["Range: bytes=0-9"][..], &[][..], &["If-Match: \"nope\""][..], &[][..]] {
        let response = raw_get(&oracle, extra);
        assert!(
            response.content_length().is_some(),
            "no Content-Length on {}: {:?}",
            response.status,
            response.headers
        );
        assert_eq!(response.header("transfer-encoding"), None);
        assert_eq!(response.header("content-encoding"), None);
    }
    // The loop spent four requests, and the fifth is the withdrawn one.
    let withdrawn = raw_get(&oracle, &[]);
    assert_status(&withdrawn, 404);
    assert!(withdrawn.content_length().is_some());
}

#[test]
fn a_method_that_is_not_get_or_head_is_refused() {
    let oracle = Oracle::serving(BODY).start();
    let response = common::oracle::raw_request(&oracle, "PUT", "/dump.sql", &[]);
    assert_status(&response, 405);
}

#[test]
fn a_range_past_the_end_is_416_and_names_the_length() {
    let oracle = Oracle::serving(BODY).start();
    let response = raw_get(&oracle, &[&format!("Range: bytes={}-{}", BODY.len(), BODY.len() + 9)]);

    assert_status(&response, 416);
    assert_eq!(response.header("content-range"), Some(format!("bytes */{}", BODY.len()).as_str()));
}

/// An oracle that silently ignored a header it could not parse would hide the
/// bug it exists to surface, so a malformed `Range` is a refusal rather than a
/// full-body answer.
#[test]
fn an_unparseable_range_is_refused_rather_than_ignored() {
    let oracle = Oracle::serving(BODY).start();
    assert_status(&raw_get(&oracle, &["Range: rows=1-2"]), 400);
    assert_status(&raw_get(&oracle, &["Range: bytes=0-9,20-29"]), 400);
}

// ---------------------------------------------------------------------------
// The knobs
// ---------------------------------------------------------------------------

#[test]
fn the_range_ignoring_knob_answers_200_with_the_whole_body() {
    let oracle = Oracle::serving(BODY).ignoring_range().start();
    let response = raw_get(&oracle, &["Range: bytes=10-19"]);

    assert_status(&response, 200);
    assert_eq!(response.body, BODY, "a range-ignorant server sends everything");
    assert_eq!(response.header("content-range"), None);
}

#[test]
fn the_etag_knob_changes_the_validator_from_the_stated_request_onward() {
    let oracle = Oracle::serving(BODY).etag_changing_after(2).start();

    let first = raw_get(&oracle, &["Range: bytes=0-3"]);
    let second = raw_get(&oracle, &["Range: bytes=0-3"]);
    let third = raw_get(&oracle, &["Range: bytes=0-3"]);

    assert_eq!(first.header("etag"), Some(oracle.etag()));
    assert_eq!(second.header("etag"), Some(oracle.etag()), "the knob fires after request 2");
    assert_eq!(third.header("etag"), Some(oracle.changed_etag()));
    assert_ne!(oracle.etag(), oracle.changed_etag());
    assert_ne!(
        third.header("last-modified"),
        first.header("last-modified"),
        "the rewritten object is newer, not merely differently tagged"
    );
}

/// The mechanism `docs/design/roadmap-P14-remote-input.md`, "D10" pins a
/// remote read with: the validator taken at the probe rides every later ranged
/// GET, so a rewrite mid-scan comes back as a refusal instead of as mixed
/// bytes.
#[test]
fn a_precondition_carrying_the_stale_validator_is_refused_once_the_object_changes() {
    let oracle = Oracle::serving(BODY).etag_changing_after(1).start();
    let pinned = format!("If-Match: {}", oracle.etag());

    let first = raw_get(&oracle, &[&pinned, "Range: bytes=0-3"]);
    assert_status(&first, 206);
    assert_eq!(first.body, &BODY[0..=3]);

    let after = raw_get(&oracle, &[&pinned, "Range: bytes=4-7"]);
    assert_status(&after, 412);
    assert!(after.body.is_empty(), "a refusal carries no object bytes");
}

#[test]
fn a_precondition_against_a_source_with_no_validator_is_refused() {
    let oracle = Oracle::serving(BODY).without_etag().start();
    let response = raw_get(&oracle, &["If-Match: \"anything\"", "Range: bytes=0-3"]);

    assert_status(&response, 412);
}

#[test]
fn an_unmodified_since_precondition_refuses_only_a_newer_object() {
    let oracle = Oracle::serving(BODY).etag_changing_after(1).start();
    let opened = raw_head(&oracle, &[]).header("last-modified").unwrap().to_string();
    let pinned = format!("If-Unmodified-Since: {opened}");

    // The knob has fired by now, so the object is an hour newer than the date
    // the run opened on.
    assert_status(&raw_get(&oracle, &[&pinned, "Range: bytes=0-3"]), 412);
}

#[test]
fn the_short_range_knob_answers_fewer_bytes_than_asked_and_declares_what_it_sent() {
    let oracle = Oracle::serving(BODY).short_range_after(1, 4).start();

    let full = raw_get(&oracle, &["Range: bytes=0-19"]);
    assert_eq!(full.body.len(), 20, "the knob fires after request 1");

    let short = raw_get(&oracle, &["Range: bytes=0-19"]);
    assert_status(&short, 206);
    assert_eq!(short.body, &BODY[0..4]);
    assert_eq!(short.content_length(), Some(4));
    assert_eq!(
        short.header("content-range"),
        Some(format!("bytes 0-3/{}", BODY.len()).as_str()),
        "a short answer is an honest one: the head says what was sent"
    );
    assert!(short.complete(), "a short range is not a truncated body");
}

#[test]
fn the_truncating_knob_declares_a_length_it_does_not_send() {
    let oracle = Oracle::serving(BODY).truncating_body_after(0, 5).start();
    let response = raw_get(&oracle, &["Range: bytes=0-19"]);

    assert_status(&response, 206);
    assert_eq!(response.content_length(), Some(20), "the head is not the lie's carrier");
    assert_eq!(response.body, &BODY[0..5]);
    assert!(!response.complete(), "the connection closed with fifteen bytes owing");
}

#[test]
fn the_not_found_knob_withdraws_the_object_from_the_stated_request_onward() {
    let oracle = Oracle::serving(BODY).not_found_after(2).start();

    assert_status(&raw_get(&oracle, &["Range: bytes=0-3"]), 206);
    assert_status(&raw_get(&oracle, &["Range: bytes=4-7"]), 206);
    assert_status(&raw_get(&oracle, &["Range: bytes=8-11"]), 404);
    assert_status(&raw_head(&oracle, &[]), 404);
}

/// `object_store` substitutes the Unix epoch where a server sends no
/// `Last-Modified`, so "the server said nothing" and "the server said 1970" are
/// indistinguishable in its metadata — which is the asymmetry
/// `docs/design/roadmap-P14-remote-input.md`, "D5" reads as absence. The
/// oracle has to be able to produce both halves of it.
#[test]
fn the_suppressing_knobs_omit_the_weak_identity_headers() {
    let neither = Oracle::serving(BODY).without_etag().without_last_modified().start();
    let response = raw_head(&neither, &[]);
    assert_status(&response, 200);
    assert_eq!(response.header("etag"), None);
    assert_eq!(response.header("last-modified"), None);

    let half = Oracle::serving(BODY).without_etag().start();
    let response = raw_head(&half, &[]);
    assert_eq!(response.header("etag"), None);
    assert!(response.header("last-modified").is_some());
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

/// The other half of the instrument. A test asserting that the origin probe
/// costs one round trip rather than two, or that a precondition rode on every
/// ranged GET, reads it here.
#[test]
fn the_log_records_every_request_in_arrival_order() {
    let oracle = Oracle::serving(BODY).start();
    raw_head(&oracle, &[]);
    raw_get(&oracle, &["Range: bytes=0-3", "If-Match: \"x\""]);
    raw_get(&oracle, &[]);

    let requests = oracle.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(oracle.request_count(), 3);

    assert_eq!(requests[0].method, "HEAD");
    assert_eq!(requests[0].path, "/dump.sql");
    assert_eq!(requests[0].range(), None);

    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].range(), Some("bytes=0-3"));
    assert_eq!(requests[1].header("if-match"), Some("\"x\""));
    assert_eq!(requests[1].header("If-Match"), Some("\"x\""), "lookup is case-insensitive");

    assert_eq!(requests[2].range(), None);
}

// ---------------------------------------------------------------------------
// Serving a fixture, and the dates
// ---------------------------------------------------------------------------

#[test]
fn a_fixture_is_served_byte_for_byte() {
    let path = common::fixture("16/edge_cases/default.sql");
    let expected = std::fs::read(&path).expect("the fixture exists");
    let oracle = Oracle::serving_file(&path).start();

    assert_eq!(oracle.body(), expected.as_slice());
    let whole = raw_get(&oracle, &[]);
    assert_eq!(whole.body, expected);

    let middle = raw_get(&oracle, &["Range: bytes=100-199"]);
    assert_eq!(middle.body, &expected[100..200]);
    assert_eq!(middle.content_length(), Some(100));
}

#[test]
fn http_dates_round_trip_and_read_as_imf_fixdate() {
    // The epoch, a leap day, and the instant the oracle's first version is
    // stamped with.
    for unix in [0, 951_782_400, 1_784_764_800, 2_000_000_000] {
        assert_eq!(parse_http_date(&http_date(unix)), Some(unix), "{unix} does not round trip");
    }
    assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    assert_eq!(http_date(784_111_777), "Sun, 06 Nov 1994 08:49:37 GMT");
    assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), None, "obsolete formats are not");
    assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 UTC"), None, "GMT or nothing");
}

// ---------------------------------------------------------------------------
// The stalled origin
// ---------------------------------------------------------------------------

/// Long enough to be unmistakably a stall against a loopback server that
/// otherwise answers in microseconds, short enough that the test which waits
/// it out costs a fifth of a second.
const STALL: Duration = Duration::from_millis(200);

#[test]
fn a_stalled_request_is_answered_only_after_the_stall() {
    let oracle = Oracle::serving(BODY).stalling_request(1, STALL).start();
    let started = Instant::now();
    let response = raw_get(&oracle, &["Range: bytes=0-3"]);
    let waited = started.elapsed();

    assert_status(&response, 206);
    assert_eq!(response.body, &BODY[..4], "the answer, once it comes, is the right one");
    assert!(waited >= STALL, "answered in {waited:?}, which is no stall at all");
}

#[test]
fn only_the_addressed_request_stalls() {
    let oracle = Oracle::serving(BODY).stalling_request(2, STALL).start();
    // A stall addressed to one request rather than to a suffix is what lets a
    // client's *recovery* be observed, so the requests either side of it must
    // be answered at once.
    let before = Instant::now();
    assert_status(&raw_get(&oracle, &[]), 200);
    assert!(before.elapsed() < STALL, "the request before the stalled one waited");

    let during = Instant::now();
    assert_status(&raw_get(&oracle, &[]), 200);
    assert!(during.elapsed() >= STALL, "the addressed request did not stall");

    let after = Instant::now();
    assert_status(&raw_get(&oracle, &[]), 200);
    assert!(after.elapsed() < STALL, "the request after the stalled one waited");
}

#[test]
fn a_client_that_hangs_up_mid_stall_frees_the_server_at_once() {
    // The property the single-threaded server lives or dies by: a client that
    // gives up on a stalled request is asking for the retry to be served, and
    // a stall that outlived it would hold that retry behind itself.
    let oracle = Oracle::serving(BODY).stalling_request(1, Duration::from_secs(30)).start();
    let mut abandoned = TcpStream::connect(oracle.addr()).unwrap();
    abandoned
        .write_all(format!("GET /dump.sql HTTP/1.1\r\nHost: {}\r\n\r\n", oracle.addr()).as_bytes())
        .unwrap();
    // Wait for the request to reach the server, so the hang-up lands inside
    // the stall rather than before it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while oracle.request_count() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(oracle.request_count(), 1, "the stalled request arrived");
    drop(abandoned);

    let started = Instant::now();
    assert_status(&raw_get(&oracle, &[]), 200);
    assert!(started.elapsed() < Duration::from_secs(5), "the retry waited out the abandoned stall");
}
