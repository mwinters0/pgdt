//! A table read with its descendants, held to the server's answers over a
//! restore of the same dump (`docs/design/decisions.md`, "D130", "D131"):
//! every table the restore holds, `INHERITS` children and partitions alike,
//! listed and read as `SELECT … FROM <table>` reads it — over a complete map
//! and by a cold query's pass — or refused where a `--load-via-partition-root`
//! dump writes its rows through its root (`KD158`).

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use pgdt_codec::SchemaMode;
use pgdt_core::Error;
use pgdt_dump::ReadOptions;
use pgdt_io::{ByteRangeSource, LocalFileSource};
use pgdt_mapping::statistics::StatisticsRequest;
use pgdt_model::TableName;
use pgdt_query::{Dump, Expr, Predicate, PredicateOp, QueryOptions, Table, TableSelector};

use common::{
    Answer, Cold, Reading, VERSIONS, answers, inheritance_fixture, mapped, partitions_fixture,
    rows_of,
};

const FLAG_SETS: [&str; 2] = ["default", "load-via-partition-root"];

/// Tables whose columns are all generated: the server computes them, and a
/// dump carries no value of one (I5), so a read here has none to answer.
const GENERATED: [&str; 1] = ["public.derived"];

fn source(path: &Path) -> Arc<dyn ByteRangeSource> {
    Arc::new(LocalFileSource::open(path).unwrap())
}

/// A field as `COPY … TO STDOUT` writes it in text format.
fn copy_text(field: Option<String>) -> String {
    let Some(field) = field else { return "\\N".to_string() };
    let mut out = String::with_capacity(field.len());
    for c in field.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// A read of `table` under `schema_mode`, its rows as the server's `COPY`
/// text, sorted — or its refusal.
async fn read(table: &Table, schema_mode: SchemaMode) -> pgdt_core::Result<Vec<String>> {
    let options = QueryOptions { schema_mode, ..QueryOptions::default() };
    let plan = table.plan(ReadOptions::default(), options)?;
    let mut stream = plan.in_file_order(8192);
    let mut rows = Vec::new();
    while let Some(batch) = stream.next().await {
        for row in rows_of(&batch?) {
            rows.push(row.into_iter().map(copy_text).collect::<Vec<_>>().join("\t"));
        }
    }
    rows.sort();
    Ok(rows)
}

/// The listed table `qualified` names, in the one database a plain dump has.
fn listed<'a>(dump: &'a Dump, qualified: &str) -> &'a TableName {
    dump.tables()
        .iter()
        .find(|name| name.qualified_name() == qualified)
        .unwrap_or_else(|| panic!("{qualified} is not listed: {:?}", dump.tables()))
}

/// Every table `path`'s restore answers, read with its descendants over a
/// complete map in both schema modes and held to the server's rows and its
/// summary's count; the names of those refused as written through their
/// partition root.
async fn held_to_answers(path: &Path) -> BTreeSet<String> {
    let answers = answers(path);
    assert!(answers.restore_errors.is_empty(), "{}: {:?}", path.display(), answers.restore_errors);
    let dump = mapped(source(path), &StatisticsRequest::DATA).await;
    let mut refused = BTreeSet::new();
    for ((qualified, reading), answer) in &answers.tables {
        if *reading != Reading::Descendants {
            continue;
        }
        let Answer::Rows(expected) = answer else {
            panic!("{}: {qualified} is refused by the server: {answer:?}", path.display());
        };
        let name = listed(&dump, qualified);
        if GENERATED.contains(&qualified.as_str()) {
            continue;
        }
        let table = dump.table(&TableSelector::from(name)).unwrap().unwrap();
        for schema_mode in [SchemaMode::Typed, SchemaMode::Strings] {
            match read(&table, schema_mode).await {
                Ok(rows) => {
                    assert_eq!(&rows, expected, "{} {qualified} {schema_mode:?}", path.display())
                }
                Err(Error::PartitionThroughRoot { table, .. }) => {
                    assert_eq!(&table, qualified);
                    refused.insert(table);
                    continue;
                }
                Err(error) => panic!("{} {qualified} {schema_mode:?}: {error}", path.display()),
            }
            let options = QueryOptions { schema_mode, ..QueryOptions::default() };
            let summary = table.summary(&options).unwrap();
            assert_eq!(summary.rows, expected.len() as u64, "{} {qualified}", path.display());
        }
    }
    refused
}

/// The inheritance schema: every table answers as its restore does — three
/// levels of `INHERITS` with rows at each, a child in another schema, a
/// child of two parents read under both, a child's own column projected
/// away, and a partitioned table, which holds no block, read through a
/// partition itself partitioned — and under `--load-via-partition-root`
/// every partition below the root refuses while the root reads whole.
#[tokio::test]
async fn an_inherited_table_reads_its_descendants_as_its_restore_does() {
    let through_root: BTreeSet<String> =
        ["public.reading_2020", "public.reading_2020_north", "public.reading_2020_south"]
            .into_iter()
            .chain(["public.reading_2021"])
            .map(str::to_string)
            .collect();
    for version in VERSIONS {
        for flag_set in FLAG_SETS {
            let refused = held_to_answers(&inheritance_fixture(version, flag_set)).await;
            let expected =
                if flag_set == "default" { BTreeSet::new() } else { through_root.clone() };
            assert_eq!(refused, expected, "{version} {flag_set}");
        }
    }
}

/// The partitions schema: each root reads its partitions, and the only
/// tables refused are partitions whose rows `pg_dump` wrote through their
/// root — by the flag, or forced for a hash key on an enum.
#[tokio::test]
async fn a_partitioned_table_reads_its_partitions_as_its_restore_does() {
    for version in VERSIONS {
        for flag_set in FLAG_SETS {
            let path = partitions_fixture(version, flag_set);
            let refused = held_to_answers(&path).await;
            let forced = ["public.feel_a", "public.feel_z", "public.shuffle_a", "public.shuffle_z"]
                .into_iter()
                .chain(["public.spread_a", "public.spread_z"]);
            let by_flag = ["public.evt_a", "public.evt_z"];
            let mut expected: BTreeSet<String> = forced.map(str::to_string).collect();
            if flag_set != "default" {
                expected.extend(by_flag.map(str::to_string));
            }
            assert_eq!(refused, expected, "{version} {flag_set}");
        }
    }
}

/// A cold query's pass reads what a complete map's table reads: a parent
/// runs to the end of the file for its descendants' blocks, a partitioned
/// table holding none is found by its descendants', and a partition written
/// through its root refuses.
#[tokio::test]
async fn a_cold_query_reads_a_table_s_descendants() {
    for version in VERSIONS {
        for flag_set in FLAG_SETS {
            let path = inheritance_fixture(version, flag_set);
            let answers = answers(&path);
            for ((qualified, reading), answer) in &answers.tables {
                if *reading != Reading::Descendants {
                    continue;
                }
                let Answer::Rows(expected) = answer else { unreachable!() };
                let cold = Cold::default();
                let table = cold.table(source(&path), qualified, SchemaMode::Typed).await;
                let table = table.unwrap().unwrap_or_else(|| panic!("{qualified} not found cold"));
                match read(&table, SchemaMode::Typed).await {
                    Ok(rows) => assert_eq!(&rows, expected, "{version} {flag_set} {qualified}"),
                    Err(Error::PartitionThroughRoot { .. }) => {
                        assert_ne!(flag_set, "default", "{version} {qualified}");
                    }
                    Err(error) => panic!("{version} {flag_set} {qualified}: {error}"),
                }
            }
        }
    }
}

/// A filter or a projection naming a descendant's own column refuses a read
/// of its parent, as the server's would — the column is not the parent's,
/// though every row read from the descendant's block carries it.
#[tokio::test]
async fn a_descendant_s_own_column_is_not_its_parent_s() {
    let path = inheritance_fixture(18, "default");
    let dump = mapped(source(&path), &StatisticsRequest::DATA).await;
    let animal = dump.table(&TableSelector::named("animal").in_schema("public")).unwrap().unwrap();
    let projected =
        QueryOptions { projection: Some(vec!["breed".into()]), ..QueryOptions::default() };
    assert!(matches!(
        animal.plan(ReadOptions::default(), projected).map(|_| ()),
        Err(Error::UnknownProjectionColumn { column, .. }) if column == "breed"
    ));
    let breed = Predicate { column: "breed".into(), op: PredicateOp::IsNotNull };
    let filtered = QueryOptions { filter: Expr::Term(breed), ..QueryOptions::default() };
    assert!(matches!(
        animal.plan(ReadOptions::default(), filtered).map(|_| ()),
        Err(Error::UnknownPredicateColumn { column, .. }) if column == "breed"
    ));
    let pet = dump.table(&TableSelector::named("pet").in_schema("public")).unwrap().unwrap();
    let schema = pet.schema(&QueryOptions::default()).unwrap();
    let names: Vec<&str> = schema.schema().fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(names, ["owner"]);
}
