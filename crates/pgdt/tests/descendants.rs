//! `pgdt query` and `pgdt sql` read a table with its descendants, and alone,
//! held to the server's answers over a restore of the same dump
//! (`docs/design/decisions.md`, "D130", "D131", "D132"): a partition written
//! through its root refused by `query` and left out of `sql`'s catalog, and
//! its root too when read alone (`KD158`), the remedy spelled in each front
//! end's words.

mod common;

use common::{Answer, Reading, answers, inheritance_fixture, run, sandboxed};

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
/// under their own names: every one below the root, whose rows are written
/// through it, and read alone the root too.
fn through_root(flag_set: &str, table: &str, reading: Reading) -> bool {
    let below = ["public.reading_2020", "public.reading_2020_north"]
        .into_iter()
        .chain(["public.reading_2020_south", "public.reading_2021"]);
    let root = (reading == Reading::Only).then_some("public.reading");
    flag_set != "default" && below.chain(root).any(|t| t == table)
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

/// **A partition root read alone through `STORED AS PGDUMP` refuses**, its
/// remedy spelled as the statement's option.
#[test]
fn pgdt_sql_spells_a_root_read_alone_as_its_option() {
    let (_dir, dump) = sandboxed(&inheritance_fixture(18, "load-via-partition-root"), "dump.sql");
    let dump = dump.to_str().unwrap();
    common::run_ok(&["parse", "--source", dump]);
    let create = format!(
        "CREATE EXTERNAL TABLE r STORED AS PGDUMP LOCATION '{dump}' \
         OPTIONS ('pgdump.table' 'reading', 'pgdump.inheritance' 'only')"
    );
    let out = run(&["sql", "-q", "-c", &create]);
    let said = text(&out.stdout) + &text(&out.stderr);
    assert!(said.contains("partition root `public.reading`"), "{said}");
    assert!(said.contains("(`'pgdump.inheritance' 'descendants'`)"), "{said}");
}
