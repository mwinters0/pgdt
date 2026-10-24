//! `pgdt` against gzip input, end to end: what the binary does with the file
//! `pg_dump -Fp --compress` writes.

use std::io::Read;
use std::path::Path;

mod common;
use common::{VERSIONS, gzip_fixture, run, run_ok, stderr_of};

/// `gz` decompressed beside itself as `<name>.sql`, by an encoder not ours.
fn plaintext_beside(gz: &Path) -> std::path::PathBuf {
    let mut plain = Vec::new();
    flate2::read::GzDecoder::new(std::fs::File::open(gz).unwrap()).read_to_end(&mut plain).unwrap();
    let path = gz.with_extension("");
    std::fs::write(&path, plain).unwrap();
    path
}

/// **A gzip dump reads as its plaintext reads**, at every tested major: a
/// cold `query` runs the first pass and answers what the decompressed file
/// answers, saving a map that tiles what it covers, a `parse` writes the cache beside the dump, and a `query` through
/// that cache answers the same again, and `info` describes what it describes.
#[test]
fn a_gzip_dump_reads_as_its_plaintext() {
    for version in VERSIONS {
        let dir = tempfile::tempdir().unwrap();
        let gz = dir.path().join("dump.sql.gz");
        std::fs::copy(gzip_fixture(version, "edge_cases", "compress"), &gz).unwrap();
        let plain = plaintext_beside(&gz);
        let (gz_arg, plain_arg) = (gz.to_str().unwrap(), plain.to_str().unwrap());
        let cache = dir.path().join("dump.sql.gz.dtcache");

        for table in ["public.widgets", "logs.events", "public.escapes"] {
            let expected = run_ok(&["query", "--source", plain_arg, "--table", table]);
            let cold = run_ok(&["query", "--source", gz_arg, "--table", table]);
            assert_eq!(cold, expected, "{version}: {table}, cold");
            assert!(cache.exists(), "{version}: a cold query saves the map it built");
            // Stopped at its table, with the end met by the first pass's
            // reads: the map it saved tiles the file to the extent it saved.
            let info = run_ok(&["info", "--source", gz_arg]);
            assert!(!info.contains("does not tile"), "{version}: {table}: {info}");
            std::fs::remove_file(&cache).unwrap();
            run_ok(&["parse", "--source", gz_arg]);
            let warm = run_ok(&["query", "--source", gz_arg, "--table", table]);
            assert_eq!(warm, expected, "{version}: {table}, through the cache");
        }
        run_ok(&["parse", "--source", plain_arg]);
        let described = |source: &str| {
            let info = run_ok(&["info", "--source", source, "--json"]);
            let info: serde_json::Value = serde_json::from_str(&info).unwrap();
            assert!(info["metadata"].is_object(), "{version}: {source} describes its tables");
            (info["metadata"].clone(), info["scanned_through"].clone())
        };
        assert_eq!(described(gz_arg), described(plain_arg), "{version}: what info describes");

        // Under the spacing, so one unit by the reader's choice: never warned,
        // and described in gzip's words.
        let info: serde_json::Value =
            serde_json::from_str(&run_ok(&["info", "--source", gz_arg, "--json"])).unwrap();
        assert!(non_seekable_warning(&info).is_none(), "{version}: {}", info["diagnostics"]);
        let detail = run_ok(&["info", "--source", gz_arg, "--detail"]);
        assert!(
            detail.contains("compression: gzip — 1 unit(s) in 1 member(s), largest unit "),
            "{version}: {detail}"
        );
    }
}

/// The non-seekable warning an `info --json` document's `diagnostics` carry,
/// if any (`docs/design/decisions.md`, "D19").
fn non_seekable_warning(info: &serde_json::Value) -> Option<serde_json::Value> {
    let diagnostics = info["diagnostics"].as_array().expect("diagnostics is always an array");
    diagnostics.iter().find_map(|d| d["kind"].get("NonSeekableCompressedSource").cloned())
}

/// **A member whose writer ended no deflate block across twice the shipped
/// spacing is warned, in gzip's words**: written as `pg_dump`'s `gzwrite`
/// drives zlib — 8 KiB calls with `Z_NO_FLUSH`, `Z_FINISH` on the last — at
/// level 1 by `flate2` over zlib-rs, the engine `libz-rs-sys` exports, in
/// every build (the workspace's `Cargo.toml`). That level ends no block
/// before the final one, which it opens over its lookahead at `Z_FINISH`, so
/// the member is two units, the first all but its last few hundred bytes. The
/// warning names that unit against the spacing and the remedy.
#[test]
fn a_gzip_dump_its_writer_ended_no_block_in_is_warned() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let mut plain = String::from("SET client_encoding = 'UTF8';\n\n");
    plain.push_str(
        "CREATE TABLE public.t (\n    id integer,\n    name text\n);\n\n\
         COPY public.t (id, name) FROM stdin;\n",
    );
    let mut id: u64 = 0;
    while plain.len() < 52 << 20 {
        id += 1;
        plain.push_str(&format!("{id}\trow {id}, {}\n", id.wrapping_mul(7919) % 10_007));
    }
    plain.push_str("\\.\n\n");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(1));
    for piece in plain.as_bytes().chunks(8192) {
        encoder.write_all(piece).unwrap();
    }
    let gz = dir.path().join("level1.sql.gz");
    std::fs::write(&gz, encoder.finish().unwrap()).unwrap();
    let gz_arg = gz.to_str().unwrap();

    let parsed = run_ok(&["parse", "--source", gz_arg]);
    assert!(parsed.contains("[warning] this gzip source has no seek structure"), "{parsed}");
    let info: serde_json::Value =
        serde_json::from_str(&run_ok(&["info", "--source", gz_arg, "--json"])).unwrap();
    let warning = non_seekable_warning(&info).unwrap_or_else(|| panic!("{}", info["diagnostics"]));
    let spacing = warning["container"]["Gzip"]["spacing"].as_u64().expect("gzip's container");
    let largest = warning["largest_unit"].as_u64().unwrap();
    assert!(largest > 2 * spacing, "{warning}");
    assert_eq!(warning["block_count"], info["compression"]["blocks"], "{warning}");
    assert_eq!(largest, info["compression"]["max_block_uncompressed"].as_u64().unwrap());
}

/// The status lines naming a gzip first pass in `stderr`: those saying it
/// started, and those saying it completed.
fn first_pass_lines(stderr: &str) -> (Vec<&str>, Vec<&str>) {
    let lines = |what: &str| stderr.lines().filter(|l| l.contains(what)).collect::<Vec<_>>();
    (lines("first pass started"), lines("first pass complete"))
}

/// `args` run to success, and what it wrote to stderr.
fn stderr_ok(args: &[&str]) -> String {
    let output = run(args);
    let stderr = stderr_of(&output);
    assert!(output.status.success(), "{args:?}: {stderr}");
    stderr
}

/// **A gzip first pass says it ran, once, and why the run is serial**, as the
/// `.xz` walk does (`docs/design/decisions.md`, "D64"): a cold `parse` says it
/// started, naming the codec and that it reads serially while it builds the
/// index, and that it completed, naming the units and members the index
/// holds; a `parse` and an `info` over the complete cache it saved say
/// nothing of it.
#[test]
fn a_gzip_first_pass_is_announced_once_started_and_complete() {
    let dir = tempfile::tempdir().unwrap();
    let gz = dir.path().join("dump.sql.gz");
    std::fs::copy(gzip_fixture(16, "edge_cases", "compress"), &gz).unwrap();
    let gz_arg = gz.to_str().unwrap();

    let stderr = stderr_ok(&["parse", "--source", gz_arg]);
    let (started, complete) = first_pass_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 1), "{stderr}");
    assert!(started[0].contains("codec=gzip"), "{stderr}");
    assert!(started[0].contains("reading serially while it builds the index"), "{stderr}");
    assert!(!started[0].contains("resum"), "a fresh pass resumes nothing: {stderr}");
    assert!(complete[0].contains("units=1 members=1"), "{stderr}");
    assert!(stderr.find("first pass started") < stderr.find("first pass complete"), "{stderr}");

    for args in [["parse", "--source", gz_arg], ["info", "--source", gz_arg]] {
        let stderr = stderr_ok(&args);
        assert!(!stderr.contains("first pass"), "{args:?}: {stderr}");
    }
}

/// **A resumed first pass names the checkpoint it resumes from**: a cold
/// `query` stopped at a small table ahead of a large one saves an index
/// reaching only as far as its pass got, saying it started and not that it
/// completed, and the `parse` that resumes it says from which checkpoint,
/// then that it completed.
#[test]
fn a_resumed_gzip_first_pass_names_its_checkpoint() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let mut plain = String::from("SET client_encoding = 'UTF8';\n\n");
    for (table, rows) in [("small", 50u64), ("large", 200_000)] {
        plain.push_str(&format!(
            "CREATE TABLE public.{table} (\n    id integer,\n    name text\n);\n\n\
             COPY public.{table} (id, name) FROM stdin;\n"
        ));
        for id in 1..=rows {
            plain.push_str(&format!(
                "{id}\trow {id} of {table}, {}\n",
                id.wrapping_mul(7919) % 10_007
            ));
        }
        plain.push_str("\\.\n\n");
    }
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(plain.as_bytes()).unwrap();
    let gz = dir.path().join("dump.sql.gz");
    std::fs::write(&gz, encoder.finish().unwrap()).unwrap();
    let gz_arg = gz.to_str().unwrap();

    let output = run(&["query", "--source", gz_arg, "--table", "public.small"]);
    let stderr = stderr_of(&output);
    assert!(output.status.success(), "{stderr}");
    let (started, complete) = first_pass_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 0), "stopped short of the end: {stderr}");

    let stderr = stderr_ok(&["parse", "--source", gz_arg]);
    let (started, complete) = first_pass_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 1), "{stderr}");
    assert!(started[0].contains("resuming from a saved checkpoint"), "{stderr}");
    assert!(started[0].contains("checkpoint=0 plaintext=0"), "{stderr}");
    assert!(complete[0].contains("members=1"), "{stderr}");
}

/// `plain` written as BGZF beside it, by `noodles-bgzf`'s writer — a member
/// to each 64 KiB of plaintext and the empty member that ends the file.
fn bgzf_beside(plain: &Path) -> std::path::PathBuf {
    use std::io::Write as _;
    let mut writer = noodles_bgzf::io::Writer::new(Vec::new());
    writer.write_all(&std::fs::read(plain).unwrap()).unwrap();
    let path = plain.with_extension("sql.bgz");
    std::fs::write(&path, writer.finish().unwrap()).unwrap();
    path
}

/// The status lines naming a BGZF walk in `stderr`: those saying it started,
/// and those saying it completed.
fn walk_lines(stderr: &str) -> (Vec<&str>, Vec<&str>) {
    let lines = |what: &str| stderr.lines().filter(|l| l.contains(what)).collect::<Vec<_>>();
    (lines("BGZF member walk started"), lines("BGZF member walk complete"))
}

/// **A BGZF dump is walked at open and reads as its plaintext**: a cold
/// `query` answers what the decompressed file answers, a cold `parse` says
/// once that its members' walk started and that it completed — counting every
/// member, the empty one ending the file included, not only the one unit's —
/// and runs no first pass, `info` describes
/// it in gzip's words with every member counted, and a `parse` and an `info`
/// over its cache say nothing of the walk (`docs/design/decisions.md`,
/// "D64").
#[test]
fn a_bgzf_dump_is_walked_at_open_and_reads_as_its_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let gz = dir.path().join("dump.sql.gz");
    std::fs::copy(gzip_fixture(16, "edge_cases", "compress"), &gz).unwrap();
    let plain = plaintext_beside(&gz);
    let bgz = bgzf_beside(&plain);
    let (bgz_arg, plain_arg) = (bgz.to_str().unwrap(), plain.to_str().unwrap());

    for table in ["public.widgets", "logs.events", "public.escapes"] {
        let expected = run_ok(&["query", "--source", plain_arg, "--table", table]);
        let cold = ["query", "--source", bgz_arg, "--table", table, "--dtcache", "none"];
        assert_eq!(run_ok(&cold), expected, "{table}, cold");
    }

    let stderr = stderr_ok(&["parse", "--source", bgz_arg]);
    let (started, complete) = walk_lines(&stderr);
    assert_eq!((started.len(), complete.len()), (1, 1), "{stderr}");
    assert!(started[0].contains("codec=gzip"), "{stderr}");
    let members: u64 = complete[0]
        .split_once("units=1 members=")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("one unit and its members: {stderr}"));
    // The plaintext's member or members, and the empty one ending the file.
    assert!(members >= 2, "{stderr}");
    assert!(!stderr.contains("first pass"), "a BGZF file is walked, not passed: {stderr}");
    let detail = run_ok(&["info", "--source", bgz_arg, "--detail"]);
    let shape = format!("compression: gzip — 1 unit(s) in {members} member(s), largest unit ");
    assert!(detail.contains(&shape), "{detail}");
    for args in [["parse", "--source", bgz_arg], ["info", "--source", bgz_arg]] {
        let stderr = stderr_ok(&args);
        assert!(!stderr.contains("BGZF member walk"), "{args:?}: {stderr}");
    }
    for table in ["public.widgets", "logs.events"] {
        let expected = run_ok(&["query", "--source", plain_arg, "--table", table]);
        let warm = run_ok(&["query", "--source", bgz_arg, "--table", table]);
        assert_eq!(warm, expected, "{table}, through the cache");
    }
}
