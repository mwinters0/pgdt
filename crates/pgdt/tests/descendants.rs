//! `pgdt query` and `pgdt sql` read a table with its descendants, held to
//! the server's answers over a restore of the same dump
//! (`docs/design/decisions.md`, "D130", "D131"): a partition written through
//! its root refused by `query` and left out of `sql`'s catalog (`KD158`).

mod common;

use common::{Answer, Reading, answers, inheritance_fixture, run, sandboxed};

const FLAG_SETS: [&str; 2] = ["default", "load-via-partition-root"];

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Every table the restore answers, read with its descendants: the server's
/// rows, or a refusal where the rows are written through a partition root.
fn tables(flag_set: &str) -> Vec<(String, Vec<String>)> {
    answers(&inheritance_fixture(18, flag_set))
        .tables
        .into_iter()
        .filter(|((_, reading), _)| *reading == Reading::Descendants)
        .map(|((table, _), answer)| match answer {
            Answer::Rows(rows) => (table, rows),
            Answer::Error(error) => panic!("{table} is refused by the server: {error}"),
        })
        .collect()
}

/// The partitions a `--load-via-partition-root` dump of this schema writes
/// through their root, refused under their own names.
fn through_root(flag_set: &str, table: &str) -> bool {
    let partitions = ["public.reading_2020", "public.reading_2020_north"]
        .into_iter()
        .chain(["public.reading_2020_south", "public.reading_2021"]);
    flag_set != "default" && partitions.into_iter().any(|t| t == table)
}

#[test]
fn pgdt_query_reads_a_table_s_descendants() {
    for flag_set in FLAG_SETS {
        let (_dir, dump) = sandboxed(&inheritance_fixture(18, flag_set), "dump.sql");
        let dump = dump.to_str().unwrap();
        for (table, expected) in tables(flag_set) {
            let out = run(&["query", "--source", dump, "--table", &table]);
            if through_root(flag_set, &table) {
                let said = text(&out.stderr);
                assert!(!out.status.success(), "{flag_set} {table} read");
                assert!(said.contains("written through its partition root"), "{table}: {said}");
                continue;
            }
            assert!(out.status.success(), "{flag_set} {table}: {}", text(&out.stderr));
            // A header line where any row was printed, then the rows.
            let stdout = text(&out.stdout);
            let mut rows: Vec<String> = stdout.lines().skip(1).map(str::to_string).collect();
            rows.sort();
            assert_eq!(rows, expected, "{flag_set} {table}");
        }
    }
}

#[test]
fn pgdt_sql_reads_a_table_s_descendants() {
    for flag_set in FLAG_SETS {
        let (_dir, dump) = sandboxed(&inheritance_fixture(18, flag_set), "dump.sql");
        let dump = dump.to_str().unwrap();
        common::run_ok(&["parse", "--source", dump]);
        for (table, expected) in tables(flag_set) {
            let sql = format!("SELECT count(*) AS n FROM d.{table}");
            let source = format!("d={dump}");
            let args = ["sql", "--format", "csv", "-q", "--source", &source, "-c", &sql];
            let out = run(&args);
            if through_root(flag_set, &table) {
                let said = text(&out.stdout) + &text(&out.stderr);
                assert!(said.contains("not found"), "{flag_set} {table}: {said}");
                continue;
            }
            assert!(out.status.success(), "{flag_set} {table}: {}", text(&out.stderr));
            assert_eq!(text(&out.stdout).trim(), format!("n\n{}", expected.len()), "{table}");
        }
    }
}
