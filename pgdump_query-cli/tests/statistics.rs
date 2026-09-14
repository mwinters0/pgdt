//! `pgdq parse --statistics` / `--statistics-group-size` — the statistics
//! flags' CLI surface.
//!
//! What a gathered statistic means is the library's
//! (`pgdump_query/tests/statistics.rs`). What only the binary can say is that
//! `parse` gathers by default, that each flag reaches the cache it writes, and
//! that a combination the two flags cannot both mean is refused rather than
//! half-honoured. The cache is read back through `info --json`, which carries
//! each block as the library holds it.

use std::path::Path;

mod common;
use common::{run, sandboxed, stderr_of, stdout_of};

const DUMP: &str = "16/statistics/default.sql";

/// `parse` under `extra` on a private copy, then the `COPY` blocks of the
/// cache it wrote, keyed by table.
fn blocks_after(extra: &[&str]) -> Vec<(String, serde_json::Value)> {
    let (_dir, dump) = sandboxed(DUMP, "statistics.sql");
    let mut args = vec!["parse", "--source", dump.to_str().unwrap()];
    args.extend_from_slice(extra);
    let out = run(&args);
    assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
    info_blocks(&dump)
}

fn info_blocks(dump: &Path) -> Vec<(String, serde_json::Value)> {
    let out = run(&["info", "--source", dump.to_str().unwrap(), "--json"]);
    assert!(out.status.success(), "{}", stderr_of(&out));
    let json: serde_json::Value = serde_json::from_str(&stdout_of(&out)).unwrap();
    json["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|span| span["body"]["Data"]["Copy"].as_object())
        .map(|copy| {
            (copy["header"]["table"].as_str().unwrap().to_string(), copy["statistics"].clone())
        })
        .collect()
}

#[test]
fn parse_gathers_every_table_by_default_at_a_mebibyte() {
    let blocks = blocks_after(&[]);
    assert_eq!(blocks.len(), 3);
    for (table, statistics) in &blocks {
        assert_eq!(statistics["group_size"], 1 << 20, "{table}");
        assert!(statistics["columns"].as_array().unwrap().iter().all(|c| !c.is_null()), "{table}");
    }
}

#[test]
fn none_gathers_nothing_and_a_stated_size_is_recorded() {
    assert!(blocks_after(&["--statistics", "none"]).iter().all(|(_, s)| s.is_null()));
    for (table, statistics) in blocks_after(&["--statistics-group-size", "4096"]) {
        assert_eq!(statistics["group_size"], 4096, "{table}");
    }
}

#[test]
fn a_selection_gathers_its_tables_and_columns_alone() {
    let blocks = blocks_after(&["--statistics", "specials,public.ordered.id"]);
    for (table, statistics) in blocks {
        match table.as_str() {
            "specials" => {
                assert!(statistics["columns"].as_array().unwrap().iter().all(|c| !c.is_null()))
            }
            "ordered" => {
                let columns = statistics["columns"].as_array().unwrap();
                assert!(!columns[0].is_null());
                assert!(columns[1..].iter().all(serde_json::Value::is_null));
            }
            _ => assert!(statistics.is_null(), "{table} was not named"),
        }
    }
}

#[test]
fn contradictory_or_empty_statistics_flags_are_refused() {
    let (_dir, dump) = sandboxed(DUMP, "refused.sql");
    let source = dump.to_str().unwrap();
    for (extra, says) in [
        (&["--statistics", "none", "--statistics-group-size", "64"][..], "drop one of them"),
        (&["--statistics-group-size", "0"][..], "a group size of 0"),
        (&["--statistics", "public..id"][..], "is not a table"),
        (&["--statistics", "a.b.c.d"][..], "more parts"),
        (&["--preamble-only", "--statistics", "none"][..], "cannot be used with"),
    ] {
        let mut args = vec!["parse", "--source", source];
        args.extend_from_slice(extra);
        let out = run(&args);
        assert!(!out.status.success(), "{extra:?} was accepted");
        assert!(stderr_of(&out).contains(says), "{extra:?}: {}", stderr_of(&out));
    }
    assert!(!dump.with_extension("sql.dqcache").exists(), "a refusal scans nothing");
}
