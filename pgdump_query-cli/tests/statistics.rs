//! `pgdq parse --statistics` / `--statistics-group-size`, and what `info`
//! reports of them.
//!
//! What a gathered statistic means is the library's
//! (`pgdump_query/tests/statistics.rs`). What only the binary can say is that
//! `parse` gathers by default, that each flag reaches the cache it writes, that
//! a combination the two flags cannot both mean is refused rather than
//! half-honoured, and that `info --detail` and `--json` report one rollup per
//! table and column and no group's values.

use std::path::Path;

mod common;
use common::{run, run_ok, sandboxed, stderr_of};

const DUMP: &str = "16/statistics/default.sql";

/// `parse` under `extra` on a private copy, then `info --json` over the cache
/// it wrote, and the dump's path for a further `info`.
fn info_after(extra: &[&str]) -> (tempfile::TempDir, std::path::PathBuf, serde_json::Value) {
    let (dir, dump) = sandboxed(DUMP, "statistics.sql");
    let mut args = vec!["parse", "--source", dump.to_str().unwrap()];
    args.extend_from_slice(extra);
    let out = run(&args);
    assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
    let json = info_json(&dump);
    (dir, dump, json)
}

fn info_json(dump: &Path) -> serde_json::Value {
    serde_json::from_str(&run_ok(&["info", "--source", dump.to_str().unwrap(), "--json"])).unwrap()
}

/// The export's statistics rollup, keyed by table.
fn tables_after(extra: &[&str]) -> Vec<(String, serde_json::Value)> {
    let (_dir, _dump, json) = info_after(extra);
    tables_of(&json)
}

fn tables_of(json: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    json["statistics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["table"].as_str().unwrap().to_string(), t.clone()))
        .collect()
}

fn column<'a>(table: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    table["columns"].as_array().unwrap().iter().find(|c| c["name"] == name).unwrap()
}

#[test]
fn parse_gathers_every_table_by_default_at_a_mebibyte() {
    let tables = tables_after(&[]);
    assert_eq!(tables.len(), 3);
    for (table, statistics) in &tables {
        assert_eq!(statistics["group_sizes"], serde_json::json!([1 << 20]), "{table}");
        assert_eq!(statistics["gathered_blocks"], 1, "{table}");
        let columns = statistics["columns"].as_array().unwrap();
        assert!(columns.iter().all(|c| c["gathered_blocks"] == 1), "{table}");
    }
}

#[test]
fn none_gathers_nothing_and_a_stated_size_is_recorded() {
    assert!(tables_after(&["--statistics", "none"]).iter().all(|(_, s)| s["gathered_blocks"] == 0));
    for (table, statistics) in tables_after(&["--statistics-group-size", "4096"]) {
        assert_eq!(statistics["group_sizes"], serde_json::json!([4096]), "{table}");
    }
}

#[test]
fn a_selection_gathers_its_tables_and_columns_alone() {
    let tables = tables_after(&["--statistics", "specials,public.ordered.id"]);
    for (table, statistics) in tables {
        let columns = statistics["columns"].as_array().unwrap();
        match table.as_str() {
            "public.specials" => assert!(columns.iter().all(|c| c["gathered_blocks"] == 1)),
            "public.ordered" => {
                assert_eq!(columns[0]["gathered_blocks"], 1);
                assert!(columns[1..].iter().all(|c| c["gathered_blocks"] == 0));
            }
            _ => assert_eq!(statistics["gathered_blocks"], 0, "{table} was not named"),
        }
    }
}

/// **The rollup reads what was gathered**, on columns whose shape the fixture
/// asserts (`pgdump_query/tests/statistics_fixture.rs`): at a group size of a
/// few KiB `ordered` spans several groups, its `id` ascends and `reversed`
/// descends, `high_card` overflows every group's dictionary and `low_card`
/// fills none, and text under the default collation keeps no bounds. The long
/// value leaves groups no row starts in, outside every share.
#[test]
fn info_reports_each_tables_statistics_as_counts() {
    let (_dir, _dump, json) = info_after(&["--statistics-group-size", "4096"]);
    let tables = tables_of(&json);
    let ordered = &tables.iter().find(|(t, _)| t == "public.ordered").unwrap().1;
    let groups = ordered["groups"].as_u64().unwrap();
    assert!(groups > 1, "several groups at 4 KiB");
    assert_eq!(ordered["empty_groups"], 0);
    assert_eq!(ordered["rows"], 1000);

    let id = column(ordered, "id");
    assert_eq!(
        id["sortedness"],
        serde_json::json!({"ascending": 1, "descending": 0, "unsorted": 0})
    );
    assert_eq!(
        (&id["groups_with_rows"], &id["groups_with_bounds"]),
        (&groups.into(), &groups.into())
    );
    assert_eq!(column(ordered, "reversed")["sortedness"]["descending"], 1);
    assert_eq!(column(ordered, "high_card")["groups_with_dictionary"], 0);
    assert_eq!(column(ordered, "low_card")["groups_with_dictionary"], groups);
    let default_text = column(ordered, "default_text");
    assert_eq!(
        default_text["sortedness"],
        serde_json::json!({"ascending": 0, "descending": 0, "unsorted": 0})
    );
    assert_eq!(default_text["dictionary_blocks"], 1);

    let long_value = &tables.iter().find(|(t, _)| t == "public.long_value").unwrap().1;
    let empty = long_value["empty_groups"].as_u64().unwrap();
    assert!(empty > 0, "the long value leaves empty groups at 4 KiB");
    assert_eq!(
        column(long_value, "id")["groups_with_rows"].as_u64().unwrap(),
        long_value["groups"].as_u64().unwrap() - empty
    );

    for span in json["spans"].as_array().unwrap() {
        if let Some(copy) = span["body"]["Data"]["Copy"].as_object() {
            assert!(!copy.contains_key("statistics"), "no group's values are exported: {copy:?}");
        }
    }
}

/// **One rollup, two renderings**: every table and column line `--detail`
/// prints under `statistics:` is the export's record for it, and a cache with
/// no statistics at all says so in one line.
#[test]
fn the_detail_listing_renders_the_exports_rollup() {
    let (_dir, dump, json) = info_after(&["--statistics-group-size", "4096"]);
    let detail = run_ok(&["info", "--source", dump.to_str().unwrap(), "--detail"]);
    let section: Vec<&str> = detail
        .lines()
        .skip_while(|l| *l != "statistics:")
        .skip(1)
        .take_while(|l| l.starts_with("    "))
        .collect();
    let mut lines = section.iter();
    for (name, table) in tables_of(&json) {
        let line = lines.next().expect("a line per table");
        let head = format!("    {name}: statistics over 1 of 1 block(s), group size 4096 bytes; ");
        assert!(line.starts_with(&head), "{line}");
        assert!(line.contains(&format!(" per group over {} group(s)", table["groups"])), "{line}");
        for column in table["columns"].as_array().unwrap() {
            let line = lines.next().expect("a line per column");
            let name = column["name"].as_str().unwrap();
            let head = format!("        {name}: over 1 of 1 block(s), ");
            assert!(line.starts_with(&head), "{line}");
            let rows = &column["groups_with_rows"];
            let bounds = match &column["sortedness"] {
                s if s["ascending"] == 1 => format!(
                    "ascending, bounds in {} of {rows} group(s)",
                    column["groups_with_bounds"]
                ),
                s if s["descending"] == 1 => format!(
                    "descending, bounds in {} of {rows} group(s)",
                    column["groups_with_bounds"]
                ),
                s if s["unsorted"] == 1 => format!(
                    "unsorted, bounds in {} of {rows} group(s)",
                    column["groups_with_bounds"]
                ),
                _ => "no bounds".to_string(),
            };
            let dictionary = match column["dictionary_blocks"].as_u64().unwrap() {
                0 => "no dictionary".to_string(),
                _ => {
                    format!("dictionary in {} of {rows} group(s)", column["groups_with_dictionary"])
                }
            };
            assert_eq!(*line, format!("{head}{bounds}, {dictionary}"));
        }
    }
    assert!(lines.next().is_none(), "the section is the export's rollup and nothing else");

    let (_dir, dump, _) = info_after(&["--statistics", "none"]);
    let detail = run_ok(&["info", "--source", dump.to_str().unwrap(), "--detail"]);
    assert!(detail.lines().any(|l| l == "statistics: none gathered"), "{detail}");
    let plain = run_ok(&["info", "--source", dump.to_str().unwrap()]);
    assert!(!plain.contains("statistics"), "the section is a --detail addition: {plain}");
}

/// **A table is its database and the name its blocks' `COPY` gives**: a
/// `--load-via-partition-root` dump's partitions all copy into the root, so they
/// roll up under it, as a query of the root reads them; and a table in a later
/// database sits under that database's heading, as the block listing does.
#[test]
fn a_rollup_keys_a_table_by_database_and_the_name_its_blocks_copy_into() {
    for (fixture, heading, line) in [
        (
            "16/partitions/load-via-partition-root.sql",
            None,
            "    public.evt: statistics over 2 of 2 block(s), ",
        ),
        (
            "16/edge_cases/dumpall.sql",
            Some("database: pgdq_tenant"),
            "    public.tenant_only: statistics over 1 of 1 block(s), ",
        ),
    ] {
        let (_dir, dump) = sandboxed(fixture, "rollup.sql");
        run_ok(&["parse", "--source", dump.to_str().unwrap()]);
        let detail = run_ok(&["info", "--source", dump.to_str().unwrap(), "--detail"]);
        let section: Vec<&str> = detail.lines().skip_while(|l| *l != "statistics:").collect();
        let at = section.iter().position(|l| l.starts_with(line)).expect(line);
        if let Some(heading) = heading {
            assert_eq!(section[at - 1], heading, "{fixture}");
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
