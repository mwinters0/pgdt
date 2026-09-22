//! `pgdt parse --statistics` / `--statistics-group-size` /
//! `--statistics-min-rows` / `--statistics-max-rows`, and what `info` reports
//! of them.
//!
//! What a gathered statistic means is the library's
//! (`pgdump_query/tests/statistics.rs`). What only the binary can say is that
//! `parse` gathers by default, that each flag reaches the cache it writes, that
//! a combination the flags cannot all mean is refused rather than
//! half-honoured, that `info --json` exports every block's groups compact and
//! with no rollup, that `info --detail` rolls those groups up per table and
//! column, that `query` skips what they rule out unless told `--statistics
//! none`, and that it says what an early stop left unread.

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

/// **`parse` gathers every table by default at a mebibyte**, recording the
/// minimum each block was sized under — which makes the block of the megabyte
/// row, whose groups hold a row or none, one group of a coarser size — and
/// `--statistics-min-rows 0` leaves that block at a mebibyte.
#[test]
fn parse_gathers_every_table_by_default_at_a_mebibyte() {
    let blocks = blocks_after(&[]);
    assert_eq!(blocks.len(), 3);
    for (table, block) in &blocks {
        let statistics = &block["statistics"];
        let size = statistics["group_size"].as_u64().unwrap();
        match table.as_str() {
            "public.long_value" => assert!(size > 1 << 20, "{table}: {size}"),
            _ => assert_eq!(size, 1 << 20, "{table}"),
        }
        assert_eq!(array(&statistics["groups"]).len(), 1, "{table}");
        assert_eq!(statistics["sizing"]["Density"]["min_rows"], 1024, "{table}");
        assert!(array(&statistics["columns"]).iter().all(|c| !c.is_null()), "{table}");
    }
    for (table, block) in blocks_after(&["--statistics-min-rows", "0"]) {
        let statistics = &block["statistics"];
        assert_eq!(statistics["group_size"], 1 << 20, "{table}");
        assert_eq!(statistics["sizing"]["Density"]["min_rows"], 0, "{table}");
        if table == "public.long_value" {
            assert!(array(&statistics["groups"]).len() > 1);
        }
    }
}

#[test]
fn none_gathers_nothing_and_a_stated_size_is_recorded() {
    assert!(blocks_after(&["--statistics", "none"]).iter().all(|(_, b)| b["statistics"].is_null()));
    for (table, block) in blocks_after(&["--statistics-group-size", "4096"]) {
        assert_eq!(block["statistics"]["group_size"], 4096, "{table}");
        assert_eq!(block["statistics"]["sizing"], "Stated", "{table}");
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

/// **A stated maximum re-reads the table too dense for it and says where it
/// still misses.** `ordered`'s thousand rows are one group at a mebibyte, so a
/// maximum of a hundred is broken there; the block is read a second time at
/// the finer size its own group predicts, records the maximum it was sized
/// under, and — its rows all lying inside one of those finer groups — says on
/// stderr that it still holds more. Asking again re-reads nothing and says it
/// again.
#[test]
fn a_stated_maximum_rereads_the_dense_table_and_says_where_it_still_misses() {
    let (_dir, dump) = sandboxed(DUMP, "maximum.sql");
    let source = dump.to_str().unwrap();
    let flags = ["--statistics-min-rows", "8", "--statistics-max-rows", "100"];
    let parse = |extra: &[&str]| {
        let mut args = vec!["parse", "--source", source];
        args.extend_from_slice(extra);
        let out = run(&args);
        assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
        stderr_of(&out)
    };

    let stderr = parse(&flags);
    assert!(stderr.contains("statistics back-fill started blocks=1"), "{stderr}");
    assert!(stderr.contains("statistics back-fill complete blocks=1"), "{stderr}");
    assert!(
        stderr.contains(
            "statistics groups still hold more rows than the stated maximum \
             table=\"ordered\" group_size=65536 max_rows=100"
        ),
        "{stderr}"
    );

    let ordered = blocks_of(&info_json(&dump));
    let ordered = &ordered.iter().find(|(t, _)| t == "public.ordered").unwrap().1["statistics"];
    assert_eq!(ordered["group_size"], 65536);
    assert_eq!(ordered["sizing"]["Density"]["min_rows"], 8);
    assert_eq!(ordered["sizing"]["Density"]["max_rows"], 100);
    assert_eq!(array(&ordered["groups"]).len(), 1, "the rows all start in one finer group");

    let stderr = parse(&flags);
    assert!(!stderr.contains("statistics back-fill started"), "no third read: {stderr}");
    assert!(stderr.contains("still hold more rows than the stated maximum"), "{stderr}");
    let stderr = parse(&[]);
    assert!(!stderr.contains("statistics back-fill started"), "unstated takes what is held");
    assert!(!stderr.contains("still hold more rows"), "{stderr}");
}

/// **A `--memory` too small for a table's statistics declines it rather than
/// gathering it badly**: the run exits clean, says which block declined and
/// what it declined under, records the decline in the cache, and a second run
/// at the same allowance re-reads nothing and says it again — while a run
/// under a larger allowance re-reads it and says nothing
/// (`docs/design/decisions.md`, "D85").
///
/// **Not vacuous**: the same dump under an allowance that fits gathers every
/// block and prints no decline, so the allowance and nothing else declined it.
#[test]
fn a_memory_allowance_too_small_declines_the_block_and_only_a_larger_one_rereads_it() {
    let (_dir, dump) = sandboxed(DUMP, "decline.sql");
    let source = dump.to_str().unwrap();
    let parse = |extra: &[&str]| {
        let mut args = vec!["parse", "--source", source];
        args.extend_from_slice(extra);
        let out = run(&args);
        assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
        stderr_of(&out)
    };

    // At and below `MEMORY_UNPOOLED_BOUND × 100 / (100 − MEMORY_MARGIN_PERCENT)`
    // the margin ceiling is zero on its own, so it leaves the statistics
    // nothing whatever the budget: every block declines, whatever its width.
    // 300 MiB is well inside that band — the line itself is 320 MiB, and where
    // it falls is pinned by `pgdump_query::io`'s own unit test — so nothing
    // here rests on which way `(a / 100) * 80` rounds.
    let tight = ["--memory", "314572800"];
    let stderr = parse(&tight);
    assert!(stderr.contains("statistics_bytes=0 (stated)"), "{stderr}");
    assert!(stderr.contains("statistics declined"), "{stderr}");
    assert!(stderr.contains("declined_under_bytes=0 allowance_bytes=0"), "{stderr}");
    let blocks = blocks_of(&info_json(&dump));
    assert!(!blocks.is_empty());
    for (table, copy) in &blocks {
        assert_eq!(copy["statistics"], Value::Null, "{table}");
        assert_eq!(copy["statistics_declined"], 0, "{table}");
    }

    // The same allowance leaves them alone and says so again.
    let stderr = parse(&tight);
    assert!(!stderr.contains("statistics back-fill started"), "no re-read: {stderr}");
    assert!(stderr.contains("statistics declined"), "{stderr}");

    // A larger one re-reads every one of them, and nothing declines.
    let stderr = parse(&["--memory", "8589934592"]);
    assert!(
        stderr.contains(&format!("statistics back-fill complete blocks={}", blocks.len())),
        "{stderr}"
    );
    assert!(!stderr.contains("statistics declined"), "{stderr}");
    for (table, copy) in &blocks_of(&info_json(&dump)) {
        assert_ne!(copy["statistics"], Value::Null, "{table}");
        assert_eq!(copy["statistics_declined"], Value::Null, "{table}");
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
    assert_eq!(default_text["bounds"]["sortedness"], "Unsorted", "bytewise, not the server's");
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
            Some("database: pgdt_tenant"),
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

    let (_, stderr) = parse(&["--statistics-min-rows", "16"]);
    assert!(stderr.contains("statistics back-fill complete blocks=3"), "{stderr}");
    let (_, stderr) = parse(&["--statistics-min-rows", "16"]);
    assert_eq!(backfill_lines(&stderr), 0, "{stderr}");
    let (_, stderr) = parse(&[]);
    assert_eq!(backfill_lines(&stderr), 0, "an unstated minimum keeps a block's size: {stderr}");
}

/// **A `parse` says what its statistics held, in one status line as it
/// returns**: the total then, the peak and each term's peak, the terms
/// telling a gathered pass from a loaded one. A `parse` holding none — nothing
/// gathered and none cached — prints no such line, and neither does a `query`.
#[test]
fn a_parse_says_what_its_statistics_held() {
    let (_dir, dump) = sandboxed(DUMP, "statistics.sql");
    let source = dump.to_str().unwrap();
    let held = |args: &[&str]| -> Vec<String> {
        let out = run(args);
        assert!(out.status.success(), "{args:?}: {}", stderr_of(&out));
        let stderr = stderr_of(&out);
        stderr.lines().filter(|l| l.contains(" statistics held ")).map(str::to_owned).collect()
    };
    let field = |line: &str, key: &str| -> u64 {
        let key = format!("{key}=");
        let value = line.split_whitespace().find_map(|word| word.strip_prefix(key.as_str()));
        value.unwrap_or_else(|| panic!("{line}: no {key}")).parse().unwrap()
    };

    assert_eq!(held(&["parse", "--source", source, "--statistics", "none"]), Vec::<String>::new());

    let gathered = held(&["parse", "--source", source]);
    let [line] = gathered.as_slice() else { panic!("{gathered:?}") };
    let (bytes, peak) = (field(line, "bytes"), field(line, "peak_bytes"));
    assert!(bytes > 0 && peak >= bytes, "{line}");
    assert_eq!(field(line, "retained_peak_bytes"), bytes, "{line}");
    assert_eq!(field(line, "loaded_peak_bytes"), 0, "{line}: the cache held none");
    assert!(field(line, "gathering_peak_bytes") > 0, "{line}");
    for term in ["gathering", "pieces", "interned"] {
        assert!(field(line, &format!("{term}_peak_bytes")) <= peak, "{line}");
    }

    let loaded = held(&["parse", "--source", source]);
    let [line] = loaded.as_slice() else { panic!("{loaded:?}") };
    assert_eq!(field(line, "loaded_peak_bytes"), field(line, "bytes"), "{line}");
    assert_eq!(field(line, "gathering_peak_bytes"), 0, "{line}: nothing lacked statistics");

    let queried = held(&["query", "--source", source, "--table", "public.ordered"]);
    assert_eq!(queried, Vec::<String>::new());
}

#[test]
fn contradictory_or_empty_statistics_flags_are_refused() {
    let (_dir, dump) = sandboxed(DUMP, "refused.sql");
    let source = dump.to_str().unwrap();
    for (extra, says) in [
        (&["--statistics", "none", "--statistics-group-size", "64"][..], "drop one of them"),
        (&["--statistics", "none", "--statistics-min-rows", "64"][..], "drop one of them"),
        (&["--statistics", "none", "--statistics-max-rows", "64"][..], "drop one of them"),
        (
            &["--statistics-max-rows", "64"][..],
            "1024 rows --statistics-min-rows asks for by default",
        ),
        (
            &["--statistics-min-rows", "64", "--statistics-max-rows", "8"][..],
            "below the 64 rows --statistics-min-rows asks for —",
        ),
        (
            &["--statistics-group-size", "4096", "--statistics-max-rows", "8"][..],
            "cannot be used with",
        ),
        (&["--preamble-only", "--statistics-max-rows", "8"][..], "cannot be used with"),
        (&["--statistics-group-size", "0"][..], "a group size of 0"),
        (&["--statistics-group-size", "1000"][..], "a power of two"),
        (
            &["--statistics-group-size", "4096", "--statistics-min-rows", "8"][..],
            "cannot be used with",
        ),
        (&["--preamble-only", "--statistics-min-rows", "8"][..], "cannot be used with"),
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
    assert!(!dump.with_extension("sql.dtcache").exists(), "a refusal scans nothing");
}

/// **`query` skips what the statistics rule out, says so, and prints what
/// `--statistics none` prints**, serially and split: `id` ascends, so a range
/// near its top reads a few of the groups a 1024-byte size cuts the table
/// into.
///
/// **The split leg is a real split**, asserted rather than assumed: the plan
/// narrows the batch span to seat the readers asked for and says how many it
/// planned (`docs/design/decisions.md`, "D84"), which is the only thing a CLI
/// leg can read the sub-stream count off. Without it a leg that silently
/// planned one would test the serial path twice.
#[test]
fn query_skips_the_groups_its_statistics_rule_out_unless_told_none() {
    let (_dir, dump) = sandboxed(DUMP, "pruned.sql");
    let source = dump.to_str().unwrap();
    run_ok(&["parse", "--source", source, "--statistics-group-size", "1024"]);
    for jobs in ["1", "3"] {
        let query = |extra: &[&str]| {
            let mut args = vec![
                "query",
                "--source",
                source,
                "--table",
                "public.ordered",
                "--where",
                "id >= 990 or id < 3",
                "--jobs",
                jobs,
                // A stated allowance buys a plain source no more read
                // buffers than the library's own constant
                // (`docs/design/decisions.md`, "D83"), so what splits this
                // is the batch span the plan spends to seat the readers
                // asked for ("D84") — three sub-streams at `--jobs 3`,
                // asserted below off the note that says so.
                "--memory",
                "1073741824",
            ];
            args.extend_from_slice(extra);
            let out = run(&args);
            assert!(out.status.success(), "{args:?}: {}", stderr_of(&out));
            (common::stdout_of(&out), stderr_of(&out))
        };
        let (pruned, said) = query(&[]);
        let (unpruned, unsaid) = query(&["--statistics", "none"]);
        assert_eq!(said.contains(&format!("{jobs} sub-stream(s) were planned")), jobs != "1");
        assert_eq!(pruned, unpruned, "--jobs {jobs}");
        assert_eq!(pruned.lines().count(), 1 + 13, "--jobs {jobs}: {pruned}");
        let note = said.lines().find(|l| l.starts_with("note: row-group statistics rule out"));
        assert!(note.is_some(), "--jobs {jobs}: {said}");
        assert!(!note.unwrap().contains(" 0 of"), "--jobs {jobs}: {said}");
        // A skip quotes no budget, so it takes no provenance clause — the
        // other half of what the narrowing note beside it carries
        // (`pgdump_query::PlanNote::budget_bytes`).
        assert!(!note.unwrap().contains("the budget in force is"), "--jobs {jobs}: {said}");
        assert!(!unsaid.contains("row-group statistics"), "--jobs {jobs}: {unsaid}");
    }
}

/// **`query` says what an early stop left unread, once, and only where one
/// fired**: at the shipped group size `ordered` is one group, so `id < 20`
/// skips nothing and stops at `20` — one block, however many sub-streams its
/// pieces went to — while `id <= 1000`, planned and never passed, and
/// `--statistics none` print no such note.
///
/// **The split leg is a real split**, read off the narrowing note as its
/// sibling above reads it (`docs/design/decisions.md`, "D84"): a stop summed
/// once over sub-streams that turned out to be one is not the property under
/// test.
#[test]
fn query_notes_what_an_early_stop_left_unread_only_where_one_fired() {
    let (_dir, dump) = sandboxed(DUMP, "stopped.sql");
    let source = dump.to_str().unwrap();
    run_ok(&["parse", "--source", source]);
    for jobs in ["1", "3"] {
        let query = |filter: &str, extra: &[&str]| {
            let mut args = vec![
                "query",
                "--source",
                source,
                "--table",
                "public.ordered",
                "--where",
                filter,
                "--jobs",
                jobs,
                // A stated allowance buys a plain source no more read
                // buffers than the library's own constant
                // (`docs/design/decisions.md`, "D83"), so what splits this
                // is the batch span the plan spends to seat the readers
                // asked for ("D84") — three sub-streams at `--jobs 3`,
                // asserted below off the note that says so.
                "--memory",
                "1073741824",
            ];
            args.extend_from_slice(extra);
            let out = run(&args);
            assert!(out.status.success(), "{args:?}: {}", stderr_of(&out));
            (common::stdout_of(&out), stderr_of(&out))
        };
        let stop_note = |said: &str| {
            said.lines()
                .filter(|l| l.starts_with("note: reading stopped early in"))
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        let (stopped, said) = query("id < 20", &[]);
        let (unstopped, unsaid) = query("id < 20", &["--statistics", "none"]);
        assert_eq!(said.contains(&format!("{jobs} sub-stream(s) were planned")), jobs != "1");
        assert_eq!(stopped, unstopped, "--jobs {jobs}");
        let notes = stop_note(&said);
        assert_eq!(notes.len(), 1, "--jobs {jobs}: {said}");
        assert!(notes[0].contains(" 1 block(s) "), "--jobs {jobs}: {said}");
        assert!(!notes[0].contains(" 0 byte(s)"), "--jobs {jobs}: {said}");
        assert!(stop_note(&unsaid).is_empty(), "--jobs {jobs}: {unsaid}");
        let (_, whole) = query("id <= 1000", &[]);
        assert!(stop_note(&whole).is_empty(), "--jobs {jobs}: {whole}");
        assert!(
            whole.contains("note: row-group statistics rule out 0 of"),
            "--jobs {jobs}: {whole}"
        );
    }
}
