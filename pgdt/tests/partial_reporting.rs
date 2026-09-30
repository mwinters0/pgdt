//! `pgdt parse` / `pgdt info` / `pgdt query` end to end: **`info`
//! reads, `parse` scans** (`docs/design/decisions.md`, "D61").
//!
//! These drive the real binary (`CARGO_BIN_EXE_pgdt`) rather than the library,
//! because what they pin is the *command's* contract: which invocations fail,
//! what they say, and what the two renderings of one index agree about. A
//! library test cannot observe an exit status or a message.
//!
//! **Partial caches are built by hand, by truncating a complete index at a
//! block boundary.** That is the state an interrupted `parse` leaves — every
//! block up to a `CopyEnd` watermark banked and an `Unscanned` tail after it —
//! and constructing it directly is what makes these tests deterministic.
//! `pgdump_query`'s own `tests/map_file.rs` covers that a real interruption
//! produces this shape, and that its metadata covers every database segment
//! the scan finished.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pgdump_query::cache::{CACHE_FORMAT_VERSION, OVERWRITE_WAYS_OUT};
use pgdump_query::{
    DumpIndex, DumpMetadata, LocalFileSource, ScanOptions, Span, SpanBody, build_index, cache,
};

mod common;
use common::{fixture, pgdt, run, stderr_of, stdout_of};

/// Two copies of `edge_cases/create.sql` concatenated, the second's database
/// renamed — a real `\connect`-delimited multi-database dump, the same
/// construction `pgdump_query/tests/common/mod.rs`'s `multidb_fixture` uses.
fn sandboxed_multidb() -> (tempfile::TempDir, PathBuf) {
    let content = std::fs::read_to_string(fixture("16/edge_cases/create.sql")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("multidb.sql");
    let renamed = content.replace("pgdt_fixture", "pgdt_fixture_2");
    std::fs::write(&dump, format!("{content}{renamed}")).unwrap();
    (dir, dump)
}

/// Write a cache holding everything the map held up to `frontier`, an
/// `Unscanned` tail for the rest, and the first `keep_databases` databases'
/// DDL.
///
/// `keep_databases` is what lets a test put a block in the map whose database
/// has no metadata behind it. A real scan no longer produces that pairing —
/// `map_forward` states a database's DDL at its first `COPY` block, before any
/// of its blocks can be banked — so this builds the state directly, to pin
/// what `resolve_columns` answers when it is handed one.
async fn write_truncated_cache(
    dump: &Path,
    frontier: u64,
    keep_databases: usize,
) -> (PathBuf, DumpIndex) {
    let source = LocalFileSource::open(dump).unwrap();
    let size = std::fs::metadata(dump).unwrap().len();
    let full = build_index(&source, &ScanOptions::default()).await.unwrap();

    // Clamp the last kept span to the frontier, the way `Builder::snapshot`
    // closes spans at a `CopyEnd` watermark. A complete scan's spans are
    // greedy — a block's span runs on to wherever the next one starts — so
    // filtering alone would drop the very block the scan had just banked.
    let mut spans: Vec<Span> = full.spans.iter().filter(|s| s.start < frontier).cloned().collect();
    if let Some(last) = spans.last_mut() {
        last.end = frontier;
    }
    spans.push(Span {
        start: frontier,
        end: size,
        database: None,
        text: None,
        toc: None,
        toc_owned: false,
        body: SpanBody::Unscanned,
    });
    let mut metadata = full.metadata.clone().unwrap();
    metadata.databases.truncate(keep_databases);

    let index = DumpIndex {
        spans,
        scanned_through: frontier,
        metadata: Some(DumpMetadata { databases: metadata.databases }),
        roles: full.roles.clone(),
        tablespaces: full.tablespaces.clone(),
        diagnostics: Vec::new(),
    };
    let path = cache::colocated_path(dump);
    cache::save(&path, &source, &index).await.unwrap();
    (path, index)
}

/// The `end_offset` of the `n`-th `COPY` block, as a truncation point that is
/// a real resumable watermark rather than an arbitrary byte.
async fn block_frontier(dump: &Path, n: usize) -> u64 {
    let source = LocalFileSource::open(dump).unwrap();
    let index = build_index(&source, &ScanOptions::default()).await.unwrap();
    index.blocks().nth(n).expect("the fixture has this many blocks").end_offset
}

// ---------------------------------------------------------------------------
// `info` never scans
// ---------------------------------------------------------------------------

/// **The two messages must stay distinguishable.** Both end in `pgdt parse`,
/// and both have to be distinguishable: "you have never parsed this file"
/// sends a reader to run it, "your file changed since you parsed it" sends
/// them to ask what changed. Both are asserted rather than one standing in
/// for the other.
#[tokio::test]
async fn info_with_no_cache_names_parse_and_exits_non_zero() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let out = run(&["info", "--source", dump.to_str().unwrap()]);

    assert!(!out.status.success(), "info must not succeed without a cache");
    let err = stderr_of(&out);
    assert!(err.contains("no cache at"), "{err}");
    assert!(err.contains("pgdt parse"), "{err}");
    assert!(stdout_of(&out).is_empty(), "nothing is reported when nothing can be");
}

#[tokio::test]
async fn info_against_a_changed_file_says_the_file_changed() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());

    // Same file, one byte longer: every offset in the cache could now be
    // wrong, which is a different fact from "there is no cache".
    let mut bytes = std::fs::read(&dump).unwrap();
    bytes.push(b'\n');
    std::fs::write(&dump, &bytes).unwrap();

    let out = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(!out.status.success());
    let err = stderr_of(&out);
    assert!(err.contains("has changed since it was parsed"), "{err}");
    assert!(err.contains("pgdt parse"), "{err}");
    assert!(!err.contains("no cache at"), "the two messages must not collapse: {err}");
}

/// **`parse` refuses that same cache rather than scanning over it and
/// overwriting the file within its first throttled save** — so aiming
/// `--dtcache` at another file's cache leaves that file's own valid index
/// untouched, with the refusal said aloud
/// (`docs/design/decisions.md`, "D20"). The byte comparison is the
/// half that would fail silently.
#[tokio::test]
async fn parse_refuses_a_cache_that_records_another_source_and_leaves_it_alone() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());
    let cache_path = cache::colocated_path(&dump);
    let before = std::fs::read(&cache_path).unwrap();

    let mut bytes = std::fs::read(&dump).unwrap();
    bytes.push(b'\n');
    std::fs::write(&dump, &bytes).unwrap();

    let out = run(&["parse", "--source", dump.to_str().unwrap()]);
    assert!(!out.status.success(), "{}", stdout_of(&out));
    let err = stderr_of(&out);
    assert!(err.contains(cache_path.to_str().unwrap()), "{err}");
    assert_eq!(std::fs::read(&cache_path).unwrap(), before, "the refused cache is untouched");

    // The way out is the caller's: remove it, and the same command scans.
    std::fs::remove_file(&cache_path).unwrap();
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());
    assert_ne!(std::fs::read(&cache_path).unwrap(), before);
}

/// **Or the caller says it may be replaced**: under
/// `--overwrite-unusable-cache` both commands that scan start cold over a
/// cache written for another file and replace it with one of this file,
/// which `info` then reads (`docs/design/decisions.md`, "D20").
#[tokio::test]
async fn the_scanning_commands_replace_an_unusable_cache_when_told_to() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let source = dump.to_str().unwrap();
    for command in [
        vec!["parse", "--source", source, "--overwrite-unusable-cache"],
        vec![
            "query",
            "--source",
            source,
            "--table",
            "t_numeric",
            "--schema-mode",
            "strings",
            "--overwrite-unusable-cache",
        ],
    ] {
        assert!(run(&["parse", "--source", source]).status.success());
        let mut bytes = std::fs::read(&dump).unwrap();
        bytes.push(b'\n');
        std::fs::write(&dump, &bytes).unwrap();
        assert!(!run(&["info", "--source", source]).status.success());

        let out = run(&command);
        assert!(out.status.success(), "{}: {}", command[0], stderr_of(&out));
        let out = run(&["info", "--source", source]);
        assert!(out.status.success(), "{}: {}", command[0], stderr_of(&out));
    }
}

/// **The ways out are worded once, and every refusal says all three.** A cache
/// that describes another file reaches the user from two places — the library
/// error `parse` and `query` surface, and the sentence `info` prints — for the
/// size mismatch here and for a contradicted compression claim in
/// `tests/xz_source.rs`, and each names removing it, naming another path and
/// `--overwrite-unusable-cache` (`docs/design/decisions.md`, "D20"). A refusal
/// that named fewer would read as a tool with less recourse than it has; one
/// that named a way out the others do not is the drift this test exists to
/// catch, the wording living in two crates.
///
/// `info`'s is the arm that must name the ways out **and** `pgdt parse`, in
/// that order: `parse` refuses this same condition, so a reader sent straight
/// to it meets a second refusal.
#[tokio::test]
async fn refusals_name_both_ways_out() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());

    let mut bytes = std::fs::read(&dump).unwrap();
    bytes.push(b'\n');
    std::fs::write(&dump, &bytes).unwrap();

    let source = dump.to_str().unwrap();
    for args in [
        vec!["parse", "--source", source],
        vec!["info", "--source", source],
        vec!["query", "--source", source, "--table", "t_numeric"],
    ] {
        let out = run(&args);
        assert!(!out.status.success(), "{}: {}", args[0], stdout_of(&out));
        let err = stderr_of(&out);
        assert!(err.contains(OVERWRITE_WAYS_OUT), "{}: {err}", args[0]);
    }

    let err = stderr_of(&run(&["info", "--source", source]));
    let ways_out = err.find(OVERWRITE_WAYS_OUT).unwrap();
    let parse_hint = err.find("`pgdt parse --source").expect("the remedy still ends at `parse`");
    assert!(ways_out < parse_hint, "the ways out come before the command they enable: {err}");
}

/// Foreign bytes and a cache from another build are told apart too — the path
/// is wrong in the first case and right in the second, which is different
/// advice — and `parse` refuses to write over the foreign bytes, which no flag
/// changes.
#[tokio::test]
async fn info_distinguishes_foreign_bytes_from_another_builds_cache() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let cache_path = cache::colocated_path(&dump);
    std::fs::write(&cache_path, b"not a cache at all").unwrap();

    let out = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("is not a pgdt cache"), "{}", stderr_of(&out));
    for flags in [&[][..], &["--overwrite-unusable-cache"]] {
        let out = run(&[&["parse", "--source", dump.to_str().unwrap()], flags].concat());
        assert!(!out.status.success(), "{flags:?}: {}", stdout_of(&out));
        assert!(stderr_of(&out).contains("is not a pgdt cache"), "{}", stderr_of(&out));
        assert_eq!(std::fs::read(&cache_path).unwrap(), b"not a cache at all");
    }

    std::fs::remove_file(&cache_path).unwrap();
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());
    let mut bytes = std::fs::read(&cache_path).unwrap();
    bytes[8..12].copy_from_slice(&(CACHE_FORMAT_VERSION + 1).to_le_bytes());
    std::fs::write(&cache_path, &bytes).unwrap();

    let out = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("written by a different pgdt build"), "{}", stderr_of(&out));
}

/// `--dtcache none` means "ignore the cache", which leaves
/// `info` with nothing at all to answer from. Rejected up front rather than
/// silently reporting an empty index — and the message names the way out,
/// since the usual reason to reach for `none` is a read-only directory beside
/// the dump, which `parse --dtcache <path>` answers.
#[tokio::test]
async fn info_rejects_a_disabled_cache_and_names_the_remedy() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let out = run(&["info", "--source", dump.to_str().unwrap(), "--dtcache", "none"]);
    assert!(!out.status.success());
    let stderr = stderr_of(&out);
    assert!(stderr.contains("never scans"), "{stderr}");
    assert!(stderr.contains("pgdt parse --source"), "{stderr}");
    assert!(stderr.contains("--dtcache <path>"), "{stderr}");
}

/// `info` reads a cache and stops there — it does not extend, rewrite, or
/// touch it. Proven by the bytes on disk, not by the absence of a delay.
#[tokio::test]
async fn info_leaves_a_partial_cache_exactly_as_it_found_it() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let frontier = block_frontier(&dump, 2).await;
    let (cache_path, _) = write_truncated_cache(&dump, frontier, 1).await;
    let before = std::fs::read(&cache_path).unwrap();

    let out = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));

    assert_eq!(before, std::fs::read(&cache_path).unwrap(), "info never writes the cache");
}

// ---------------------------------------------------------------------------
// Coverage is stated once, at the top
// ---------------------------------------------------------------------------

/// A partial cache lists **exactly** the blocks the map holds, with the
/// coverage line above them and nothing below it qualified. The absence of
/// per-record qualification is the assertion that matters: a partial index
/// lacks records, not confidence, and a per-block "(partial)" marker would
/// suggest a variation that does not exist.
#[tokio::test]
async fn a_partial_cache_lists_exactly_the_blocks_it_holds() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let frontier = block_frontier(&dump, 2).await;
    let (_cache_path, truncated) = write_truncated_cache(&dump, frontier, 1).await;
    let expected: Vec<String> = truncated.blocks().map(|b| b.header.qualified_name()).collect();
    assert_eq!(expected.len(), 3, "the truncation kept the first three blocks");

    let out = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);

    let size = std::fs::metadata(&dump).unwrap().len();
    let percent = (frontier * 100) / size;
    assert_eq!(
        text.lines().next().unwrap(),
        format!("Scan completion: {percent}% ({frontier} bytes)"),
        "the coverage line is the first thing printed"
    );
    assert!(percent < 100, "sanity: this cache really is partial");

    let listed: Vec<String> = text
        .lines()
        .filter(|l| l.ends_with(" rows)"))
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect();
    assert_eq!(listed, expected);
    assert_eq!(text.matches("Scan completion").count(), 1, "stated once, at the top");
}

/// A complete cache says so on the same line, in the same place. The line is
/// unconditional: "how much of this file do we know" is not a question only a
/// partial answer raises.
#[tokio::test]
async fn a_complete_cache_reports_full_coverage() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());
    let size = std::fs::metadata(&dump).unwrap().len();

    let out = run(&["info", "--source", dump.to_str().unwrap()]);
    let text = stdout_of(&out);
    assert_eq!(text.lines().next().unwrap(), format!("Scan completion: 100% ({size} bytes)"));
}

/// Cache-only mode reports a partial cache too. It cannot extend one — there
/// is no dump file to extend it from — but "as far as the scan got" is still
/// an answer.
#[tokio::test]
async fn cache_only_mode_reports_a_partial_cache() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let frontier = block_frontier(&dump, 2).await;
    let (cache_path, _) = write_truncated_cache(&dump, frontier, 1).await;

    let out = run(&["info", "--dtcache", cache_path.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(text.starts_with("Scan completion: "), "{text}");
    assert!(
        text.contains("answering from a cache with no source dump file"),
        "the unverified-historical warning still rides along: {text}"
    );
}

// ---------------------------------------------------------------------------
// `parse` resumes
// ---------------------------------------------------------------------------

/// A resumed `parse` says where it picked up, then prints the whole index —
/// the listing describes the file's state after the run, not the run's diff,
/// so the one line about the invocation goes above it.
#[tokio::test]
async fn parse_resumes_from_a_partial_cache_and_says_so() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let frontier = block_frontier(&dump, 2).await;
    write_truncated_cache(&dump, frontier, 1).await;

    let out = run(&["parse", "--source", dump.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert_eq!(
        text.lines().next().unwrap(),
        format!(
            "resumed a previous scan at byte {frontier} of {}",
            std::fs::metadata(&dump).unwrap().len()
        )
    );

    // And the file is fully known afterwards.
    let info = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(stdout_of(&info).starts_with("Scan completion: 100%"), "{}", stdout_of(&info));
}

/// A first `parse` has nothing to resume from and says nothing about it.
#[tokio::test]
async fn a_cold_parse_prints_no_resume_line() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let out = run(&["parse", "--source", dump.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let text = stdout_of(&out);
    assert!(!text.contains("resumed"), "{text}");
    assert!(!text.contains("nothing to scan"), "{text}");
}

/// `--preamble-only` hangs off `parse`: it is a scan extent, and `info` has
/// none. It leaves an ordinary partial cache, which `info` then reads like
/// any other.
#[tokio::test]
async fn preamble_only_moved_to_parse_and_leaves_a_cache_info_reads() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(pgdt().args(["info", "--preamble-only"]).output().unwrap().status.code() != Some(0));

    let out = run(&["parse", "--source", dump.to_str().unwrap(), "--preamble-only"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    assert!(stdout_of(&out).contains("user-defined types:"), "{}", stdout_of(&out));

    let info = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(info.status.success(), "{}", stderr_of(&info));
    let text = stdout_of(&info);
    assert!(text.starts_with("Scan completion: "), "{text}");
    assert!(!text.starts_with("Scan completion: 100%"), "a preamble scan stops early: {text}");
    assert!(text.contains("no COPY blocks found"), "{text}");
}

// ---------------------------------------------------------------------------
// Machine-readable resolution
// ---------------------------------------------------------------------------

/// The indented lines `info --detail` prints under each block, grouped by
/// block in file order — minus the four byte-offset lines and the column
/// summary, which are not per-column resolution.
///
/// An enum's `labels:` continuation line is kept, with its extra indent
/// intact, because it is per-column resolution too — the export states those
/// labels once per *type* rather than once per column, so it is pinned by
/// `an_enum_columns_declared_labels_are_listed_beneath_it` instead of by the
/// cross-check below.
///
/// The type listing `--detail` prints under `user-defined types:` is indented
/// the same way and is not per-column resolution, but it sits in the metadata
/// header above every block line, so no block is open to collect it; the
/// `statistics:` section below the last block closes it.
fn detail_column_lines(text: &str) -> Vec<Vec<String>> {
    const NOT_A_COLUMN: [&str; 5] =
        ["columns:", "header offset:", "data offset:", "terminator:", "end offset:"];
    let mut blocks: Vec<Vec<String>> = Vec::new();
    for line in text.lines().take_while(|l| !l.starts_with("statistics:")) {
        if let Some(rest) = line.strip_prefix("    ") {
            if NOT_A_COLUMN.iter().any(|k| rest.starts_with(k)) {
                continue;
            }
            if let Some(current) = blocks.last_mut() {
                current.push(rest.to_string());
            }
        } else if line.ends_with(" rows)") {
            blocks.push(Vec::new());
        }
    }
    blocks
}

/// **One resolution pass, two renderings.** The export must not become a
/// second implementation of what the listing says, so every per-column line
/// `--detail` prints is checked against the JSON record for the same column
/// of the same block: the Arrow type verbatim, and the outcome through the
/// token, whose words are the sentence's own opening.
#[tokio::test]
async fn the_json_export_and_the_detail_listing_agree_column_for_column() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());

    let detail = stdout_of(&run(&["info", "--source", dump.to_str().unwrap(), "--detail"]));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&run(&[
        "info",
        "--source",
        dump.to_str().unwrap(),
        "--json",
    ])))
    .expect("--json emits JSON");

    let resolution = json["resolution"].as_array().expect("per-block resolution");
    let per_block = detail_column_lines(&detail);
    assert_eq!(resolution.len(), per_block.len(), "same blocks, same order");
    assert!(resolution.len() > 1, "sanity: this fixture has several blocks");

    let mut checked = 0usize;
    for (block, lines) in resolution.iter().zip(&per_block) {
        for column in block["columns"].as_array().unwrap() {
            let name = column["name"].as_str().unwrap();
            let outcome = column["outcome"].as_str().unwrap();
            let arrow_type = column["arrow_type"].as_str().unwrap();
            // `--detail` stays silent for a column that mapped to
            // `Utf8View`: the no-information answer, and the only Arrow type a
            // non-`Mapped` outcome ever produces, so the two arms never both
            // fire.
            let expected = if outcome == "mapped" {
                if arrow_type == "Utf8View" {
                    assert!(
                        !lines.iter().any(|l| l.starts_with(&format!("{name}: "))),
                        "a mapped Utf8View column says nothing: {name}"
                    );
                    continue;
                }
                format!("{name}: {arrow_type}")
            } else {
                assert_eq!(arrow_type, "Utf8View", "every refusal falls back to text: {name}");
                format!("{name}: {}", outcome.replace('_', " "))
            };
            assert!(
                lines.iter().any(|l| l.starts_with(&expected)),
                "no --detail line for {expected:?} among {lines:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 10, "sanity: this fixture exercises a real spread of outcomes");
}

/// **Which columns carry labels, and how they are spelled.** The cross-check
/// above only makes the two renderings agree about a column's type and
/// outcome; this pins what the labels line says.
///
/// The list is the type's own declaration order, uncapped, and each label is
/// single-quoted with any interior quote doubled — the fixture's `public.mood`
/// declares `has space`, `has,comma` and `has'quote` precisely so an unquoted
/// join cannot pass. Labels attach to a column whose *own* comparison plan is
/// an enum: a scalar enum column and a domain over one. An empty enum has no
/// plan and says `empty enum` instead, and an enum inside an array is a nested
/// column no label-valued filter reaches.
///
/// **`--json` states the labels once per type, not once per column.** The
/// export has carried `metadata.databases[].types[]` in full from the start,
/// so a per-column `labels` field would duplicate — and duplicate worse, since
/// a consumer joining `declared` against that list gets an answer for
/// `public.mood[]` too. The per-column line survives only in the text, where
/// it answers a user standing at one refused column.
#[tokio::test]
async fn an_enum_columns_declared_labels_are_listed_beneath_it() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());

    let detail = stdout_of(&run(&["info", "--source", dump.to_str().unwrap(), "--detail"]));
    let lines: Vec<&str> = detail.lines().collect();
    let at = lines
        .iter()
        .position(|l| *l == "    v_mood: Dictionary(Int32, Utf8)")
        .expect("the enum column's own line");
    assert_eq!(
        lines[at + 1],
        "        labels: 'sad', 'ok', 'happy', 'has space', 'has,comma', 'has''quote'",
        "the labels sit directly beneath their column, quoted"
    );

    let json: serde_json::Value = serde_json::from_str(&stdout_of(&run(&[
        "info",
        "--source",
        dump.to_str().unwrap(),
        "--json",
    ])))
    .expect("--json emits JSON");

    for block in json["resolution"].as_array().unwrap() {
        for column in block["columns"].as_array().unwrap() {
            assert!(
                column["labels"].is_null(),
                "the export states labels per type, not per column: {column}"
            );
        }
    }
    assert_eq!(
        json_types(&json)["public.mood"]["Enum"]["labels"],
        serde_json::json!(["sad", "ok", "happy", "has space", "has,comma", "has'quote"]),
        "the export carries the same labels, raw, on the type itself"
    );
}

/// **A column holding values its type cannot hold says how many, beneath its
/// type, in each tier** (`docs/design/decisions.md`, "D96"): `t_date`'s two
/// infinities, and `t_extremes.v_date`'s greatest date and the day past the
/// calendar beside its two infinities. A column holding none says nothing,
/// and `--json` carries each block's count as the cache holds it.
#[tokio::test]
async fn a_columns_unrepresentable_count_is_stated_beneath_it() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());

    let detail = stdout_of(&run(&["info", "--source", dump.to_str().unwrap(), "--detail"]));
    let lines: Vec<&str> = detail.lines().collect();
    let beneath = |table: &str, column: &str| {
        let block = lines.iter().position(|l| l.starts_with(&format!("{table} ("))).unwrap();
        let at = block
            + lines[block..]
                .iter()
                .position(|l| l.starts_with(&format!("    {column}: ")))
                .unwrap();
        lines[at + 1]
    };
    assert_eq!(
        beneath("public.t_date", "v_date"),
        "        unrepresentable: 2 value(s) Arrow cannot hold"
    );
    assert_eq!(
        beneath("public.t_extremes", "v_date"),
        "        unrepresentable: 2 value(s) Arrow cannot hold; 2 past 262142-12-31, which a \
         DataFusion query cannot display"
    );
    assert!(!beneath("public.t_extremes", "v_integer").contains("unrepresentable"));

    let json: serde_json::Value = serde_json::from_str(&stdout_of(&run(&[
        "info",
        "--source",
        dump.to_str().unwrap(),
        "--json",
    ])))
    .expect("--json emits JSON");
    let block = json["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|span| span["body"]["Data"]["Copy"].as_object())
        .find(|block| block["header"]["table"] == "t_date")
        .expect("t_date's block");
    assert_eq!(block["unrepresentable"][1], serde_json::json!({"format": 2, "engine": 0}));
    assert!(json["calendar_end"].is_i64(), "the calendar the counts were taken against");
}

/// The `kind` of every user-defined type the export names, keyed by name —
/// `metadata.databases[].types[]` flattened across databases, which is
/// unambiguous here because a type name is schema-qualified.
fn json_types(json: &serde_json::Value) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    for db in json["metadata"]["databases"].as_array().expect("per-database metadata") {
        for def in db["types"].as_array().expect("the type list") {
            out.insert(def["name"].as_str().unwrap().to_string(), def["kind"].clone());
        }
    }
    out
}

/// **`--detail` names every user-defined type, beneath the count that had
/// been their only trace.** Nothing else in `info` says what one is at any
/// verbosity, a column naming only its declared type.
///
/// The listing is one line per type in declaration order, and **every
/// `TypeKind` arm renders** — a listing headed `user-defined types` that
/// showed only enums would be a lie about what the dump holds. The type
/// fixture declares all six emission shapes, so the arms checked here are the
/// ones a real `pg_dump` writes; the two it never writes (an unparseable
/// composite body, a range naming no subtype) are pinned as a unit test on
/// `type_kind_summary` instead.
#[tokio::test]
async fn the_detail_listing_names_every_user_defined_type() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());

    let plain = stdout_of(&run(&["info", "--source", dump.to_str().unwrap()]));
    assert!(!plain.contains("public.mood "), "the listing is a --detail addition: {plain}");

    let detail = stdout_of(&run(&["info", "--source", dump.to_str().unwrap(), "--detail"]));
    let lines: Vec<&str> = detail.lines().collect();
    let heading = lines
        .iter()
        .position(|l| l.starts_with("user-defined types: "))
        .expect("the count heads the listing");
    let count: usize =
        lines[heading].trim_start_matches("user-defined types: ").parse().expect("a count");
    let listed: Vec<&str> = lines[heading + 1..]
        .iter()
        .take_while(|l| l.starts_with("    "))
        .map(|l| l.trim())
        .collect();
    assert_eq!(listed.len(), count, "one line per type the count counts: {listed:?}");

    // One of each emission shape, with the payload that shape carries.
    let expected = [
        "public.mood            enum: 'sad', 'ok', 'happy', 'has space', 'has,comma', 'has''quote'",
        "public.empty_enum      enum: (no labels)",
        "public.text_c          domain over text COLLATE pg_catalog.\"C\"",
        "public.derived_domain  domain over public.base_domain",
        "public.point2d         composite: x integer, y text",
        "public.collated_pair   composite: plain text, c text COLLATE pg_catalog.\"C\"",
        "public.empty_comp      composite: (no fields)",
        "public.myrange         range over double precision",
        "public.mybase          base type",
        "public.shellonly       shell type",
    ];
    for want in expected {
        let squashed = squash(want);
        assert!(
            listed.iter().any(|l| squash(l) == squashed),
            "no type line for {want:?} among {listed:?}"
        );
    }

    // The listing and the export state one fact, so they may not disagree
    // about which types the dump declares.
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&run(&[
        "info",
        "--source",
        dump.to_str().unwrap(),
        "--json",
    ])))
    .expect("--json emits JSON");
    let exported = json_types(&json);
    assert_eq!(exported.len(), count, "the export names the same types the count counts");
    for line in &listed {
        let name = line.split_whitespace().next().unwrap();
        assert!(exported.contains_key(name), "{name} is listed but not exported");
    }
}

/// A type line's name column is padded to the widest name the database
/// declares, so a comparison against a literal must not depend on how wide
/// that happens to be.
fn squash(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `--json` carries the coverage as components, not as the rendered
/// percentage the text listing prints — a script computes whatever ratio it
/// wants instead of parsing that line back apart.
#[tokio::test]
async fn the_json_export_carries_coverage_as_components() {
    let (_dir, dump) = common::sandboxed("16/types/default.sql", "dump.sql");
    let frontier = block_frontier(&dump, 2).await;
    write_truncated_cache(&dump, frontier, 1).await;
    let size = std::fs::metadata(&dump).unwrap().len();

    let json: serde_json::Value = serde_json::from_str(&stdout_of(&run(&[
        "info",
        "--source",
        dump.to_str().unwrap(),
        "--json",
    ])))
    .unwrap();
    assert_eq!(json["scanned_through"].as_u64(), Some(frontier));
    assert_eq!(json["total_size"].as_u64(), Some(size));
    assert!(json.get("Scan completion").is_none(), "no rendered string");
}

/// **The distinction `MetadataNotScanned` exists for.** Given a block whose
/// database has no DDL behind it, `NotDeclared` would be the wrong answer with
/// identical-looking output: it means the dump never explained the column and
/// is final, where this means "finish the parse and ask again".
///
/// The pairing is hand-built (see `write_truncated_cache`): a real scan states
/// each database's DDL at its first `COPY` block, so no producer leaves a
/// banked block whose database it never read. This pins the resolver's answer
/// for the callers that can still present one — an embedder's own index, or a
/// `ResumeToken` carried across a re-scan.
#[tokio::test]
async fn a_later_databases_blocks_report_metadata_not_scanned() {
    let (_dir, dump) = sandboxed_multidb();
    let source = LocalFileSource::open(&dump).unwrap();
    let full = build_index(&source, &ScanOptions::default()).await.unwrap();

    // Stop just past the first block of the *second* database, keeping only
    // the first database's DDL.
    let second_db = full
        .blocks()
        .find(|b| b.database.as_deref() == Some("pgdt_fixture_2"))
        .expect("the concatenated fixture has a second database with blocks");
    let frontier = second_db.end_offset;
    write_truncated_cache(&dump, frontier, 1).await;

    let json: serde_json::Value = serde_json::from_str(&stdout_of(&run(&[
        "info",
        "--source",
        dump.to_str().unwrap(),
        "--json",
    ])))
    .unwrap();

    let mut saw_first = false;
    let mut saw_second = false;
    for block in json["resolution"].as_array().unwrap() {
        let outcomes: Vec<&str> = block["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["outcome"].as_str().unwrap())
            .collect();
        if outcomes.is_empty() {
            continue;
        }
        match block["database"].as_str() {
            Some("pgdt_fixture_2") => {
                assert!(
                    outcomes.iter().all(|o| *o == "metadata_not_scanned"),
                    "{:?}: {outcomes:?}",
                    block["table"]
                );
                saw_second = true;
            }
            _ => {
                assert!(
                    !outcomes.contains(&"metadata_not_scanned"),
                    "the first database's DDL was read: {:?} {outcomes:?}",
                    block["table"]
                );
                saw_first = true;
            }
        }
    }
    assert!(saw_first && saw_second, "both databases must be represented");

    // And the same index resolves those columns properly once the parse
    // finishes, which is what makes the outcome's advice honest.
    assert!(run(&["parse", "--source", dump.to_str().unwrap()]).status.success());
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&run(&[
        "info",
        "--source",
        dump.to_str().unwrap(),
        "--json",
    ])))
    .unwrap();
    for block in json["resolution"].as_array().unwrap() {
        for column in block["columns"].as_array().unwrap() {
            assert_ne!(column["outcome"].as_str(), Some("metadata_not_scanned"));
        }
    }
}

/// **A signal that lands during the listing ends the process by that signal,
/// in its handler** (`docs/design/decisions.md`, "D26"). The cache is saved
/// before the listing prints, so there is nothing left to stop, and the
/// listing awaits nothing, so no task could answer it there.
///
/// Deterministic without timing: the dump lists far more than a pipe holds and
/// nothing drains it past the first byte, so the process is blocked on a write
/// inside the listing when the signal is sent. Under a handler that only
/// recorded the signal, that write would hold the process until the deadline.
#[test]
fn a_signal_during_the_listing_ends_the_process_by_that_signal() {
    use std::fmt::Write as _;
    use std::io::Read as _;
    use std::os::unix::process::ExitStatusExt as _;
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let dump = dir.path().join("many_tables.sql");
    let mut text = String::new();
    for n in 0..4000 {
        writeln!(text, "CREATE TABLE public.table_{n:04} (id integer, label text);\n").unwrap();
        writeln!(text, "COPY public.table_{n:04} (id, label) FROM stdin;\n{n}\tx\n\\.\n").unwrap();
    }
    std::fs::write(&dump, text).unwrap();

    let mut child = pgdt()
        .args(["parse", "--source", dump.to_str().unwrap()])
        .current_dir(dir.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the pgdt binary runs");
    let mut stdout = child.stdout.take().unwrap();
    let mut first = [0u8; 1];
    stdout.read_exact(&mut first).expect("the listing starts");
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .expect("`kill` runs")
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().unwrap();
            panic!("a listing nobody drains held the process past the signal");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut rest = Vec::new();
    stdout.read_to_end(&mut rest).unwrap();
    let listing = String::from_utf8_lossy(&rest);

    // Killed by the signal, not exiting with 130: a shell reports the two
    // alike, and only the first stops a calling script.
    assert_eq!(status.signal(), Some(2), "{status:?}");
    assert!(!listing.contains("wrote cache to"), "the listing was cut short: {listing}");
    // And what it cut short was only the listing: the cache is whole.
    let info = run(&["info", "--source", dump.to_str().unwrap()]);
    assert!(info.status.success(), "{}", stderr_of(&info));
    assert!(stdout_of(&info).starts_with("Scan completion: 100%"), "{}", stdout_of(&info));
}
