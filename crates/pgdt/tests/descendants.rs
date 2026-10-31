//! `pgdt query` and `pgdt sql` read a table with its descendants, and alone,
//! held to the server's answers over a restore of the same dump
//! (`docs/design/decisions.md`, "D130", "D131", "D132"): a partition written
//! through its root refused by `query` and left out of `sql`'s catalog
//! (`KD158`), the remedy spelled in each front end's words, and a partitioned
//! table read alone reading none (I93), and both modes read in one statement
//! by one dump attached twice; `pgdt info --json` stating what a
//! TimescaleDB database's catalog concludes, and its listing opening with the
//! logical model, a hypertable's chunks folded under it.

mod common;

use common::{Answer, Reading, answers, inheritance_fixture, run, sandboxed, timescaledb_fixture};

const FLAG_SETS: [&str; 2] = ["default", "load-via-partition-root"];

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The mode's word, as each front end spells it.
fn mode(reading: Reading) -> &'static str {
    match reading {
        Reading::Descendants => "descendants",
        Reading::Only => "only",
    }
}

/// Every table the restore answers, each way the server read it: the
/// server's rows, or a refusal where the rows are written through a
/// partition root.
fn tables(flag_set: &str) -> Vec<(String, Reading, Vec<String>)> {
    answers(&inheritance_fixture(18, flag_set))
        .tables
        .into_iter()
        .map(|((table, reading), answer)| match answer {
            Answer::Rows(rows) => (table, reading, rows),
            Answer::Error(error) => panic!("{table} is refused by the server: {error}"),
        })
        .collect()
}

/// The tables a `--load-via-partition-root` dump of this schema refuses
/// under their own names: every leaf below the root, whose rows are written
/// through it, and read with its descendants the partitioned table between.
fn through_root(flag_set: &str, table: &str, reading: Reading) -> bool {
    let leaves = ["public.reading_2020_north", "public.reading_2020_south", "public.reading_2021"];
    let between = (reading == Reading::Descendants).then_some("public.reading_2020");
    flag_set != "default" && leaves.into_iter().chain(between).any(|t| t == table)
}

#[test]
fn pgdt_query_reads_a_table_s_descendants() {
    for flag_set in FLAG_SETS {
        let (_dir, dump) = sandboxed(&inheritance_fixture(18, flag_set), "dump.sql");
        let dump = dump.to_str().unwrap();
        for (table, reading, expected) in tables(flag_set) {
            let args = ["query", "--source", dump, "--table", &table];
            let out = run(&[&args[..], &["--inheritance", mode(reading)]].concat());
            let at = format!("{flag_set} {table} {reading:?}");
            if through_root(flag_set, &table, reading) {
                let said = text(&out.stderr);
                assert!(!out.status.success(), "{at} read");
                assert!(said.contains("written through"), "{at}: {said}");
                // Read alone, the remedy is the root read with its
                // descendants, which the flag spells.
                let spelled = said.contains("(`--inheritance descendants`)");
                assert_eq!(spelled, reading == Reading::Only, "{at}: {said}");
                continue;
            }
            assert!(out.status.success(), "{at}: {}", text(&out.stderr));
            // A header line where any row was printed, then the rows.
            let stdout = text(&out.stdout);
            let mut rows: Vec<String> = stdout.lines().skip(1).map(str::to_string).collect();
            rows.sort();
            assert_eq!(rows, expected, "{at}");
        }
    }
}

#[test]
fn pgdt_sql_reads_a_table_s_descendants() {
    for flag_set in FLAG_SETS {
        let (_dir, dump) = sandboxed(&inheritance_fixture(18, flag_set), "dump.sql");
        let dump = dump.to_str().unwrap();
        common::run_ok(&["parse", "--source", dump]);
        for (table, reading, expected) in tables(flag_set) {
            let sql = format!("SELECT count(*) AS n FROM d.{table}");
            let source = format!("d={dump}:inheritance={}", mode(reading));
            let args = ["sql", "--format", "csv", "-q", "--source", &source, "-c", &sql];
            let out = run(&args);
            let at = format!("{flag_set} {table} {reading:?}");
            if through_root(flag_set, &table, reading) {
                let said = text(&out.stdout) + &text(&out.stderr);
                assert!(said.contains("not found"), "{at}: {said}");
                continue;
            }
            assert!(out.status.success(), "{at}: {}", text(&out.stderr));
            assert_eq!(text(&out.stdout).trim(), format!("n\n{}", expected.len()), "{at}");
        }
    }
}

/// **Both modes in one statement are one dump attached twice**, once in each
/// (`docs/manual/descendants.md`, "Both in one statement: the dump registered
/// twice"): a join of each table's count under one catalog and under the
/// other holds each side to its own oracle, and a table either refuses is in
/// neither catalog.
#[test]
fn pgdt_sql_reads_both_modes_by_one_dump_attached_twice() {
    for flag_set in FLAG_SETS {
        let (_dir, dump) = sandboxed(&inheritance_fixture(18, flag_set), "dump.sql");
        let dump = dump.to_str().unwrap();
        common::run_ok(&["parse", "--source", dump]);
        let whole = format!("whole={dump}");
        let own = format!("own={dump}:inheritance=only");
        let all = tables(flag_set);
        let count = |table: &str, reading| {
            all.iter().find(|(t, r, _)| t == table && *r == reading).map(|(_, _, rows)| rows.len())
        };
        let mut differ = 0;
        for (table, _, _) in all.iter().filter(|(_, r, _)| *r == Reading::Descendants) {
            let either = [Reading::Descendants, Reading::Only];
            if either.into_iter().any(|r| through_root(flag_set, table, r)) {
                continue;
            }
            let sql = format!(
                "SELECT w.n AS whole, o.n AS own \
                 FROM (SELECT count(*) AS n FROM whole.{table}) w \
                 CROSS JOIN (SELECT count(*) AS n FROM own.{table}) o"
            );
            let args = ["sql", "--format", "csv", "-q", "--source", &whole, "--source", &own];
            let out = run(&[&args[..], &["-c", &sql]].concat());
            let at = format!("{flag_set} {table}");
            assert!(out.status.success(), "{at}: {}", text(&out.stdout) + &text(&out.stderr));
            let expected = (count(table, Reading::Descendants), count(table, Reading::Only));
            let (Some(through), Some(alone)) = expected else { panic!("{at}: {expected:?}") };
            assert_eq!(text(&out.stdout).trim(), format!("whole,own\n{through},{alone}"), "{at}");
            differ += usize::from(through != alone);
        }
        assert!(differ > 0, "{flag_set}: no table's two answers differ");
    }
}

/// **A partition written through its root, read alone through `STORED AS
/// PGDUMP`, refuses**, its remedy — the root read with its descendants —
/// spelled as the statement's option.
#[test]
fn pgdt_sql_spells_a_partition_read_alone_as_its_option() {
    let (_dir, dump) = sandboxed(&inheritance_fixture(18, "load-via-partition-root"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let create = format!(
        "CREATE EXTERNAL TABLE r STORED AS PGDUMP LOCATION '{dump}' \
         OPTIONS ('pgdump.table' 'reading_2021', 'pgdump.inheritance' 'only')"
    );
    let out = run(&["sql", "-q", "-c", &create]);
    let said = text(&out.stdout) + &text(&out.stderr);
    assert!(said.contains("partition root `public.reading`"), "{said}");
    assert!(said.contains("(`'pgdump.inheritance' 'descendants'`)"), "{said}");
}

/// `info --json` states each TimescaleDB database's catalog read by its
/// recipe, beside the rows the catalog's blocks recorded, whatever level the
/// parse mapped them at — here a dump holding the catalog without the
/// extension.
#[test]
fn the_json_export_states_a_timescaledb_catalog() {
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "tables"), "dump.sql");
    let dump = dump.to_str().unwrap();
    let parse = run(&["parse", "--source", dump, "--statistics-level", "metadata"]);
    assert!(parse.status.success(), "{}", text(&parse.stderr));
    let info = run(&["info", "--source", dump, "--json"]);
    assert!(info.status.success(), "{}", text(&info.stderr));
    let json: serde_json::Value = serde_json::from_slice(&info.stdout).expect("--json emits JSON");
    let [db] = json["timescaledb"].as_array().expect("one TimescaleDB database").as_slice() else {
        panic!("{}", json["timescaledb"])
    };
    assert_eq!(db["extension"], false);
    assert_eq!(db["version"]["source"], "stated");
    assert_eq!(db["version"]["version"], "2.31.0");
    assert_eq!(db["resolution"]["state"], "resolved");
    assert_eq!(db["resolution"]["recipe"], "relid");
    let hypertables = db["resolution"]["hypertables"].as_array().unwrap();
    let metrics = hypertables.iter().find(|h| h["table"] == "public.metrics").expect("listed");
    assert!(metrics["chunks"].as_array().is_some_and(|c| !c.is_empty()));
}

/// **A hypertable reaching a compressed chunk refuses in each front end**:
/// `query` naming the chunk, under either mode for the chunk read by its own
/// name, and `sql` leaving each out of its catalog; read alone, the
/// hypertable reads its root's own rows, as its restore holds them (TS8).
#[test]
fn a_compressed_chunk_refuses_in_each_front_end() {
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "default"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let chunk = "_timescaledb_internal._hyper_1_2_chunk";
    for (table, reading) in
        [("public.metrics", "descendants"), (chunk, "descendants"), (chunk, "only")]
    {
        let at = format!("{table} {reading}");
        let out = run(&["query", "--source", dump, "--table", table, "--inheritance", reading]);
        let said = text(&out.stderr);
        assert!(!out.status.success(), "{at} read");
        assert!(said.contains("compressed") && said.contains(chunk), "{at}: {said}");
        let sql = format!("SELECT count(*) AS n FROM d.{table}");
        let source = format!("d={dump}:inheritance={reading}");
        let out = run(&["sql", "--format", "csv", "-q", "--source", &source, "-c", &sql]);
        let said = text(&out.stdout) + &text(&out.stderr);
        assert!(said.contains("not found"), "{at}: {said}");
    }
    let held = answers(&timescaledb_fixture(18, "default")).held;
    let Some(Answer::Rows(rows)) = held.get("public.metrics") else { panic!("{held:?}") };
    let sql = "SELECT count(*) AS n FROM d.public.metrics";
    let source = format!("d={dump}:inheritance=only");
    let out = run(&["sql", "--format", "csv", "-q", "--source", &source, "-c", sql]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout).trim(), format!("n\n{}", rows.len()));
}

/// **`pgdt query` says what the map finds of a hypertable it reads**, though
/// it prints no file-level finding (D135): read with its descendants, the
/// rows its root holds and the chunks the file lacks; read alone at 2.31,
/// whose `ONLY` reads it empty (TS8), its root's rows alone.
#[test]
fn pgdt_query_says_a_hypertable_s_findings() {
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "exclude-chunks"), "dump.sql");
    let dump = dump.to_str().unwrap();
    let root =
        "warning: this read of TimescaleDB hypertable `public.metrics` includes the 1 row(s)";
    let absent = "warning: this read of TimescaleDB hypertable `public.metrics` holds no row of";
    for (reading, absent_said) in [("descendants", true), ("only", false)] {
        let args = ["query", "--source", dump, "--table", "public.metrics", "--inheritance"];
        let out = run(&[&args[..], &[reading]].concat());
        let said = text(&out.stderr);
        assert!(out.status.success(), "{reading}: {said}");
        assert!(said.contains(root), "{reading}: {said}");
        assert_eq!(said.contains(absent), absent_said, "{reading}: {said}");
    }
}

/// The lines of `info`'s section headed `heading`, to the blank line ending it.
fn section<'a>(listing: &'a str, heading: &str) -> Vec<&'a str> {
    let mut lines = listing.lines().skip_while(|l| *l != heading);
    assert!(lines.next().is_some(), "no `{heading}`: {listing}");
    lines.take_while(|l| !l.is_empty()).collect()
}

/// **`info` names each table with descendants, how many and their rows**,
/// each table's rows in all held to its server's answer read with its
/// descendants, whether a partition's rows are its own blocks' or written
/// through its root, which a block listing alone cannot say.
#[test]
fn info_names_each_table_with_descendants() {
    let descendants = [
        ("public.animal", 6),
        ("public.dog", 1),
        ("public.mammal", 3),
        ("public.pet", 1),
        ("public.reading", 4),
        ("public.reading_2020", 2),
    ];
    for flag_set in FLAG_SETS {
        let (_dir, dump) = sandboxed(&inheritance_fixture(18, flag_set), "dump.sql");
        let dump = dump.to_str().unwrap();
        common::run_ok(&["parse", "--source", dump]);
        let listing = common::run_ok(&["info", "--source", dump]);
        let answered = tables(flag_set);
        let expected: Vec<String> = descendants
            .iter()
            .map(|(table, count)| {
                let rows = answered
                    .iter()
                    .find(|(t, reading, _)| t == table && *reading == Reading::Descendants)
                    .map(|(_, _, rows)| rows.len())
                    .unwrap();
                format!("    {table}: {count} descendant(s), {rows} row(s) in all")
            })
            .collect();
        assert_eq!(section(&listing, "tables with descendants:"), expected, "{flag_set}");
    }
}

/// **`info` says what refuses a read, among its diagnostics**: the partition
/// root a `--load-via-partition-root` dump writes its partitions' rows
/// through, naming the partitions refused by name, and a hypertable's
/// compressed chunks, naming the first; a dump writing every row to its own
/// table says neither.
#[test]
fn info_says_what_refuses_a_read() {
    let through = "    [warning] partition root `public.reading` has its partitions' rows \
                   written through it, in the COPY blocks at offsets ";
    for flag_set in FLAG_SETS {
        let (_dir, dump) = sandboxed(&inheritance_fixture(18, flag_set), "dump.sql");
        let dump = dump.to_str().unwrap();
        common::run_ok(&["parse", "--source", dump]);
        let listing = common::run_ok(&["info", "--source", dump]);
        let found = section(&listing, "diagnostics:");
        let line = found.iter().find(|l| l.starts_with(through));
        assert_eq!(line.is_some(), flag_set != "default", "{flag_set}: {listing}");
        if let Some(line) = line {
            assert!(line.contains("each partition they hold, `public.reading_"), "{line}");
        }
    }
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "default"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let listing = common::run_ok(&["info", "--source", dump]);
    let compressed = "    [warning] TimescaleDB hypertable `public.metrics` has chunk \
                      `_timescaledb_internal._hyper_1_";
    let found = section(&listing, "diagnostics:");
    let line = found.iter().find(|l| l.starts_with(compressed));
    assert!(line.is_some_and(|l| l.contains("and 3 more whose rows are compressed")), "{listing}");
}

/// **`info` states each TimescaleDB database's release and what its catalog
/// concludes ahead of the blocks, and folds each chunk's block under its
/// hypertable's**, a compressed relation's beside them where it is no chunk
/// of its own (2.29 on), unless `--detail` lists every block; at 2.13 the
/// release is inferred, and the compressed relations are chunks of an
/// internal hypertable, folded under it.
#[test]
fn info_states_the_timescaledb_catalog_and_folds_chunks() {
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "default"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let listing = common::run_ok(&["info", "--source", dump]);
    let catalog = section(&listing, "TimescaleDB: 2.31.0, stated by its catalog");
    for line in [
        "    hypertable public.metrics: 6 chunk(s): 2 uncompressed, 4 compressed (2 partially), \
         0 absent from the file; dimensions time, device (hashed)",
        "    hypertable public.counters: 3 chunk(s): 3 uncompressed, 0 compressed, 0 absent from \
         the file; dimensions tick",
        "    continuous aggregate public.metrics_daily: materialized only, its rows read as \
         _timescaledb_internal._materialized_hypertable_2; the view is not read",
        "    continuous aggregate public.metrics_weekly: real-time, its rows read as \
         _timescaledb_internal._materialized_hypertable_3; the view is not read",
    ] {
        assert!(catalog.contains(&line), "{line}: {listing}");
    }
    // Before the blocks, which a chunk's no longer lists.
    let at = listing.find("TimescaleDB: ").unwrap();
    assert!(listing.find("public.metrics (1 rows)").is_some_and(|block| block > at), "{listing}");
    assert!(!listing.contains("\n_timescaledb_internal._hyper_1_2_chunk ("), "{listing}");
    let beneath = listing.lines().skip_while(|l| *l != "public.metrics (1 rows)").nth(1);
    let folded = "    chunks: 6 block(s), 44 row(s); compressed relations: 4 block(s), 6 row(s) — \
                  folded, `pgdt info --detail` lists each";
    assert_eq!(beneath, Some(folded), "{listing}");
    assert!(listing.contains("\n41 COPY block(s), "), "every block counted: {listing}");
    let detail = common::run_ok(&["info", "--source", dump, "--detail"]);
    assert!(detail.contains("\n_timescaledb_internal._hyper_1_2_chunk (0 rows)\n"), "{detail}");
    assert!(!detail.contains("folded"), "{detail}");

    let (_dir, dump) = sandboxed(&timescaledb_fixture(13, "default"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let listing = common::run_ok(&["info", "--source", dump]);
    let catalog = section(&listing, "TimescaleDB: 2.13, inferred from its catalog's columns");
    let internal = "    hypertable _timescaledb_internal._compressed_hypertable_4: internal, \
                    holding compressed chunks' rows; 4 chunk(s): 0 absent from the file";
    assert!(catalog.contains(&internal), "{listing}");
    let beneath = listing
        .lines()
        .skip_while(|l| !l.starts_with("_timescaledb_internal._compressed_hypertable_4 ("))
        .nth(1);
    assert!(beneath.is_some_and(|l| l.starts_with("    chunks: 4 block(s), ")), "{listing}");
}

/// **A chunk the catalog lists and the file lacks is counted absent**, and a
/// catalog that does not read says so, naming the hypertables its rows name,
/// its chunks then listed as blocks of their own, there being no chunk list
/// to fold them by.
#[test]
fn info_counts_absent_chunks_and_says_an_unreadable_catalog() {
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "exclude-chunks"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let listing = common::run_ok(&["info", "--source", dump]);
    let catalog = section(&listing, "TimescaleDB: 2.31.0, stated by its catalog");
    let metrics = catalog.iter().find(|l| l.starts_with("    hypertable public.metrics: "));
    let absent = "6 chunk(s): 0 uncompressed, 0 compressed, 6 absent from the file";
    assert!(metrics.is_some_and(|l| l.contains(absent)), "{listing}");

    let (dir, dump) = sandboxed(&timescaledb_fixture(18, "default"), "dump.sql");
    let slice = "COPY _timescaledb_catalog.dimension_slice (id, chunk_id,";
    let text = std::fs::read_to_string(&dump).unwrap();
    assert_eq!(text.matches(slice).count(), 1);
    let edited = dir.path().join("edited.sql");
    let renamed = "COPY _timescaledb_catalog.dimension_slice (id, chunk_ident,";
    std::fs::write(&edited, text.replace(slice, renamed)).unwrap();
    let edited = edited.to_str().unwrap();
    common::run_ok(&["parse", "--source", edited]);
    let listing = common::run_ok(&["info", "--source", edited]);
    let catalog = section(&listing, "TimescaleDB: 2.31.0, stated by its catalog");
    let unreadable = "    catalog: does not read — `diagnostics:` above says why; its rows name \
                      hypertable(s) ";
    let line = catalog.iter().find(|l| l.starts_with(unreadable));
    assert!(line.is_some_and(|l| l.contains("public.metrics")), "{listing}");
    assert!(listing.contains("\n_timescaledb_internal._hyper_1_2_chunk (0 rows)\n"), "{listing}");
}

/// **A catalog held without the extension says so**: a dump selecting tables
/// by `--table` writes the catalog's blocks and no `CREATE EXTENSION` (TS2),
/// and its catalog concludes what the whole dump's does.
#[test]
fn info_says_a_catalog_held_without_its_extension() {
    let (_dir, dump) = sandboxed(&timescaledb_fixture(18, "tables"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let listing = common::run_ok(&["info", "--source", dump]);
    let catalog = section(&listing, "TimescaleDB: 2.31.0, stated by its catalog");
    assert_eq!(
        catalog.first().copied(),
        Some(
            "    extension: not created — the catalog alone, as a dump selecting tables by \
             `--table` writes it"
        ),
        "{listing}"
    );
    assert!(catalog.iter().any(|l| l.starts_with("    hypertable public.metrics: 6 chunk(s): ")));
}
