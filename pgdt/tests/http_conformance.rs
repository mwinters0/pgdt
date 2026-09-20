//! The remote source read against a **real** static origin rather than against
//! `common::oracle`.
//!
//! The oracle is ours on both sides of the wire, so the one fault it cannot
//! surface is a misreading of HTTP that our client and our server share: an
//! off-by-one in a `Range`, a `Content-Range` we spell wrong and parse back
//! the same way, a validator no shipped server would accept. That is the one
//! risk a bespoke oracle cannot retire by construction, and this file is what
//! answers it.
//!
//! **Opt-in, and empty by default.** Nothing below runs unless
//! `PGDT_HTTP_CONFORMANCE_URL` names an object, so `cargo test --workspace`
//! passes in every checkout and in CI with no server standing anywhere. That
//! is the opposite of `common::require_uv`'s rule and deliberately so: `uv` is
//! pinned by `mise.toml`, so its absence is a machine that is not set up,
//! while a static origin is something a person raises for one sitting and
//! takes down again. What is **not** skipped is anything past a variable
//! being set — an unreachable URL, an object that is not this checkout's
//! fixture, a server that ignores `Range` all fail here rather than pass
//! quietly.
//!
//! **Two variables, because the compressed object cannot be the same one.**
//! `PGDT_HTTP_CONFORMANCE_XZ_URL` names an `.xz` object whose *plaintext* is
//! that same fixture, and it is separate for two reasons. `xz` output is not
//! reproducible across versions or build options, so there are no bytes this
//! checkout could demand the object equal — what is compared is the plaintext
//! our reader hands back, which is the only invariant the person placing the
//! object can be held to. And the two turn on independently: raising a static
//! origin is cheap, placing a second object on it is a step someone may not
//! have taken.
//!
//! How to raise one and what it must serve is [`CONTRIBUTING.md`](../../CONTRIBUTING.md),
//! "Building and testing".

mod common;

use std::sync::Arc;

use common::{fixture, run_ok, strip_cache_line};
use pgdump_query::{ByteRangeSource, KnownCompression, Origin, Recognized, open};

/// The variable that turns this file's plain half on, by naming the object to
/// read.
const URL_VAR: &str = "PGDT_HTTP_CONFORMANCE_URL";

/// The variable that turns the compressed half on, by naming an `.xz` object
/// whose plaintext is [`FIXTURE`].
const XZ_URL_VAR: &str = "PGDT_HTTP_CONFORMANCE_XZ_URL";

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
/// `pgdt` would read as a local path is refused by name, since a `file:` URL
/// exercises none of what this file exists to test.
fn conformance_url_from(var: &str) -> Option<String> {
    let url = std::env::var(var).ok()?;
    let url = url.trim().to_string();
    if url.is_empty() {
        return None;
    }
    let origin = Origin::resolve(&url)
        .unwrap_or_else(|e| panic!("{var} is not a source `pgdt` can read: {e}"));
    assert!(
        origin.local_path().is_none(),
        "{var} names {url}, which `pgdt` reads as a local path — a conformance run needs an \
         origin reached over HTTP, since the transport is the subject"
    );
    Some(url)
}

fn conformance_url() -> Option<String> {
    conformance_url_from(URL_VAR)
}

/// The compressed object's URL, or `None` when nobody placed one.
fn conformance_xz_url() -> Option<String> {
    conformance_url_from(XZ_URL_VAR)
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
    dir.path().join("conformance.dtcache").to_str().unwrap().to_string()
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

/// `pgdt parse` over the checked-in fixture, into a cache of its own, with the
/// one line that names that cache removed — the listing a remote run of any
/// container must reproduce, since the two differ in where the bytes came
/// from and in nothing else.
fn local_parse() -> String {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("local.dtcache");
    let dump = fixture(FIXTURE);
    let out = run_ok(&[
        "parse",
        "--source",
        dump.to_str().unwrap(),
        "--dtcache",
        cache.to_str().unwrap(),
    ]);
    strip_cache_line(&out)
}

/// `pgdt query public.widgets` over the checked-in fixture, cacheless.
fn local_query() -> String {
    let dump = fixture(FIXTURE);
    run_ok(&[
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.widgets",
        "--dtcache",
        "none",
        "--no-columns",
    ])
}

#[test]
fn parse_over_a_real_origin_finds_what_a_local_parse_finds() {
    let Some(url) = conformance_url() else { return };
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(&dir);
    let remote = run_ok(&["parse", "--source", &url, "--dtcache", &cache]);
    assert_eq!(strip_cache_line(&remote), local_parse());

    // And the cache that scan left reads back against the same URL, so the
    // origin recorded in it is one the object still matches.
    let report = run_ok(&["info", "--source", &url, "--dtcache", &cache]);
    assert!(report.contains("public.widgets"), "{report}");
}

#[test]
fn query_over_a_real_origin_streams_the_same_rows_a_local_query_streams() {
    let Some(url) = conformance_url() else { return };
    let remote = run_ok(&[
        "query",
        "--source",
        &url,
        "--table",
        "public.widgets",
        "--dtcache",
        "none",
        "--no-columns",
    ]);
    assert_eq!(remote, local_query());
}

// ---------------------------------------------------------------------------
// The compressed object, whose footer walk is the request cadence furthest
// from anything the oracle can stand in for
// ---------------------------------------------------------------------------
//
// Everything above reads a plain dump: a probe and then chunks, forward, each
// large. A fetched `.xz` opens by walking stream footers backward — a chain of
// tiny ranged GETs whose every position is read out of the bytes of the one
// before it, ending at the object's last twelve bytes, at least one per stream
// (`pgdump_query/src/io.rs`, `walk_seek_table`). The oracle answers one
// request per connection and says `Connection: close` on every response
// (`tests/common/oracle.rs`), so it has never seen those requests arrive down
// a connection it is holding open, which is what a real origin does with them.

/// The compressed object, opened as a run opens it — which for
/// [`KnownCompression::Unknown`] is where the footer walk is paid.
///
/// What the object must be is checked here rather than in each test, so that
/// an object placed wrong is one message rather than five, and so that no test
/// below computes an offset from a length that is not the fixture's.
///
/// **It is the right dump**, by the plaintext length the walked index accounts
/// for — the whole bytes are compared in the first test below, this being the
/// cheap half of that and the half every other test depends on. **It really is
/// `.xz`**, read off what recognition made of the object rather than off its
/// name: a source carrying no seek table is one whose first bytes were not
/// `.xz`'s magic (`docs/design/decisions.md`, "D14"), whatever the URL ends
/// in. And it is **more than one stream of more than one block**, or the walk
/// this half exists to exercise is a handful of requests over a file with
/// nothing to seek in. All three are properties of the object rather than a
/// recipe, so a person who compressed it differently but kept the shape
/// passes.
async fn xz_source_of(url: &str) -> Arc<dyn ByteRangeSource> {
    let source = source_of(url).await;
    let Some(table) = source.seek_table() else {
        panic!(
            "{XZ_URL_VAR} names {url}, whose bytes are not `.xz` — this half of the file reads a \
             compressed object, and the plain one is {URL_VAR}"
        )
    };
    assert!(
        table.stream_count() > 1 && table.block_count() > table.stream_count(),
        "{url} is {} stream(s) of {} block(s): the footer walk costs a round trip per stream and \
         a seek costs a block, so an object with one of either exercises neither. \
         CONTRIBUTING.md, \"Building and testing\" has a recipe.",
        table.stream_count(),
        table.block_count(),
    );
    let declared = source.size().await.unwrap();
    assert_eq!(
        declared,
        dump_bytes().len() as u64,
        "{url} holds {declared} plaintext byte(s) against fixtures/{FIXTURE}'s {}",
        dump_bytes().len(),
    );
    source
}

#[tokio::test]
async fn the_compressed_object_named_decodes_to_the_fixture_this_checkout_holds() {
    let Some(url) = conformance_xz_url() else { return };
    let body = dump_bytes();
    let source = xz_source_of(&url).await;

    // **The plaintext, not the bytes.** `xz` output is not reproducible across
    // versions or build options, so this checkout has no compressed bytes to
    // demand the object equal; what it can demand is that the plaintext our
    // reader hands back off a real origin is the fixture, which is the whole
    // of what the object was placed to be.
    let whole = source.read_range(0, body.len()).await.expect("the object decodes whole");
    assert_eq!(&whole[..], &body[..], "{url} does not hold fixtures/{FIXTURE}");
}

#[tokio::test]
async fn the_footer_walk_over_a_real_origin_builds_the_index_the_object_declares() {
    let Some(url) = conformance_xz_url() else { return };
    let body = dump_bytes();
    let source = xz_source_of(&url).await;
    let table = source.seek_table().expect("`xz_source_of` already required one");

    // The walk read no payload: every number below came out of footers,
    // indexes and headers fetched a few bytes at a time from a server we did
    // not write. A `Range` we spell in a way only our own server accepts
    // cannot produce an index that adds up.
    assert_eq!(
        table.compressed_file_size,
        source.stored_size().await.unwrap(),
        "the index the walk built describes another object's length",
    );
    assert_eq!(
        table.uncompressed_size(),
        body.len() as u64,
        "the plaintext the index accounts for"
    );
    let accounted: u64 = table.blocks.iter().map(|b| b.uncompressed_size).sum();
    assert_eq!(accounted, body.len() as u64, "every block's share of it");
    assert!(table.is_seekable(), "{table:?}");
}

#[tokio::test]
async fn a_ranged_read_of_the_compressed_object_answers_the_plaintext_at_the_offset() {
    let Some(url) = conformance_xz_url() else { return };
    let body = dump_bytes();
    let source = xz_source_of(&url).await;
    let table = source.seek_table().expect("`xz_source_of` already required one");

    // Every block's first and last plaintext byte, which is where a window
    // fetched one byte short or one byte long shows: the block's compressed
    // extent is a `Range` computed from the index, so an off-by-one there is
    // a decode that fails or a byte that is wrong, never a short answer.
    let mut spans: Vec<(u64, usize)> = Vec::new();
    for block in &table.blocks {
        let range = block.uncompressed_range();
        spans.push((range.start, 1));
        spans.push((range.end - 1, 1));
        spans.push((range.start, (range.end - range.start) as usize));
    }
    // And a read straddling every boundary, which no single block answers.
    for block in table.blocks.iter().skip(1) {
        let at = block.uncompressed_offset.saturating_sub(4);
        spans.push((at, 8.min((body.len() as u64 - at) as usize)));
    }
    for (offset, len) in spans {
        let got = source
            .read_range(offset, len)
            .await
            .unwrap_or_else(|e| panic!("{len} byte(s) at {offset} did not read: {e}"));
        let want = &body[offset as usize..offset as usize + len];
        assert_eq!(&got[..], want, "{len} byte(s) at {offset}");
    }
}

#[test]
fn parse_over_a_real_compressed_origin_finds_what_a_local_parse_finds() {
    let Some(url) = conformance_xz_url() else { return };
    let dir = tempfile::tempdir().unwrap();
    let cache = cache_in(&dir);
    let remote = run_ok(&["parse", "--source", &url, "--dtcache", &cache]);
    // The listing is the index, and the index is of the plaintext — so the
    // container is not allowed to show in it at all.
    assert_eq!(strip_cache_line(&remote), local_parse());

    // Read back through the cache the scan left, which holds the seek table:
    // this `info` opens the object with the walk already paid, which is the
    // other of `open_remote`'s two `.xz` arms and the one a second run takes.
    let report = run_ok(&["info", "--source", &url, "--dtcache", &cache]);
    assert!(report.contains("public.widgets"), "{report}");
}

#[test]
fn query_over_a_real_compressed_origin_streams_the_same_rows_a_local_query_streams() {
    let Some(url) = conformance_xz_url() else { return };
    // `--dtcache none` pays the walk again and decodes with nothing kept, so
    // the rows come out of blocks fetched during this run.
    let remote = run_ok(&[
        "query",
        "--source",
        &url,
        "--table",
        "public.widgets",
        "--dtcache",
        "none",
        "--no-columns",
    ]);
    assert_eq!(remote, local_query());
}
