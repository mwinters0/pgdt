//! `pgdq parse` / `query` / `info` against `.xz` input, end to end, and
//! differential parity against the same content read plain
//! (`docs/design/architecture.md`, "Testing philosophy").
//!
//! `pgdump_query/tests/cache.rs` covers the same ground at the library level,
//! through `open_local`/`build_index` directly. This file is the only one that
//! drives the actual `pgdq` binary against a `.xz` source — its own argument
//! parsing, its cache round trip, and its text/JSON rendering, run the way a
//! user would run them.
//!
//! **Two fixtures, generated at test time, not committed** — deliberate, since
//! the input is 2,352 bytes and the compression cost is
//! negligible. Both are derived from the same hand-written dump
//! `pgdump_query/tests/cache.rs`'s own xz tests already use
//! (`tests/data/edge_cases.sql`), which is deliberately not
//! `pg_dump` output and carries no `CREATE TABLE` DDL — every column here
//! resolves `Utf8View`, so a byte-for-byte comparison between the plain and
//! compressed renderings is exactly what "differential parity" means: same
//! bytes in, same bytes out, only the source's container differs.

use std::path::{Path, PathBuf};
use std::process::Command;

mod common;
use common::{run, stderr_of, stdout_of};

/// The hand-written dump this phase's fixtures derive from —
/// `pgdump_query/tests/data/edge_cases.sql`, not the generated `fixtures/`
/// tree `common::fixture` addresses, which carries no equivalent file (xz
/// container shape does not depend on a PostgreSQL major, so nothing here
/// needs one).
fn edge_cases_sql() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pgdump_query/tests/data/edge_cases.sql")
}

/// Compress `edge_cases.sql` with `xz_args` into a fresh directory as `name`.
/// `xz` is not `mise`-pinned, so a missing binary fails loudly here rather
/// than the test silently skipping
/// (`docs/design/roadmap.md`, "A test may assume the tools `mise` pins").
fn xz_fixture(xz_args: &[&str], name: &str) -> (tempfile::TempDir, PathBuf) {
    let out = Command::new("xz")
        .args(xz_args)
        .arg("-c")
        .arg(edge_cases_sql())
        .output()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, &out.stdout).unwrap();
    (dir, path)
}

/// Seekable, multi-block: `--block-size=512` splits `edge_cases.sql` (2,352
/// bytes) into several blocks — the same size
/// `pgdump_query/tests/cache.rs`'s own `xz_compress` uses, confirmed there
/// (by an `is_seekable()` assertion, not just a comment) to actually force
/// the split on this exact fixture.
fn seekable_xz() -> (tempfile::TempDir, PathBuf) {
    xz_fixture(&["--block-size=512"], "edge_cases.sql.xz")
}

/// Non-seekable, single block: a bare `xz` invocation with no `-T`/
/// `--block-size` — one stream, one block, D2's diagnosed shape and the
/// backward-decode-from-zero path.
fn non_seekable_xz() -> (tempfile::TempDir, PathBuf) {
    xz_fixture(&[], "edge_cases_single_block.sql.xz")
}

/// A plain (uncompressed) copy of the same content, in its own directory so
/// its colocated `.dqcache` cannot collide with either xz fixture's.
fn plain() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edge_cases.sql");
    std::fs::copy(edge_cases_sql(), &path).unwrap();
    (dir, path)
}

/// `pgdq parse --source path`, asserted to succeed — building the colocated
/// cache a following `info`/`query` call reads.
fn parse(path: &Path) {
    let out = run(&["parse", "--source", path.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
}

/// `pgdq info --source path --json`, parsed.
fn info_json(path: &Path) -> serde_json::Value {
    let out = run(&["info", "--source", path.to_str().unwrap(), "--json"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    serde_json::from_str(&stdout_of(&out)).expect("--json emits JSON")
}

// ---------------------------------------------------------------------------
// `parse` + `info`
// ---------------------------------------------------------------------------

/// The seekable shape's index is byte-for-byte the plain file's: same spans,
/// same per-block resolution, same coverage — **and no diagnostics on either
/// side**, since a seekable `.xz` earns no D2 warning.
///
/// The `compression` object is the one field that legitimately differs, and
/// it is asserted rather than merely excused: it describes the *container*,
/// which is the whole of what these two files do not share.
#[test]
fn seekable_xz_parses_to_the_same_index_as_plain() {
    let (_pd, plain_path) = plain();
    let (_xd, xz_path) = seekable_xz();

    parse(&plain_path);
    parse(&xz_path);

    let mut plain_json = info_json(&plain_path);
    let mut xz_json = info_json(&xz_path);

    assert!(
        !has_non_seekable_warning(&plain_json),
        "sanity: a plain file never earns a compression warning"
    );
    assert!(!has_non_seekable_warning(&xz_json), "a seekable .xz source earns no D2 warning");

    assert_eq!(plain_json["compression"], serde_json::Value::Null, "a plain file has no container");
    let shape = xz_json["compression"].clone();
    assert_eq!(shape["container"], "xz");
    assert!(shape["blocks"].as_u64().unwrap() > 1, "--block-size=512 splits this fixture: {shape}");
    assert_eq!(shape["streams"], 1);
    assert!(shape["max_block_uncompressed"].as_u64().unwrap() > 0, "{shape}");

    plain_json.as_object_mut().unwrap().remove("compression");
    xz_json.as_object_mut().unwrap().remove("compression");
    assert_eq!(
        plain_json, xz_json,
        "a seekable .xz source parses to exactly the same index as its plain content, \
         diagnostics (e.g. TocCoverage's Info note, which this DDL-less dump always earns) \
         included"
    );
}

/// `info --detail` states the container's shape, and it is answered
/// from the **cache alone** — `--dqcache` with no `--source` at all, which is
/// the mode a budget decline deliberately cannot be reported in (it depends
/// on a run's budget, where this is a property of the file). The three
/// numbers are what a user would otherwise run `xz --list` for, and
/// `largest block` is the largest term of what `--parallel-memory` has to
/// clear — twice it, plus a chunk buffer and the decoder's own retention.
#[test]
fn info_detail_states_the_container_shape_from_the_cache_alone() {
    let (_xd, xz_path) = seekable_xz();
    parse(&xz_path);
    let cache = xz_path.with_extension("xz.dqcache");
    assert!(cache.exists(), "parse writes the cache beside the dump: {}", cache.display());

    for args in [
        vec!["info", "--source", xz_path.to_str().unwrap(), "--detail"],
        vec!["info", "--dqcache", cache.to_str().unwrap(), "--detail"],
    ] {
        let text = stdout_of(&run(&args));
        assert!(text.contains("compression: xz"), "{args:?}: {text}");
        assert!(text.contains("block(s) in 1 stream(s)"), "{args:?}: {text}");
        assert!(text.contains("largest block"), "{args:?}: {text}");
    }

    // Without `--detail` it is not printed at all, and a plain file has
    // nothing to print at any verbosity.
    let terse = stdout_of(&run(&["info", "--source", xz_path.to_str().unwrap()]));
    assert!(!terse.contains("compression:"), "{terse}");
    let (_pd, plain_path) = plain();
    parse(&plain_path);
    let plain_text =
        stdout_of(&run(&["info", "--source", plain_path.to_str().unwrap(), "--detail"]));
    assert!(!plain_text.contains("compression:"), "{plain_text}");
}

/// Whether an `info --json` document's `diagnostics` array carries D2's
/// warning.
fn has_non_seekable_warning(json: &serde_json::Value) -> bool {
    json["diagnostics"]
        .as_array()
        .expect("diagnostics is always an array")
        .iter()
        .any(|d| d["kind"].get("NonSeekableCompressedSource").is_some())
}

/// The non-seekable shape agrees with the plain file on everything **but**
/// the diagnostics: it alone carries D2's warning, which names the one block
/// a bare `xz` invocation produced.
#[test]
fn non_seekable_xz_parses_to_the_same_index_plus_a_warning() {
    let (_pd, plain_path) = plain();
    let (_xd, xz_path) = non_seekable_xz();

    parse(&plain_path);
    parse(&xz_path);

    let mut plain_json = info_json(&plain_path);
    let mut xz_json = info_json(&xz_path);

    assert!(!has_non_seekable_warning(&plain_json), "a plain file never earns this warning");
    assert!(has_non_seekable_warning(&xz_json), "the non-seekable source must warn");

    // Everything else — spans, resolution, coverage, and any diagnostic
    // both sides earn regardless of compression (this DDL-less dump's
    // `TocCoverage` note) — must agree exactly.
    plain_json.as_object_mut().unwrap().remove("diagnostics");
    xz_json.as_object_mut().unwrap().remove("diagnostics");
    // The container's shape differs too, and describes the container rather
    // than the index — a single-block file reports exactly that.
    assert_eq!(xz_json["compression"]["blocks"], 1);
    plain_json.as_object_mut().unwrap().remove("compression");
    xz_json.as_object_mut().unwrap().remove("compression");
    assert_eq!(plain_json, xz_json, "the warning is the only thing that may differ");

    // And the warning itself is D2's, in its rendered text form — the
    // human-readable side of the same diagnostic, naming the cause and the
    // remedy (`docs/design/architecture.md`, "The compressed source").
    let text = stdout_of(&run(&["info", "--source", xz_path.to_str().unwrap(), "--detail"]));
    assert!(text.contains("no seek structure"), "{text}");
    assert!(text.contains("xz -T0"), "{text}");
    assert!(text.contains("--block-size=<size>"), "{text}");
    let plain_text = stdout_of(&run(&["info", "--source", plain_path.to_str().unwrap()]));
    assert!(!plain_text.contains("no seek structure"), "{plain_text}");
}

// ---------------------------------------------------------------------------
// `query`
// ---------------------------------------------------------------------------

/// `public.widgets`, six rows over `(id, name, description, created_at)` —
/// this hand-written dump carries no DDL, so every column resolves
/// `Utf8View` and the rendering is untyped text either way.
fn query_widgets(source: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["query", "--source", source.to_str().unwrap(), "--table", "public.widgets"];
    args.extend_from_slice(extra);
    run(&args)
}

/// A plain `query` against all three sources prints identical rows — the
/// differential parity at the row-decoding level, run through `pgdq` end to
/// end rather than through the library.
///
/// It also exercises the mapping pass's own backward read for the
/// non-seekable fixture: `table_stream`'s replay re-reads the target block
/// after the mapping pass has already walked past it
/// (`docs/design/architecture.md`, "The compressed source" — one of the
/// library's two backward-reading call sites), which for a single-block `.xz`
/// file is the decode-from-zero path. A wrong answer here
/// would mean that path decodes the wrong bytes, not just that it is slow.
#[test]
fn query_widgets_agrees_across_plain_and_both_xz_shapes() {
    let (_pd, plain_path) = plain();
    let (_sd, seekable_path) = seekable_xz();
    let (_nd, non_seekable_path) = non_seekable_xz();
    for path in [&plain_path, &seekable_path, &non_seekable_path] {
        parse(path);
    }

    let plain_out = query_widgets(&plain_path, &[]);
    let seekable_out = query_widgets(&seekable_path, &[]);
    let non_seekable_out = query_widgets(&non_seekable_path, &[]);

    for out in [&plain_out, &seekable_out, &non_seekable_out] {
        assert!(out.status.success(), "{}", stderr_of(out));
        assert!(stderr_of(out).contains("6 row(s)"), "{}", stderr_of(out));
    }
    // Row 3's `description` carries a literal embedded newline (from the
    // dump's `\n` escape), so `.lines()` does not count rows — the "6
    // row(s)" stderr assertion above is what pins the row count; this is a
    // byte-for-byte comparison of the whole rendering.
    let plain_rows = stdout_of(&plain_out);
    assert_eq!(plain_rows, stdout_of(&seekable_out), "seekable .xz must render the same rows");
    assert_eq!(
        plain_rows,
        stdout_of(&non_seekable_out),
        "non-seekable .xz must render the same rows, via its decode-from-zero backward read"
    );
}

/// A typed filter over an xz source: forward decode only, unlike the replay
/// read above, and it names the same row on every shape.
#[test]
fn filtered_query_agrees_across_plain_and_both_xz_shapes() {
    let (_pd, plain_path) = plain();
    let (_sd, seekable_path) = seekable_xz();
    let (_nd, non_seekable_path) = non_seekable_xz();
    for path in [&plain_path, &seekable_path, &non_seekable_path] {
        parse(path);
    }

    let extra = ["--filter", "name=alpha", "--no-columns"];
    let plain_out = query_widgets(&plain_path, &extra);
    let seekable_out = query_widgets(&seekable_path, &extra);
    let non_seekable_out = query_widgets(&non_seekable_path, &extra);

    for out in [&plain_out, &seekable_out, &non_seekable_out] {
        assert!(out.status.success(), "{}", stderr_of(out));
        assert!(stderr_of(out).contains("1 row(s)"), "{}", stderr_of(out));
    }
    assert_eq!(stdout_of(&plain_out), stdout_of(&seekable_out));
    assert_eq!(stdout_of(&plain_out), stdout_of(&non_seekable_out));
}

// ---------------------------------------------------------------------------
// The persisted seek table
// ---------------------------------------------------------------------------

/// Replace `path`'s contents with plain (uncompressed) bytes of **exactly the
/// same length**, so that the cache beside it still passes its stored-size
/// identity check and the only thing that has changed is what the file's own
/// first bytes say it is. That is the one arrangement a user can reach that
/// puts a compression claim and a file's content into contradiction.
fn overwrite_with_plain_bytes_of_the_same_length(path: &Path) {
    let len = std::fs::metadata(path).unwrap().len() as usize;
    let mut bytes = b"-- not compressed any more\n".repeat(len.div_ceil(27));
    bytes.truncate(len);
    std::fs::write(path, &bytes).unwrap();
    assert_eq!(std::fs::metadata(path).unwrap().len() as usize, len);
}

/// A cache whose compression claim the file contradicts is refused by all
/// three commands, and refused **having read nothing**: no stream-footer walk
/// is spent reaching an error that was always coming
/// (`docs/design/architecture.md`, "The compressed source").
///
/// `parse` used to take the other branch, deleting the cache and rescanning,
/// because it was about to overwrite that path anyway. It refuses with the
/// other two now: the deletion and the overwrite are the same act one step
/// apart, and the library replaces neither on its own
/// (`docs/design/architecture.md`, "The cache").
///
/// The sentence is this condition's own. It borrowed `Unreadable`'s —
/// "… is not a pgdq cache — check the path, or run `pgdq parse`" — until the
/// refusal made that advice something `parse` cannot take, being the command
/// that just refused; and what sits at that path *is* a pgdq cache, for some
/// other file.
#[test]
fn every_command_refuses_a_cache_that_does_not_describe_the_file() {
    let (_dir, path) = seekable_xz();
    parse(&path);
    let cache = PathBuf::from(format!("{}.dqcache", path.display()));
    let before = std::fs::read(&cache).unwrap();
    overwrite_with_plain_bytes_of_the_same_length(&path);

    for args in [
        vec!["info", "--source", path.to_str().unwrap()],
        vec!["query", "--source", path.to_str().unwrap(), "--table", "widgets"],
        vec!["parse", "--source", path.to_str().unwrap()],
    ] {
        let out = run(&args);
        assert!(!out.status.success(), "{}: {}", args[0], stdout_of(&out));
        let err = stderr_of(&out);
        assert!(err.contains("was written for another file"), "{}: {err}", args[0]);
        assert!(err.contains("remove it, or name a different cache path"), "{}: {err}", args[0]);
        assert!(
            !err.contains("is not a pgdq cache"),
            "{}: a cache for another file is still a pgdq cache: {err}",
            args[0]
        );
    }

    assert_eq!(std::fs::read(&cache).unwrap(), before, "the refused cache is untouched");
}

/// The way out is the user's, not the tool's: remove the cache that does not
/// describe this file and `parse` scans it as a cold file would. There is no
/// override flag, which is what keeps the refusal from being set once in a
/// script and never reconsidered (`docs/design/architecture.md`, "The cache").
#[test]
fn parse_scans_once_the_refused_cache_is_removed() {
    let (_dir, path) = seekable_xz();
    parse(&path);
    let cache = PathBuf::from(format!("{}.dqcache", path.display()));
    let before = std::fs::read(&cache).unwrap();
    overwrite_with_plain_bytes_of_the_same_length(&path);
    std::fs::remove_file(&cache).unwrap();

    let out = run(&["parse", "--source", path.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert_ne!(
        std::fs::read(&cache).unwrap(),
        before,
        "the cache describes the file that is there"
    );

    let info = run(&["info", "--source", path.to_str().unwrap()]);
    assert!(info.status.success(), "{}", stderr_of(&info));
}

/// **The sibling refusal is free too.** A cache recorded against a file of
/// another stored size is settled by the cache path and a `stat`, so all three
/// commands report it without opening the source — and an `.xz` source is what
/// makes that worth doing, its open being a walk of every stream footer in the
/// file (`docs/design/architecture.md`, "The compressed source").
///
/// **Proven by a file the walk itself would fail on.** Truncating the fixture
/// does both things at once: it changes the stored size the cache records, and
/// it removes the stream footer the walk reads. So a command that opened the
/// source first would report `xz error: …` — asserted below, once the cache is
/// out of the way — and one that reads the cache first reports the mismatch.
/// The two are told apart in the output, which no timing assertion could do.
///
/// Each command keeps the sentence it had: the library's own error for the two
/// that scan, and for `info` the one that names the two ways out ahead of
/// `pgdq parse` (`docs/design/architecture.md`, "The CLI's two refusals are
/// worded as one").
#[test]
fn a_cache_recorded_against_another_file_is_refused_without_walking_this_one() {
    let (_dir, path) = seekable_xz();
    parse(&path);
    let cache = PathBuf::from(format!("{}.dqcache", path.display()));
    let before = std::fs::read(&cache).unwrap();

    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() - 32]).unwrap();

    for args in [
        vec!["info", "--source", path.to_str().unwrap()],
        vec!["query", "--source", path.to_str().unwrap(), "--table", "widgets"],
        vec!["parse", "--source", path.to_str().unwrap()],
    ] {
        let out = run(&args);
        assert!(!out.status.success(), "{}: {}", args[0], stdout_of(&out));
        let err = stderr_of(&out);
        assert!(err.contains("remove it, or name a different cache path"), "{}: {err}", args[0]);
        assert!(
            !err.contains("xz error:"),
            "{}: the file must not be opened to reach this refusal: {err}",
            args[0]
        );
    }
    assert!(
        stderr_of(&run(&["info", "--source", path.to_str().unwrap()]))
            .contains("has changed since it was parsed"),
        "`info` keeps its own sentence for this condition"
    );
    assert!(
        stderr_of(&run(&["parse", "--source", path.to_str().unwrap()]))
            .contains("was written for a source of"),
        "the two scanning commands keep the library's"
    );

    assert_eq!(std::fs::read(&cache).unwrap(), before, "the refused cache is untouched");

    // The discriminator is real: with no cache to settle it, opening this file
    // is what happens, and the walk fails.
    std::fs::remove_file(&cache).unwrap();
    let out = run(&["parse", "--source", path.to_str().unwrap()]);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    assert!(stderr_of(&out).contains("xz error:"), "{}", stderr_of(&out));
}
