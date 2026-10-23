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
    }
}
