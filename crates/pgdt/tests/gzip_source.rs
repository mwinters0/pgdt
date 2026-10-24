//! `pgdt` against gzip input, end to end: what the binary does with the file
//! `pg_dump -Fp --compress` writes.

use std::io::Read;
use std::path::Path;

mod common;
use common::{VERSIONS, gzip_fixture, run_ok};

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
