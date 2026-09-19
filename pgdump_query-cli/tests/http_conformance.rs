//! The remote source read against a **real** static origin rather than against
//! `common::oracle`.
//!
//! The oracle is ours on both sides of the wire, so the one fault it cannot
//! surface is a misreading of HTTP that our client and our server share: an
//! off-by-one in a `Range`, a `Content-Range` we spell wrong and parse back
//! the same way, a validator no shipped server would accept. That is the risk
//! named in `docs/design/roadmap-P14-remote-input.md`, "D6" under `Reopens`,
//! and this file is what answers it.
//!
//! **Opt-in, and empty by default.** Nothing below runs unless
//! `PGDQ_HTTP_CONFORMANCE_URL` names an object, so `cargo test --workspace`
//! passes in every checkout and in CI with no server standing anywhere. That
//! is the opposite of `common::require_uv`'s rule and deliberately so: `uv` is
//! pinned by `mise.toml`, so its absence is a machine that is not set up,
//! while a static origin is something a person raises for one sitting and
//! takes down again. What is **not** skipped is anything past the variable
//! being set — an unreachable URL, an object that is not this checkout's
//! fixture, a server that ignores `Range` all fail here rather than pass
//! quietly.
//!
//! How to raise one and what it must serve is [`CONTRIBUTING.md`](../../CONTRIBUTING.md),
//! "Building and testing".

mod common;

use std::sync::Arc;

use common::{fixture, run_ok, strip_cache_line};
use pgdump_query::{ByteRangeSource, KnownCompression, Origin, Recognized, open};

/// The variable that turns this file on, by naming the object to read.
const URL_VAR: &str = "PGDQ_HTTP_CONFORMANCE_URL";

/// The fixture the object must be a byte-identical copy of. It is the dump the
/// oracle tests serve, so a conformance failure and an oracle failure are
/// about the same bytes.
const FIXTURE: &str = "16/edge_cases/default.sql";

fn dump_bytes() -> Vec<u8> {
    std::fs::read(fixture(FIXTURE)).expect("the fixture is checked in")
}

/// The URL to read, or `None` when nobody asked for a conformance run.
///
/// Everything it can check without touching the network it checks here, so a
/// mistyped variable is a message rather than a connection error: a value
/// `pgdq` would read as a local path is refused by name, since a `file:` URL
/// exercises none of what this file exists to test.
fn conformance_url() -> Option<String> {
    let url = std::env::var(URL_VAR).ok()?;
    let url = url.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let origin = Origin::resolve(&url)
        .unwrap_or_else(|e| panic!("{URL_VAR} is not a source `pgdq` can read: {e}"));
    assert!(
        origin.local_path().is_none(),
        "{URL_VAR} names {url}, which `pgdq` reads as a local path — a conformance run needs an \
         origin reached over HTTP, since the transport is the subject"
    );
    Some(url)
}

/// The source `url` names, opened as a run opens it: probe, then recognition.
async fn source_of(url: &str) -> Arc<dyn ByteRangeSource> {
    let origin = Origin::resolve(url).expect("the URL resolves");
    match open(&origin, KnownCompression::Unknown).await {
        Ok(Recognized::Source(source)) => source,
        Ok(Recognized::Mismatch) => panic!("nothing was claimed, so nothing can be contradicted"),
        Err(e) => panic!("{url} did not open: {e}"),
    }
}

/// A cache path in a fresh tempdir. A remote source has nothing to sit beside,
/// so every command below is given one.
fn cache_in(dir: &tempfile::TempDir) -> String {
    dir.path().join("conformance.dqcache").to_str().unwrap().to_string()
}

// ---------------------------------------------------------------------------
// What the probe reads off a real 206
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_object_named_is_the_fixture_this_checkout_holds() {
    let Some(url) = conformance_url() else { return };
    let body = dump_bytes();

    // The size arrives out of `Content-Range`'s total rather than out of a
    // `Content-Length`, which is the reading a second implementation of ours
    // could agree with wrongly.
    let origin = Origin::resolve(&url).unwrap();
    let probe = origin.probe().await.unwrap_or_else(|e| {
        panic!(
            "{url} did not answer the probe: {e}\nA conformance origin must serve ranged GETs; a \
             server that ignores `Range` is refused here by design."
        )
    });
    assert_eq!(
        probe.stored_size(),
        body.len() as u64,
        "{url} is not a copy of fixtures/{FIXTURE} — it is {} bytes against {}",
        probe.stored_size(),
        body.len()
    );
    assert_eq!(probe.leading(), &body[..probe.leading().len()], "{url} leads with other bytes");

    // And the whole object, so the rest of the file may compare against the
    // local copy rather than against what the origin happens to hold.
    let source = source_of(&url).await;
    let whole = source.read_range(0, body.len()).await.expect("the object reads whole");
    assert_eq!(&whole[..], &body[..], "{url} is not byte-identical to fixtures/{FIXTURE}");
}

#[tokio::test]
async fn a_ranged_read_answers_the_bytes_at_the_offset_asked_for() {
    let Some(url) = conformance_url() else { return };
    let source = source_of(&url).await;
    let body = dump_bytes();
    let size = body.len() as u64;

    // Both ends and a single byte: an inclusive-range off-by-one is invisible
    // in the middle of an object and shows at its last byte.
    for (offset, len) in [(0u64, 32usize), (1, 1), (100, 64), (size - 8, 8), (size - 1, 1)] {
        let got = source
            .read_range(offset, len)
            .await
            .unwrap_or_else(|e| panic!("{len} byte(s) at {offset} did not read: {e}"));
        let want = &body[offset as usize..offset as usize + len];
        assert_eq!(&got[..], want, "{len} byte(s) at {offset}");
    }
}

#[tokio::test]
async fn a_read_past_the_end_is_refused_rather_than_answered_short() {
    let Some(url) = conformance_url() else { return };
    let source = source_of(&url).await;
    let size = source.size().await.unwrap();

    // Which refusal arrives is the server's to choose — a satisfiable range it
    // answers short, or a `416` — so this pins only that the short answer is
    // never handed back as the span that was asked for, and that whatever is
    // reported names the object.
    let err = source
        .read_range(size - 4, 64)
        .await
        .expect_err("a span running past the object's end is not a span the object holds")
        .to_string();
    assert!(err.contains(&url), "{err}");
}

#[tokio::test]
async fn a_real_server_accepts_the_precondition_every_read_after_the_probe_carries() {
    let Some(url) = conformance_url() else { return };
    let source = source_of(&url).await;

    // Every ranged GET after the probe pins the object to the version the
    // probe saw, with whichever validator the server offered. Nothing here can
    // see the request, so what this proves is the half the oracle cannot: that
    // the validator we build out of a real server's own `ETag` or
    // `Last-Modified` is one that server takes back, rather than a spelling
    // only we accept, which would refuse every read of an object nobody has
    // touched.
    for offset in [0u64, 64, 128] {
        source
            .read_range(offset, 32)
            .await
            .unwrap_or_else(|e| panic!("a pinned read at {offset} was refused: {e}"));
    }
}

// ---------------------------------------------------------------------------
// End to end, which is where the precondition is paid on every chunk
// ---------------------------------------------------------------------------

#[test]
fn parse_over_a_real_origin_finds_what_a_local_parse_finds() {
    let Some(url) = conformance_url() else { return };
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(&dir);
    let remote = run_ok(&["parse", "--source", &url, "--dqcache", &cache]);

    let local_dir = tempfile::tempdir().unwrap();
    let local_cache = local_dir.path().join("local.dqcache");
    let dump = fixture(FIXTURE);
    let local = run_ok(&[
        "parse",
        "--source",
        dump.to_str().unwrap(),
        "--dqcache",
        local_cache.to_str().unwrap(),
    ]);
    assert_eq!(strip_cache_line(&remote), strip_cache_line(&local));

    // And the cache that scan left reads back against the same URL, so the
    // origin recorded in it is one the object still matches.
    let report = run_ok(&["info", "--source", &url, "--dqcache", &cache]);
    assert!(report.contains("public.widgets"), "{report}");
}

#[test]
fn query_over_a_real_origin_streams_the_same_rows_a_local_query_streams() {
    let Some(url) = conformance_url() else { return };
    let dump = fixture(FIXTURE);
    let remote = run_ok(&[
        "query",
        "--source",
        &url,
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
