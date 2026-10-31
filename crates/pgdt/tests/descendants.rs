//! `pgdt query` and `pgdt sql` read a table with its descendants, and alone,
//! held to the server's answers over a restore of the same dump
//! (`docs/design/decisions.md`, "D130", "D131", "D132"): a partition written
//! through its root refused by `query` and left out of `sql`'s catalog
//! (`KD158`), the remedy spelled in each front end's words, and a partitioned
//! table read alone reading none (I93); and `pgdt info --json` stating what a
//! TimescaleDB database's catalog concludes.

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
