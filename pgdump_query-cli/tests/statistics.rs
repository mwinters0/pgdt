//! `pgdq parse --statistics` / `--statistics-group-size`, and what `info`
//! reports of them.
//!
//! What a gathered statistic means is the library's
//! (`pgdump_query/tests/statistics.rs`). What only the binary can say is that
//! `parse` gathers by default, that each flag reaches the cache it writes, that
//! a combination the two flags cannot both mean is refused rather than
//! half-honoured, that `info --json` exports every block's groups compact and
//! with no rollup, and that `info --detail` rolls those groups up per table and
//! column.

use std::path::Path;

use serde_json::Value;

mod common;
use common::{run, run_ok, sandboxed, stderr_of};

const DUMP: &str = "16/statistics/default.sql";

/// `parse` under `extra` on a private copy, then `info --json` over the cache
/// it wrote, and the dump's path for a further `info`.
fn info_after(extra: &[&str]) -> (tempfile::TempDir, std::path::PathBuf, Value) {
    let (dir, dump) = sandboxed(DUMP, "statistics.sql");
    let mut args = vec!["parse", "--source", dump.to_str().unwrap()];
    args.extend_from_slice(extra);
    let out = run(&args);
    assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
    let json = info_json(&dump);
    (dir, dump, json)
}

fn info_json(dump: &Path) -> Value {
    serde_json::from_str(&run_ok(&["info", "--source", dump.to_str().unwrap(), "--json"])).unwrap()
}

/// Every `COPY` block the export holds, in file order, by qualified name.
fn blocks_after(extra: &[&str]) -> Vec<(String, Value)> {
    let (_dir, _dump, json) = info_after(extra);
    blocks_of(&json)
}

fn blocks_of(json: &Value) -> Vec<(String, Value)> {
    json["spans"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|span| span.pointer("/body/Data/Copy"))
        .map(|copy| {
            let header = &copy["header"];
            let table = header["table"].as_str().unwrap();
            let name = match header["schema"].as_str() {
                Some(schema) => format!("{schema}.{table}"),
                None => table.to_string(),
            };
            (name, copy.clone())
        })
        .collect()
}

/// A block's statistics for the column its header names `name`.
fn column<'a>(block: &'a Value, name: &str) -> &'a Value {
    let at = block["header"]["columns"].as_array().unwrap().iter().position(|c| c == name);
    &block["statistics"]["columns"][at.unwrap()]
}

fn array(value: &Value) -> &Vec<Value> {
    value.as_array().unwrap_or_else(|| panic!("an array: {value}"))
}

#[test]
fn parse_gathers_every_table_by_default_at_a_mebibyte() {
    let blocks = blocks_after(&[]);
    assert_eq!(blocks.len(), 3);
    for (table, block) in &blocks {
        assert_eq!(block["statistics"]["group_size"], 1 << 20, "{table}");
        assert!(array(&block["statistics"]["columns"]).iter().all(|c| !c.is_null()), "{table}");
    }
}

#[test]
fn none_gathers_nothing_and_a_stated_size_is_recorded() {
    assert!(blocks_after(&["--statistics", "none"]).iter().all(|(_, b)| b["statistics"].is_null()));
    for (table, block) in blocks_after(&["--statistics-group-size", "4096"]) {
        assert_eq!(block["statistics"]["group_size"], 4096, "{table}");
    }
}

#[test]
fn a_selection_gathers_its_tables_and_columns_alone() {
    let blocks = blocks_after(&["--statistics", "specials,public.ordered.id"]);
    for (table, block) in blocks {
        let statistics = &block["statistics"];
        match table.as_str() {
            "public.specials" => {
                assert!(array(&statistics["columns"]).iter().all(|c| !c.is_null()))
            }
            "public.ordered" => {
                let columns = array(&statistics["columns"]);
                assert!(!columns[0].is_null());
                assert!(columns[1..].iter().all(Value::is_null));
            }
            _ => assert!(statistics.is_null(), "{table} was not named"),
        }
    }
}

/// **The export is every group's statistics as the cache holds them**, on
/// columns whose shape the fixture asserts
/// (`pgdump_query/tests/statistics_fixture.rs`): at a group size of a few KiB
/// `ordered` spans several groups, its `id` ascends group over group and
/// `reversed` descends, `high_card` overflows every group's dictionary and
/// `low_card` fills none, and text under the default collation keeps no
/// bounds. The long value leaves groups no row starts in. The document is one
/// compact line, with no rollup beside the blocks.
#[test]
fn info_json_exports_every_groups_statistics_compact_and_unrolled() {
    let (_dir, dump, _) = info_after(&["--statistics-group-size", "4096"]);
    let text = run_ok(&["info", "--source", dump.to_str().unwrap(), "--json"]);
    assert_eq!(text.lines().count(), 1, "compact: one line");
    let json: Value = serde_json::from_str(&text).unwrap();
    assert!(json.get("statistics").is_none(), "no rollup beside the blocks");
    let blocks = blocks_of(&json);

    let ordered = &blocks.iter().find(|(t, _)| t == "public.ordered").unwrap().1;
    let groups = array(&ordered["statistics"]["groups"]);
    assert!(groups.len() > 1, "several groups at 4 KiB");
    assert!(groups.iter().all(|g| g["rows"].as_u64().unwrap() > 0));
    assert_eq!(groups.iter().map(|g| g["rows"].as_u64().unwrap()).sum::<u64>(), 1000);

    let bound = |column: &Value, k: usize, end: &str| -> i64 {
        column["bounds"]["groups"][k][end].as_str().unwrap().parse().unwrap()
    };
    for (name, order) in [("id", "Ascending"), ("reversed", "Descending")] {
        let column = column(ordered, name);
        assert_eq!(column["bounds"]["sortedness"], order, "{name}");
        assert_eq!(array(&column["bounds"]["groups"]).len(), groups.len(), "{name}");
        assert_eq!(array(&column["null_counts"]).len(), groups.len(), "{name}");
        for k in 1..groups.len() {
            match order {
                "Ascending" => assert!(bound(column, k - 1, "max") <= bound(column, k, "min")),
                _ => assert!(bound(column, k - 1, "min") >= bound(column, k, "max")),
            }
        }
    }
    let high_card = &column(ordered, "high_card")["dictionary"];
    assert!(array(&high_card["groups"]).iter().all(Value::is_null));
    let low_card = &column(ordered, "low_card")["dictionary"];
    let entries = array(&low_card["entries"]).len() as u64;
    for group in array(&low_card["groups"]) {
        assert!(array(group).iter().all(|i| i.as_u64().unwrap() < entries), "{group}");
    }
    let default_text = column(ordered, "default_text");
    assert!(default_text["bounds"].is_null());
    assert!(!default_text["dictionary"].is_null());

    let long_value = &blocks.iter().find(|(t, _)| t == "public.long_value").unwrap().1;
    let groups = array(&long_value["statistics"]["groups"]);
    assert!(groups.iter().any(|g| g["rows"] == 0), "the long value leaves empty groups at 4 KiB");
    assert_eq!(array(&column(long_value, "id")["null_counts"]).len(), groups.len());
}

/// **`--detail`'s rollup is the sum of the export's groups**: each table and
/// column line under `statistics:` is re-derived here from the groups `--json`
/// exports, a group no row starts in outside every share, and a cache with no
/// statistics at all says so in one line.
#[test]
fn the_detail_listing_rolls_up_the_exports_groups() {
    let (_dir, dump, json) = info_after(&["--statistics-group-size", "4096"]);
    let detail = run_ok(&["info", "--source", dump.to_str().unwrap(), "--detail"]);
    let section: Vec<&str> = detail
        .lines()
        .skip_while(|l| *l != "statistics:")
        .skip(1)
        .take_while(|l| l.starts_with("    "))
        .collect();
    let mean = |total: u64, count: u64| (total + count / 2).checked_div(count).unwrap_or(0);
    let mut expected = Vec::new();
    for (name, block) in blocks_of(&json) {
        let statistics = &block["statistics"];
        let groups = array(&statistics["groups"]);
        let with_rows: Vec<bool> = groups.iter().map(|g| g["rows"] != 0).collect();
        let occupied = with_rows.iter().filter(|&&r| r).count() as u64;
        let sum = |key: &str| groups.iter().map(|g| g[key].as_u64().unwrap()).sum::<u64>();
        let empty = groups.len() as u64 - occupied;
        let mut line = format!(
            "    {name}: statistics over 1 of 1 block(s), group size {} bytes; {} rows and {} \
             bytes per group over {} group(s)",
            statistics["group_size"],
            mean(sum("rows"), occupied),
            mean(sum("bytes"), occupied),
            groups.len(),
        );
        if empty > 0 {
            line.push_str(&format!(", {empty} empty"));
        }
        expected.push(line);
        let kept = |per_group: &Value| {
            array(per_group)
                .iter()
                .zip(&with_rows)
                .filter(|(g, rows)| !g.is_null() && **rows)
                .count()
        };
        for (name, column) in
            array(&block["header"]["columns"]).iter().zip(array(&statistics["columns"]))
        {
            let name = name.as_str().unwrap();
            let bounds = match &column["bounds"] {
                Value::Null => "no bounds".to_string(),
                bounds => format!(
                    "{}, bounds in {} of {occupied} group(s)",
                    bounds["sortedness"].as_str().unwrap().to_lowercase(),
                    kept(&bounds["groups"])
                ),
            };
            let dictionary = match &column["dictionary"] {
                Value::Null => "no dictionary".to_string(),
                dictionary => {
                    format!("dictionary in {} of {occupied} group(s)", kept(&dictionary["groups"]))
                }
            };
            expected.push(format!("        {name}: over 1 of 1 block(s), {bounds}, {dictionary}"));
        }
    }
    assert!(expected.iter().any(|l| l.ends_with(" empty")), "an empty group is exercised");
    assert_eq!(section, expected);

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

/// **A `parse` that re-reads blocks for statistics they lack says how many on
/// stderr**, once when it starts and once when every one is re-read, and its
/// resume line no longer claims there was nothing to scan; a `parse` whose
/// blocks lack nothing prints neither line and says so.
#[test]
fn a_backfilling_parse_counts_the_blocks_it_rereads() {
    let (_dir, dump) = sandboxed(DUMP, "statistics.sql");
    let parse = |extra: &[&str]| {
        let mut args = vec!["parse", "--source", dump.to_str().unwrap()];
        args.extend_from_slice(extra);
        let out = run(&args);
        assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
        (common::stdout_of(&out), stderr_of(&out))
    };
    let backfill_lines = |stderr: &str| stderr.lines().filter(|l| l.contains("back-fill")).count();

    let (_, stderr) = parse(&["--statistics", "none"]);
    assert_eq!(backfill_lines(&stderr), 0, "{stderr}");

    let (stdout, stderr) = parse(&["--statistics", "specials,public.ordered"]);
    assert!(stderr.contains("statistics back-fill started blocks=2"), "{stderr}");
    assert!(stderr.contains("statistics back-fill complete blocks=2"), "{stderr}");
    assert!(!stdout.contains("nothing to scan"), "{stdout}");
    assert!(
        stdout.contains("only blocks lacking the requested statistics were re-read"),
        "{stdout}"
    );

    let (stdout, stderr) = parse(&["--statistics-group-size", "4096"]);
    assert!(stderr.contains("statistics back-fill started blocks=3"), "{stderr}");
    assert!(stderr.contains("statistics back-fill complete blocks=3"), "{stderr}");
    let sizes: Vec<u64> = blocks_of(&info_json(&dump))
        .iter()
        .map(|(_, block)| block["statistics"]["group_size"].as_u64().unwrap())
        .collect();
    assert_eq!(sizes, vec![4096; 3]);

    let (stdout_again, stderr) = parse(&[]);
    assert_eq!(backfill_lines(&stderr), 0, "{stderr}");
    assert!(stdout_again.starts_with("nothing to scan"), "{stdout_again}");
    assert_ne!(stdout, stdout_again);
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
