//! `Expr::In` against the `Or` of `=` it replaces (`docs/design/decisions.md`,
//! "D53"): **the same rows, the same row groups skipped and the same error**,
//! over every column of the `types`, `statistics`, `edge_cases` and
//! `partitions` fixtures at every major, with generated lists and trees.
//!
//! The membership is answered by one lookup where the `Or` compares once per
//! value, and its statistics by the `Or`'s own combination, so the check is
//! that the lookup finds exactly what the comparisons would, per kind, and
//! that nothing else moved. Per row is the query's rows with statistics off;
//! per group is the pruning note and the early stops with them on, beside the
//! rows again; the error is whatever either raises, the membership naming its
//! operator `IN` where the `Or`'s term names `=`.
//!
//! **A NULL in a list is referred to a membership of that NULL alone**, the
//! `Or` of `=` having no NULL literal: `c IN (a, NULL)` against
//! `c = a OR c IN (NULL)`. What `c IN (NULL)` answers is pinned apart, by
//! `predicate.rs`'s own tests, against a term over a field the row lacks.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;
use futures::StreamExt;
use pgdump_query::cache::{self, CacheMode};
use pgdump_query::{
    ComparisonNote, ComparisonSemantics, CopyBlock, DumpIndex, EarlyStop, Expr, LocalFileSource,
    Membership, Parallelism, PlanNote, Predicate, PredicateOp, QueryOptions, ResolvedSchema,
    ScanOptions, StatisticsRequest, StatisticsSelection, map_file, table_stream_partitions,
};

mod common;
use common::{VERSIONS, fixture, fixtures_root, sandboxed};

/// The group size statistics are gathered at: tens of bytes, so a fixture's
/// block spans many groups and a membership has groups to skip.
const TINY_GROUP: u64 = 32;

/// Lists drawn per column, each asked bare and under `NOT`.
const LISTS_PER_COLUMN: usize = 3;

/// Random trees per table over the lists drawn for it.
const TREES_PER_TABLE: usize = 6;

/// The schemas swept, at each major's `default` flag set.
const SCHEMAS: [&str; 4] = ["types", "statistics", "edge_cases", "partitions"];

/// SplitMix64, seeded, so a failing list is the same list on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Everything a query was seen to do: its rows or its error, and beside
/// them what its plan skipped, where its sub-streams stopped, the schema and
/// the divergence notes, each note once.
#[derive(Debug, PartialEq)]
struct Seen {
    outcome: Result<Option<RecordBatch>, String>,
    plan: Vec<PlanNote>,
    stops: Vec<EarlyStop>,
    resolved: Option<(ResolvedSchema, Vec<ComparisonNote>)>,
}

/// `options` against `table`, drained in file order to the first error.
async fn seen(dump: &Path, cache: &Path, table: &str, options: QueryOptions) -> Seen {
    let source = LocalFileSource::open(dump).unwrap();
    let cache = CacheMode::enabled(cache.to_path_buf());
    let mut streams =
        match table_stream_partitions(&source, table, ScanOptions::default(), options, cache).await
        {
            Ok(streams) => streams,
            Err(e) => {
                return Seen {
                    outcome: Err(named_as_equality(&e.to_string())),
                    plan: Vec::new(),
                    stops: Vec::new(),
                    resolved: None,
                };
            }
        };
    let plan = streams[0].plan_notes();
    let (mut batches, mut failure) = (Vec::new(), None);
    'drain: for stream in &mut *streams {
        while let Some(batch) = stream.next().await {
            match batch {
                Ok(batch) => batches.push(batch),
                Err(e) => {
                    failure = Some(named_as_equality(&e.to_string()));
                    break 'drain;
                }
            }
        }
    }
    let stops = streams.iter().flat_map(|stream| stream.early_stops()).collect();
    let mut notes: Vec<ComparisonNote> = Vec::new();
    for note in streams[0].comparison_notes() {
        if !notes.contains(&note) {
            notes.push(note);
        }
    }
    let outcome = match failure {
        Some(e) => Err(e),
        None => Ok(batches.first().map(|first| concat_batches(&first.schema(), &batches).unwrap())),
    };
    Seen { outcome, plan, stops, resolved: Some((streams[0].resolved_schema(), notes)) }
}

/// An error's text with the membership's operator written as its terms'
/// would be — the one place the two are worded apart.
fn named_as_equality(error: &str) -> String {
    error.replace(" IN ...`", " = ...`").replace("`IN` on column", "`=` on column")
}

/// `dump` mapped into a cache beside a private copy of it, every statistic
/// gathered at [`TINY_GROUP`].
async fn gathered(dump: &Path) -> (tempfile::TempDir, PathBuf, DumpIndex) {
    let (dir, copy) = sandboxed(dump, "dump.sql");
    let source = LocalFileSource::open(&copy).unwrap();
    let request = StatisticsRequest {
        selection: StatisticsSelection::DATA,
        group_size: Some(NonZeroU64::new(TINY_GROUP).unwrap()),
        ..StatisticsRequest::DATA
    };
    let cache = CacheMode::enabled(cache::colocated_path(&copy));
    let run = map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    assert!(!run.interrupted);
    (dir, copy, run.index)
}

/// The values a column's lists draw from: its stored bounds and dictionary
/// entries, each also spelled with a trailing `0` — the same number for a
/// fraction, the next text or a refusal elsewhere — and the committed
/// oracle's values for its declared type (`docs/design/decisions.md`, "D70").
fn pool(
    blocks: &[&CopyBlock],
    column: usize,
    oracle: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut out = BTreeSet::new();
    let mut declared = None;
    for block in blocks {
        let Some(statistics) = block.statistics.as_deref() else { continue };
        let Some(Some(column)) = statistics.columns.get(column) else { continue };
        declared = declared.or(column.declared_type.clone());
        if let Some(bounds) = &column.bounds {
            for group in bounds.groups.iter().flatten() {
                out.insert(group.min.clone());
                out.insert(group.max.clone());
            }
        }
        if let Some(dictionary) = &column.dictionary {
            out.extend(dictionary.entries.iter().cloned());
        }
    }
    let respelled: Vec<String> = out.iter().map(|value| format!("{value}0")).collect();
    out.extend(respelled);
    if let Some(values) = declared.as_ref().and_then(|d| oracle.get(d)) {
        out.extend(values.iter().cloned());
    }
    out.into_iter().collect()
}

/// Each major's oracle literals by declared type.
fn oracle_literals(version: u32) -> BTreeMap<String, Vec<String>> {
    let path = fixtures_root().join(version.to_string()).join("oracle/literals.tsv");
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in std::fs::read_to_string(path).unwrap().lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        if let [declared, _, "ok", output] = fields[..]
            && output != "\\N"
        {
            out.entry(declared.to_string()).or_default().push(output.to_string());
        }
    }
    out
}

/// One membership and the `Or` it replaces, a NULL in the list referred to a
/// membership of that NULL alone.
fn pair(column: &str, values: Vec<Option<String>>) -> (Expr, Expr) {
    let disjunction = values
        .iter()
        .map(|value| match value {
            Some(v) => Expr::Term(Predicate {
                column: column.to_string(),
                op: PredicateOp::Eq,
                value: Some(v.clone()),
            }),
            None => Expr::In(Membership { column: column.to_string(), values: vec![None] }),
        })
        .collect();
    (Expr::In(Membership { column: column.to_string(), values }), Expr::Or(disjunction))
}

/// A list of one to six values from `pool`, a NULL among them one time in
/// four.
fn list(rng: &mut Rng, pool: &[String]) -> Vec<Option<String>> {
    let mut values: Vec<Option<String>> =
        (0..1 + rng.below(6)).map(|_| Some(pool[rng.below(pool.len())].clone())).collect();
    if rng.below(4) == 0 {
        let at = rng.below(values.len() + 1);
        values.insert(at, None);
    }
    values
}

/// A random tree over `leaves`, built twice in one shape: once over the
/// memberships, once over the disjunctions they replace.
fn tree(rng: &mut Rng, leaves: &[(Expr, Expr)], depth: usize) -> (Expr, Expr) {
    let children = |rng: &mut Rng| -> (Vec<Expr>, Vec<Expr>) {
        (0..1 + rng.below(3)).map(|_| tree(rng, leaves, depth - 1)).unzip()
    };
    match if depth == 0 { 0 } else { rng.below(4) } {
        0 => leaves[rng.below(leaves.len())].clone(),
        1 => {
            let (a, b) = children(rng);
            (Expr::And(a), Expr::And(b))
        }
        2 => {
            let (a, b) = children(rng);
            (Expr::Or(a), Expr::Or(b))
        }
        _ => {
            let (a, b) = tree(rng, leaves, depth - 1);
            (Expr::Not(Box::new(a)), Expr::Not(Box::new(b)))
        }
    }
}

#[derive(Default, Debug)]
struct Tally {
    compared: usize,
    kept_rows: usize,
    pruned: usize,
    errors: usize,
    with_null: usize,
}

/// `membership` against `disjunction`, statistics off and on.
async fn check(
    (dump, plain, gathered): (&Path, &Path, &Path),
    table: &str,
    base: &QueryOptions,
    (membership, disjunction): (Expr, Expr),
    tally: &mut Tally,
) {
    for (cache, use_statistics, jobs) in [(plain, false, 3), (gathered, true, 1)] {
        let options = |filter: &Expr| QueryOptions {
            filter: filter.clone(),
            use_statistics,
            parallelism: Parallelism::workers(jobs, 1 << 30),
            ..base.clone()
        };
        let got = seen(dump, cache, table, options(&membership)).await;
        let want = seen(dump, cache, table, options(&disjunction)).await;
        assert_eq!(
            got,
            want,
            "{}: {table} under {membership:?}, statistics {use_statistics}",
            dump.display()
        );
        tally.compared += 1;
        match &want.outcome {
            Ok(Some(rows)) if rows.num_rows() > 0 => tally.kept_rows += 1,
            Ok(_) => {}
            Err(_) => tally.errors += 1,
        }
        let skipped = want.plan.iter().any(|note| {
            matches!(note.kind, pgdump_query::PlanNoteKind::StatisticsPruned { skipped_groups, .. } if skipped_groups > 0)
        });
        tally.pruned += usize::from(skipped);
        tally.with_null += usize::from(format!("{membership:?}").contains("None"));
    }
}

async fn check_fixture(
    fixture: &Path,
    oracle: &BTreeMap<String, Vec<String>>,
    rng: &mut Rng,
    tally: &mut Tally,
) {
    let (dir, dump, index) = gathered(fixture).await;
    let plain = dir.path().join("plain.dtcache");
    let source = LocalFileSource::open(&dump).unwrap();
    map_file(
        &source,
        &ScanOptions::default(),
        &CacheMode::enabled(plain.clone()),
        &StatisticsRequest::METADATA,
    )
    .await
    .unwrap();
    let gathered_cache = cache::colocated_path(&dump);
    let caches = (dump.as_path(), plain.as_path(), gathered_cache.as_path());

    let mut tables: BTreeMap<(Option<String>, String), Vec<&CopyBlock>> = BTreeMap::new();
    for block in index.blocks().filter(|b| b.statistics.is_some()) {
        tables
            .entry((block.database.clone(), block.header.qualified_name()))
            .or_default()
            .push(block);
    }
    for ((database, table), blocks) in tables {
        let header = &blocks[0].header;
        if header.columns.is_empty() {
            continue;
        }
        // Only the columns every row decodes are built, so a value the build
        // cannot hold (`KD8`) ends no comparison.
        let everything = QueryOptions { database: database.clone(), ..Default::default() };
        let decodes = |options: QueryOptions| {
            let (dump, plain, table) = (&dump, &plain, &table);
            async move { seen(dump, plain, table, options).await.outcome.is_ok() }
        };
        let projection = if decodes(everything.clone()).await {
            None
        } else {
            let mut kept = Vec::new();
            for name in &header.columns {
                let one =
                    QueryOptions { projection: Some(vec![name.clone()]), ..everything.clone() };
                if decodes(one).await {
                    kept.push(name.clone());
                }
            }
            Some(kept)
        };
        let base = QueryOptions { projection, ..everything };
        let arrow = QueryOptions { semantics: ComparisonSemantics::DataFusion, ..base.clone() };

        let mut leaves = Vec::new();
        for (i, name) in header.columns.iter().enumerate() {
            let pool = pool(&blocks, i, oracle);
            if pool.is_empty() {
                continue;
            }
            for _ in 0..LISTS_PER_COLUMN {
                let (membership, disjunction) = pair(name, list(rng, &pool));
                let negated = (
                    Expr::Not(Box::new(membership.clone())),
                    Expr::Not(Box::new(disjunction.clone())),
                );
                let semantics = if rng.below(3) == 0 { &arrow } else { &base };
                check(caches, &table, semantics, (membership.clone(), disjunction.clone()), tally)
                    .await;
                check(caches, &table, semantics, negated, tally).await;
                leaves.push((membership, disjunction));
            }
        }
        if leaves.is_empty() {
            continue;
        }
        for _ in 0..TREES_PER_TABLE {
            check(caches, &table, &base, tree(rng, &leaves, 3), tally).await;
        }
    }
}

/// **The check.** One thread per major, each with its own seed. It runs whole
/// in the default suite, as `tests/pruning.rs`'s does and for its reason:
/// its floors are what make it the check.
#[test]
fn a_membership_answers_as_the_disjunction_of_equalities_it_replaces() {
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let workers: Vec<_> = VERSIONS
            .iter()
            .map(|&version| {
                scope.spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
                    runtime.block_on(async {
                        let oracle = oracle_literals(version);
                        let mut rng = Rng(0x1157_0000 + u64::from(version));
                        let mut tally = Tally::default();
                        for schema in SCHEMAS {
                            let dump = fixture(version, schema, "default");
                            check_fixture(&dump, &oracle, &mut rng, &mut tally).await;
                        }
                        tally
                    })
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().unwrap()).collect()
    });
    let tally = tallies.into_iter().fold(Tally::default(), |mut sum, t| {
        sum.compared += t.compared;
        sum.kept_rows += t.kept_rows;
        sum.pruned += t.pruned;
        sum.errors += t.errors;
        sum.with_null += t.with_null;
        sum
    });
    // Floors, so a sweep that compared nothing, kept nothing, skipped
    // nothing or only ever refused fails rather than passing vacuously.
    assert!(tally.compared > 15_000, "{tally:?}");
    assert!(tally.kept_rows > tally.compared / 3, "{tally:?}");
    assert!(tally.pruned > tally.compared / 4, "{tally:?}");
    assert!(tally.errors > 0 && tally.errors < tally.compared / 4, "{tally:?}");
    assert!(tally.with_null > tally.compared / 5, "{tally:?}");
}
