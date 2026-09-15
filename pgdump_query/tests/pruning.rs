//! The pruning consumer: a query skips the row groups whose statistics prove
//! no row satisfies its filter, and stops reading a block sorted past the
//! filter's bound (`QueryOptions::use_statistics`).
//!
//! **A pruned query returns exactly what the same query returns unpruned**,
//! checked over every fixture with generated filters — the phase's
//! correctness check. That skipping and stopping actually happen is pinned
//! apart from it, by filters whose skip is known from the `statistics`
//! fixture's shapes (`tests/statistics_fixture.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;
use futures::StreamExt;
use pgdump_query::cache::{self, CacheMode};
use pgdump_query::{
    CopyBlock, DumpIndex, EarlyStop, Expr, LocalFileSource, Parallelism, PlanNote, PlanNoteKind,
    Predicate, PredicateOp, QueryOptions, ScanOptions, StatisticsRequest, StatisticsSelection,
    map_file, table_stream, table_stream_partitions,
};

mod common;
use common::{VERSIONS, all_fixtures, sandboxed, statistics_fixture};

/// The group size the generated check gathers at: tens of bytes, where the
/// shipped mebibyte makes every fixture block one group and pruning has
/// nothing to skip.
const TINY_GROUP: u64 = 32;

/// Random trees per table, each run pruned and unpruned, serially and split.
const TREES_PER_TABLE: usize = 24;

/// Literals drawn for each operator over each column.
const LITERALS_PER_OPERATOR: usize = 2;

/// The sub-streams the split legs ask for.
const SPLIT_JOBS: usize = 3;

/// SplitMix64, seeded, so a failing filter is the same filter on every run.
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

/// `dump` mapped into a cache beside a private copy of it, gathering every
/// statistic at `group_size` — what a query then reads its statistics from.
async fn gathered(dump: &Path, group_size: u64) -> (tempfile::TempDir, PathBuf, DumpIndex) {
    let (dir, copy) = sandboxed(dump, "dump.sql");
    let source = LocalFileSource::open(&copy).unwrap();
    let request = StatisticsRequest {
        selection: StatisticsSelection::All,
        group_size: Some(NonZeroU64::new(group_size).unwrap()),
        min_rows: None,
    };
    let cache = CacheMode::Enabled(cache::colocated_path(&copy));
    let run = map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    assert!(!run.interrupted);
    (dir, copy, run.index)
}

/// One query's answer: every row in file order as one batch (`None` for no
/// row), the plan's notes, the schema, and every sub-stream's early stops
/// once drained, in file order — or the error, as text.
type Answer = Result<(Option<RecordBatch>, Vec<PlanNote>, Resolved, Vec<EarlyStop>), String>;

/// What the first sub-stream says of the schema once drained — what `pgdq
/// query` announces.
type Resolved = (pgdump_query::ResolvedSchema, Vec<pgdump_query::ComparisonNote>);

/// One query as far as it got: its rows and schema or its error, beside the
/// plan's notes and the early stops its sub-streams reached — which say,
/// whichever it ended in, whether it left a row unread.
struct Attempt {
    outcome: Result<(Option<RecordBatch>, Resolved), String>,
    notes: Vec<PlanNote>,
    stops: Vec<EarlyStop>,
}

/// `options` against `table`, its sub-streams drained in the order the split
/// handed them back, which is file order (`docs/design/decisions.md`, "D51"),
/// up to the first error.
async fn attempt(dump: &Path, cache: &Path, table: &str, options: QueryOptions) -> Attempt {
    let source = LocalFileSource::open(dump).unwrap();
    let cache = CacheMode::Enabled(cache.to_path_buf());
    let mut streams =
        match table_stream_partitions(&source, table, ScanOptions::default(), options, cache).await
        {
            Ok(streams) => streams,
            Err(e) => {
                return Attempt {
                    outcome: Err(e.to_string()),
                    notes: Vec::new(),
                    stops: Vec::new(),
                };
            }
        };
    let notes = streams[0].plan_notes();
    let (mut batches, mut failure) = (Vec::new(), None);
    'drain: for stream in &mut *streams {
        while let Some(batch) = stream.next().await {
            match batch {
                Ok(batch) => batches.push(batch),
                Err(e) => {
                    failure = Some(e.to_string());
                    break 'drain;
                }
            }
        }
    }
    let stops = streams.iter().flat_map(|stream| stream.early_stops()).collect();
    let outcome = match failure {
        Some(e) => Err(e),
        None => {
            let rows =
                batches.first().map(|first| concat_batches(&first.schema(), &batches).unwrap());
            Ok((rows, (streams[0].resolved_schema(), streams[0].comparison_notes())))
        }
    };
    Attempt { outcome, notes, stops }
}

/// [`attempt`], its notes and stops kept only where it answers.
async fn answer(dump: &Path, cache: &Path, table: &str, options: QueryOptions) -> Answer {
    let Attempt { outcome, notes, stops } = attempt(dump, cache, table, options).await;
    outcome.map(|(rows, resolved)| (rows, notes, resolved, stops))
}

/// Per block, what `stops` — gathered over sub-streams — left unread, a block
/// whose stop fired in no sub-stream mapping to `None`.
fn unread_per_block(stops: &[EarlyStop]) -> BTreeMap<u64, Option<u64>> {
    let mut out = BTreeMap::new();
    for stop in stops {
        let entry: &mut Option<u64> = out.entry(stop.header_offset).or_default();
        if let Some(bytes) = stop.unread_bytes {
            *entry.get_or_insert(0) += bytes;
        }
    }
    out
}

fn pruned(notes: &[PlanNote]) -> Option<(u64, u64, u64, u64)> {
    notes.iter().find_map(|note| match note.kind {
        PlanNoteKind::StatisticsPruned { skipped_groups, groups, skipped_bytes, bytes } => {
            Some((skipped_groups, groups, skipped_bytes, bytes))
        }
        _ => None,
    })
}

fn term(column: &str, op: PredicateOp, value: Option<&str>) -> Predicate {
    Predicate { column: column.to_string(), op, value: value.map(str::to_string) }
}

fn with(filter: Expr, use_statistics: bool, jobs: usize) -> QueryOptions {
    QueryOptions {
        filter,
        use_statistics,
        parallelism: Parallelism::workers(jobs, 1 << 30),
        ..QueryOptions::default()
    }
}

/// The literals a column's terms compare against: its stored bounds and
/// dictionary entries, a neighbour on each side of each, and the committed
/// comparison oracle's values for its declared type
/// (`docs/design/decisions.md`, "D70").
fn literals(
    blocks: &[&CopyBlock],
    column: usize,
    oracle: &BTreeMap<String, Vec<String>>,
    rng: &mut Rng,
) -> Vec<String> {
    let mut stored = BTreeSet::new();
    let mut declared = None;
    for block in blocks {
        let Some(statistics) = block.statistics.as_deref() else { continue };
        let Some(Some(column)) = statistics.columns.get(column) else { continue };
        declared = declared.or(column.declared_type.clone());
        if let Some(bounds) = &column.bounds {
            let present: Vec<_> = bounds.groups.iter().flatten().collect();
            for _ in 0..present.len().min(3) {
                let group = present[rng.below(present.len())];
                stored.insert(group.min.clone());
                stored.insert(group.max.clone());
            }
        }
        if let Some(dictionary) = &column.dictionary {
            for _ in 0..dictionary.entries.len().min(3) {
                stored.insert(dictionary.entries[rng.below(dictionary.entries.len())].clone());
            }
        }
    }
    let mut out = BTreeSet::new();
    for value in stored {
        if let Ok(n) = value.parse::<i64>() {
            out.extend(
                [n.checked_sub(1), n.checked_add(1)].into_iter().flatten().map(|n| n.to_string()),
            );
        }
        // A spelling just above, and one just below: `1.5` → `1.50` is the
        // same number, `abc` → `abc0` the next text.
        out.insert(format!("{value}0"));
        let mut shorter = value.clone();
        if shorter.pop().is_some() && !shorter.is_empty() {
            out.insert(shorter);
        }
        out.insert(value);
    }
    if let Some(values) = declared.as_ref().and_then(|d| oracle.get(d)) {
        out.extend(values.iter().cloned());
    }
    out.into_iter().collect()
}

/// Each major's oracle literals, by declared type: the `*_out` text of every
/// value the server accepted.
fn oracle_literals(version: &str) -> BTreeMap<String, Vec<String>> {
    let path = common::fixtures_root().join(version).join("oracle/literals.tsv");
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

fn random_tree(rng: &mut Rng, terms: &[Predicate], depth: usize) -> Expr {
    let children =
        |rng: &mut Rng| (0..1 + rng.below(3)).map(|_| random_tree(rng, terms, depth - 1)).collect();
    match if depth == 0 { 0 } else { rng.below(5) } {
        0 | 1 => Expr::Term(terms[rng.below(terms.len())].clone()),
        2 => Expr::And(children(rng)),
        3 => Expr::Or(children(rng)),
        _ => Expr::Not(Box::new(random_tree(rng, terms, depth - 1))),
    }
}

/// What the generated check saw, so a sweep that compared nothing fails.
#[derive(Default, Debug)]
struct Tally {
    compared: usize,
    pruning: usize,
    stopping: usize,
    skipped_groups: u64,
    unpruned_errors: usize,
    /// Unpruned errors whose pruned query read every row, so raised too.
    errors_raised_pruned: usize,
}

/// `filter` pruned against unpruned, serially and split: **identical rows in
/// file order** wherever the unpruned query answers. Where it raises, the
/// pruned query may answer instead, a value in a group it skipped or past a
/// sorted block's stop being one it never reads (`docs/design/decisions.md`,
/// "D54") — but **one whose note skips no row's bytes and whose stops left no
/// row unread raises the same error**, having read every row the unpruned
/// query reads, in the same order. Not "skips no group": at [`TINY_GROUP`] a
/// block of rows longer than a group lists groups no row starts in, which
/// every filter reading a field skips.
///
/// The unpruned query reads `plain`, a cache holding no statistic, whose rows
/// are the gathered cache's under `use_statistics: false` and which loads in a
/// fraction of the time; that the switch alone reads every group is pinned by
/// the hand-written tests below. Reading both legs from the gathered cache
/// was refused: it doubles the loading, to pin a switch that is one early
/// return before pruning, and two independently mapped caches compare more
/// than one would.
async fn check(
    dump: &Path,
    (plain, gathered): (&Path, &Path),
    table: &str,
    base: &QueryOptions,
    filter: Expr,
    tally: &mut Tally,
) {
    for jobs in [1, SPLIT_JOBS] {
        let options = |use_statistics| QueryOptions {
            filter: filter.clone(),
            use_statistics,
            parallelism: Parallelism::workers(jobs, 1 << 30),
            ..base.clone()
        };
        let reference = attempt(dump, plain, table, options(false)).await;
        let got = attempt(dump, gathered, table, options(true)).await;
        let (notes, stops) = (&reference.notes, &reference.stops);
        let (pruned_notes, pruned_stops) = (&got.notes, &got.stops);
        match (&reference.outcome, &got.outcome) {
            (Ok((rows, resolved)), Ok((pruned_rows, pruned_resolved))) => {
                assert!(pruned(notes).is_none(), "{notes:?}");
                assert!(stops.is_empty(), "{stops:?}");
                assert_eq!(
                    pruned_rows,
                    rows,
                    "{}: {table} under {filter:?} at {jobs} job(s) pruned {:?}",
                    dump.display(),
                    pruned(pruned_notes)
                );
                assert_eq!(
                    pruned_resolved,
                    resolved,
                    "{}: {table} under {filter:?}",
                    dump.display()
                );
                tally.compared += 1;
                if let Some((skipped, ..)) = pruned(pruned_notes).filter(|p| p.0 > 0) {
                    tally.pruning += 1;
                    tally.skipped_groups += skipped;
                }
                // A stop's unread bytes add to the skipped groups' without
                // overlapping them.
                let unread: u64 = pruned_stops.iter().filter_map(|s| s.unread_bytes).sum();
                if unread > 0 {
                    tally.stopping += 1;
                    let (_, _, skipped_bytes, bytes) = pruned(pruned_notes).unwrap();
                    assert!(
                        skipped_bytes + unread <= bytes,
                        "{}: {table} under {filter:?} at {jobs} job(s): {skipped_bytes} skipped \
                         and {unread} unread of {bytes}",
                        dump.display()
                    );
                }
            }
            (Err(e), pruned_outcome) => {
                tally.unpruned_errors += 1;
                // A group no row starts in lists no bytes (`RowGroup::bytes`),
                // so a note skipping none skipped no row.
                let read_every_row = pruned(pruned_notes).is_none_or(|p| p.2 == 0)
                    && pruned_stops.iter().all(|stop| stop.unread_bytes.is_none());
                if read_every_row {
                    tally.errors_raised_pruned += 1;
                    assert_eq!(
                        pruned_outcome.as_ref().err(),
                        Some(e),
                        "{}: {table} under {filter:?} at {jobs} job(s) skips nothing pruned",
                        dump.display()
                    );
                }
            }
            (Ok(_), Err(e)) => panic!(
                "{}: {table} under {filter:?} at {jobs} job(s) answers unpruned and raises \
                 pruned: {e}",
                dump.display()
            ),
        }
    }
}

/// **The correctness check.** Every fixture, gathered at [`TINY_GROUP`]
/// bytes; for every table, terms under every operator over each column
/// carrying statistics, and seeded random `And`/`Or`/`Not` trees over them.
/// One thread per major, each with its own seed.
///
/// **It runs whole, in the default suite**, though no other test here costs
/// as much: its floors are what make it the check, and every later change to
/// replay is what it exists to catch. Refused: lowering [`TREES_PER_TABLE`],
/// [`LITERALS_PER_OPERATOR`] or the singles' share, and `#[ignore]` or an
/// environment switch leaving only the hand-written tests on by default.
#[test]
fn every_fixture_prunes_to_the_rows_it_returns_unpruned() {
    let mut fixtures = all_fixtures();
    fixtures.sort();
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let workers: Vec<_> = VERSIONS
            .iter()
            .map(|&version| {
                let mine: Vec<PathBuf> = fixtures
                    .iter()
                    .filter(|f| {
                        f.parent().unwrap().parent().unwrap().ends_with(version.to_string())
                    })
                    .cloned()
                    .collect();
                scope.spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
                    runtime.block_on(async {
                        let oracle = oracle_literals(&version.to_string());
                        let mut rng = Rng(0x5eed_1008 + u64::from(version));
                        let mut tally = Tally::default();
                        for fixture in mine {
                            check_fixture(&fixture, &oracle, &mut rng, &mut tally).await;
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
        sum.pruning += t.pruning;
        sum.stopping += t.stopping;
        sum.skipped_groups += t.skipped_groups;
        sum.unpruned_errors += t.unpruned_errors;
        sum.errors_raised_pruned += t.errors_raised_pruned;
        sum
    });
    assert!(tally.compared > 40_000, "{tally:?}");
    assert!(tally.pruning > tally.compared / 3, "{tally:?}");
    assert!(tally.stopping > 20, "{tally:?}");
    assert!(tally.unpruned_errors < tally.compared / 100, "{tally:?}");
    assert!(tally.errors_raised_pruned > 0, "{tally:?}");
}

const OPERATORS: [PredicateOp; 8] = [
    PredicateOp::Eq,
    PredicateOp::Ne,
    PredicateOp::Lt,
    PredicateOp::Le,
    PredicateOp::Gt,
    PredicateOp::Ge,
    PredicateOp::IsDistinctFrom,
    PredicateOp::IsNotDistinctFrom,
];

async fn check_fixture(
    fixture: &Path,
    oracle: &BTreeMap<String, Vec<String>>,
    rng: &mut Rng,
    tally: &mut Tally,
) {
    let (dir, dump, index) = gathered(fixture, TINY_GROUP).await;
    // Resolution reads no statistic, so what refuses is asked of a cache
    // holding none, which loads in a fraction of the time.
    let plain = dir.path().join("plain.dqcache");
    let source = LocalFileSource::open(&dump).unwrap();
    let request = StatisticsRequest::NONE;
    map_file(&source, &ScanOptions::default(), &CacheMode::Enabled(plain.clone()), &request)
        .await
        .unwrap();
    let gathered_cache = cache::colocated_path(&dump);

    let mut tables: BTreeMap<(Option<String>, String), Vec<&CopyBlock>> = BTreeMap::new();
    for block in index.blocks().filter(|b| b.statistics.is_some()) {
        tables
            .entry((block.database.clone(), block.header.qualified_name()))
            .or_default()
            .push(block);
    }
    for ((database, table), blocks) in tables {
        let header = &blocks[0].header;
        // Only the columns every row of the table decodes are built, so a
        // value the build cannot hold (`KD8`) ends no comparison.
        let everything = QueryOptions { database: database.clone(), ..Default::default() };
        let projection = match answer(&dump, &plain, &table, everything.clone()).await {
            Ok(_) => None,
            Err(_) => {
                let mut decodes = Vec::new();
                for name in &header.columns {
                    let one =
                        QueryOptions { projection: Some(vec![name.clone()]), ..everything.clone() };
                    if answer(&dump, &plain, &table, one).await.is_ok() {
                        decodes.push(name.clone());
                    }
                }
                Some(decodes)
            }
        };
        let base = QueryOptions { projection, ..everything };

        let mut terms = Vec::new();
        for (i, name) in header.columns.iter().enumerate() {
            terms.push(term(name, PredicateOp::IsNull, None));
            terms.push(term(name, PredicateOp::IsNotNull, None));
            let values = literals(&blocks, i, oracle, rng);
            if values.is_empty() {
                continue;
            }
            for op in OPERATORS {
                for _ in 0..LITERALS_PER_OPERATOR {
                    let candidate = term(name, op, Some(&values[rng.below(values.len())]));
                    // Kept only where it resolves: a refusal refuses both
                    // queries alike, and a tree holding one compares nothing.
                    if resolves(&dump, &plain, &table, &base, &candidate).await {
                        terms.push(candidate);
                    }
                }
            }
        }
        for candidate in &terms {
            if rng.below(8) == 0 {
                let filter = Expr::Term(candidate.clone());
                check(&dump, (&plain, &gathered_cache), &table, &base, filter, tally).await;
            }
        }
        for _ in 0..TREES_PER_TABLE {
            let tree = random_tree(rng, &terms, 3);
            check(&dump, (&plain, &gathered_cache), &table, &base, tree, tally).await;
        }
    }
}

/// Whether `candidate` alone resolves against `table`: the plan is settled
/// before a sub-stream exists, so this reads no row.
async fn resolves(
    dump: &Path,
    cache: &Path,
    table: &str,
    base: &QueryOptions,
    candidate: &Predicate,
) -> bool {
    let source = LocalFileSource::open(dump).unwrap();
    let options = QueryOptions { filter: Expr::Term(candidate.clone()), ..base.clone() };
    let cache = CacheMode::Enabled(cache.to_path_buf());
    table_stream_partitions(&source, table, ScanOptions::default(), options, cache).await.is_ok()
}

/// A group size several `ordered` rows long, so its thousand ascending ids
/// span many groups.
const SMALL_GROUP: u64 = 1024;

/// **An ascending column under a range filter reads only the groups past the
/// bound**: `id` ascends, so every group ahead of the one holding `990` has a
/// maximum below it, and at most the group straddling it and those after are
/// read.
#[tokio::test]
async fn a_sorted_column_under_a_range_filter_skips_every_group_before_the_bound() {
    for version in VERSIONS {
        let (_dir, dump, index) =
            gathered(&statistics_fixture(version, "default"), SMALL_GROUP).await;
        let block = index.blocks_for("public.ordered").next().unwrap();
        let statistics = block.statistics.as_deref().unwrap();
        let id = statistics.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
        let below: u64 =
            id.groups.iter().flatten().filter(|b| b.max.parse::<i64>().unwrap() < 990).count()
                as u64;
        assert!(below > 10, "pg_dump {version}: {below} group(s) below the bound");

        let filter = Expr::all([term("id", PredicateOp::Ge, Some("990"))]);
        for jobs in [1, SPLIT_JOBS] {
            let (rows, notes, ..) = answer(
                &dump,
                &cache::colocated_path(&dump),
                "public.ordered",
                with(filter.clone(), true, jobs),
            )
            .await
            .unwrap();
            let (unpruned, unpruned_notes, ..) = answer(
                &dump,
                &cache::colocated_path(&dump),
                "public.ordered",
                with(filter.clone(), false, jobs),
            )
            .await
            .unwrap();
            assert_eq!(rows, unpruned, "pg_dump {version} at {jobs} job(s)");
            assert_eq!(rows.unwrap().num_rows(), 11);
            assert!(unpruned_notes.is_empty(), "{unpruned_notes:?}");
            let (skipped, groups, skipped_bytes, bytes) = pruned(&notes).unwrap();
            assert_eq!(groups, statistics.groups.len() as u64);
            assert_eq!(skipped, below, "pg_dump {version} at {jobs} job(s)");
            assert_eq!(bytes, block.terminator_offset - block.data_offset);
            assert!(skipped_bytes > bytes * 9 / 10, "{skipped_bytes} of {bytes}");
        }
    }
}

/// **A dictionary rules out a literal its bounds cannot**: `low_card` cycles
/// through four texts, so every group holds all four and bounds from `amber`
/// to `dusk`, and `bravo` sorts inside them and is none of them — so every
/// group is skipped, and only the dictionary can have said so.
#[tokio::test]
async fn a_dictionary_under_an_absent_literal_skips_every_group() {
    for version in VERSIONS {
        let (_dir, dump, index) =
            gathered(&statistics_fixture(version, "default"), SMALL_GROUP).await;
        let block = index.blocks_for("public.ordered").next().unwrap();
        let groups = block.statistics.as_ref().unwrap().groups.len() as u64;

        let absent = Expr::all([term("low_card", PredicateOp::Eq, Some("bravo"))]);
        for jobs in [1, SPLIT_JOBS] {
            let (rows, notes, ..) = answer(
                &dump,
                &cache::colocated_path(&dump),
                "public.ordered",
                with(absent.clone(), true, jobs),
            )
            .await
            .unwrap();
            assert!(rows.is_none());
            assert_eq!(
                pruned(&notes).map(|p| (p.0, p.1)),
                Some((groups, groups)),
                "pg_dump {version} at {jobs} job(s)"
            );
        }
    }
}

/// **A statistic is believed only under the declared type it was gathered
/// under.** The cache's `low_card` statistics are relabelled as gathered under
/// another type, and the filter they would rule out reads every group.
#[tokio::test]
async fn statistics_gathered_under_another_declared_type_prune_nothing() {
    let (_dir, dump, mut index) = gathered(&statistics_fixture(16, "default"), SMALL_GROUP).await;
    let absent = Expr::all([term("low_card", PredicateOp::Eq, Some("bravo"))]);
    let (_, notes, ..) = answer(
        &dump,
        &cache::colocated_path(&dump),
        "public.ordered",
        with(absent.clone(), true, 1),
    )
    .await
    .unwrap();
    assert!(pruned(&notes).unwrap().0 > 0);

    for span in &mut index.spans {
        if let pgdump_query::SpanBody::Data(pgdump_query::DataBlock::Copy(block)) = &mut span.body
            && block.header.table == "ordered"
        {
            let mut statistics = (**block.statistics.as_ref().unwrap()).clone();
            statistics.columns[8].as_mut().unwrap().declared_type =
                Some("character varying".into());
            block.statistics = Some(Arc::new(statistics));
        }
    }
    let source = LocalFileSource::open(&dump).unwrap();
    cache::save(&cache::colocated_path(&dump), &source, &index).await.unwrap();
    let (_, notes, ..) =
        answer(&dump, &cache::colocated_path(&dump), "public.ordered", with(absent, true, 1))
            .await
            .unwrap();
    assert_eq!(pruned(&notes).unwrap().0, 0);
}

/// **A serial stream resumed at any batch through pruned gaps continues with
/// exactly the rows it had not delivered**, including a pause on the last row
/// of a run of kept groups, whose next row is in a skipped one.
#[tokio::test]
async fn a_pruned_stream_resumes_across_its_gaps() {
    let (_dir, dump, _index) = gathered(&statistics_fixture(16, "default"), SMALL_GROUP).await;
    let filter = Expr::Or(vec![
        Expr::Term(term("id", PredicateOp::Lt, Some("40"))),
        Expr::Term(term("id", PredicateOp::Ge, Some("500"))),
        Expr::Term(term("id", PredicateOp::Eq, Some("300"))),
    ]);
    let source = LocalFileSource::open(&dump).unwrap();
    let cache = CacheMode::Enabled(cache::colocated_path(&dump));
    let ids = |batch: &RecordBatch| {
        let column = arrow::array::AsArray::as_primitive::<arrow::datatypes::Int32Type>(
            batch.column(0).as_ref(),
        );
        column.values().to_vec()
    };
    let expected: Vec<i32> = (1..40).chain([300]).chain(500..=1000).collect();

    for max_rows in [1, 7, 39, 40] {
        let options = QueryOptions {
            filter: filter.clone(),
            projection: Some(vec!["id".into()]),
            max_rows,
            ..QueryOptions::default()
        };
        let mut delivered = Vec::new();
        let mut token = None;
        let mut resumes = 0;
        loop {
            let mut stream = table_stream(
                &source,
                "public.ordered",
                ScanOptions::default(),
                options.clone(),
                token.take(),
                cache.clone(),
            );
            let Some(batch) = stream.next().await else { break };
            let batch = batch.unwrap();
            if resumes == 0 {
                assert!(pruned(&stream.plan_notes()).is_some_and(|p| p.0 > 0));
            }
            delivered.extend(ids(&batch));
            token = Some(stream.resume_token());
            resumes += 1;
        }
        assert_eq!(delivered, expected, "max_rows {max_rows}");
    }
}

/// A local file that records where every read it serves starts, and splits
/// as the file does.
struct Recording {
    inner: LocalFileSource,
    starts: std::sync::Mutex<Vec<u64>>,
}

impl pgdump_query::ByteRangeSource for Recording {
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = pgdump_query::Result<bytes::Bytes>> + Send + '_>,
    > {
        self.starts.lock().unwrap().push(offset);
        self.inner.read_range(offset, len)
    }

    /// The file's own advice, without which a split query is one sub-stream.
    fn partitions(&self, range: std::ops::Range<u64>) -> pgdump_query::Partitioning {
        self.inner.partitions(range)
    }

    fn size(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = pgdump_query::Result<u64>> + Send + '_>>
    {
        self.inner.size()
    }

    fn modified(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = pgdump_query::Result<Option<std::time::SystemTime>>>
                + Send
                + '_,
        >,
    > {
        self.inner.modified()
    }
}

/// **A pruned query reads nothing deep inside a run of skipped groups** —
/// split, and serially resumed at every batch, a resumed stream whose pause
/// precedes a gap included. A read may start a little way into a skipped run,
/// the row straddling out of the kept group before it ending there, and on the
/// last byte of one, where the kept group after it searches for its first row;
/// in a skipped group between two skipped groups, neither reaches.
#[tokio::test]
async fn a_pruned_query_reads_nothing_deep_inside_a_skipped_run() {
    let (_dir, dump, index) = gathered(&statistics_fixture(16, "default"), SMALL_GROUP).await;
    let block = index.blocks_for("public.ordered").next().unwrap();
    let statistics = block.statistics.as_deref().unwrap();
    let id = statistics.columns[0].as_ref().unwrap().bounds.as_ref().unwrap();
    let skipped: Vec<bool> = id
        .groups
        .iter()
        .map(|b| {
            let b = b.as_ref().unwrap();
            b.min.parse::<i64>().unwrap() >= 40 && b.max.parse::<i64>().unwrap() < 500
        })
        .collect();
    let n = statistics.group_size;
    let deep = |offset: u64| {
        let k = offset.checked_sub(block.data_offset).map(|at| (at / n) as usize);
        k.is_some_and(|k| k >= 1 && (k - 1..=k + 1).all(|g| skipped.get(g) == Some(&true)))
    };
    assert!((block.data_offset..block.terminator_offset).any(deep), "the fixture has a deep gap");

    let filter = Expr::Or(vec![
        Expr::Term(term("id", PredicateOp::Lt, Some("40"))),
        Expr::Term(term("id", PredicateOp::Ge, Some("500"))),
    ]);
    let source = Recording {
        inner: LocalFileSource::open(&dump).unwrap(),
        starts: std::sync::Mutex::new(Vec::new()),
    };
    let cache = CacheMode::Enabled(cache::colocated_path(&dump));
    let scan = ScanOptions { chunk_size: 64, ..ScanOptions::default() };
    let assert_undeep = |what: &str| {
        let starts = std::mem::take(&mut *source.starts.lock().unwrap());
        assert!(!starts.is_empty(), "{what}: read nothing at all");
        let inside: Vec<u64> = starts.into_iter().filter(|&o| deep(o)).collect();
        assert!(inside.is_empty(), "{what}: read at {inside:?}");
    };

    let split = QueryOptions {
        filter: filter.clone(),
        parallelism: Parallelism::workers(SPLIT_JOBS, 1 << 30),
        ..QueryOptions::default()
    };
    let streams =
        table_stream_partitions(&source, "public.ordered", scan.clone(), split, cache.clone())
            .await
            .unwrap();
    for mut stream in streams {
        while let Some(batch) = stream.next().await {
            batch.unwrap();
        }
    }
    assert_undeep("split");

    // The last row starting in group 2 alone of the rows below 900: a stream
    // pausing on it pauses past its run's end, with a gap before the next.
    let bytes = std::fs::read(&dump).unwrap();
    let data = &bytes[block.data_offset as usize..block.terminator_offset as usize];
    let mut at = 0u64;
    let mut last_in_group_2 = None;
    for line in data.split_inclusive(|&b| b == b'\n') {
        if at / n == 2 {
            last_in_group_2 = std::str::from_utf8(line).unwrap().split('\t').next();
        }
        at += line.len() as u64;
    }
    let paused = last_in_group_2.unwrap().to_string();
    let straddling = Expr::Or(vec![
        Expr::Term(term("id", PredicateOp::Eq, Some(&paused))),
        Expr::Term(term("id", PredicateOp::Ge, Some("900"))),
    ]);
    for (filter, max_rows, expected) in [(filter, 13, 39 + 501), (straddling, 1, 1 + 101)] {
        let options = QueryOptions {
            filter,
            projection: Some(vec!["id".into()]),
            max_rows,
            ..QueryOptions::default()
        };
        let mut token = None;
        let mut rows = 0;
        loop {
            let mut stream = table_stream(
                &source,
                "public.ordered",
                scan.clone(),
                options.clone(),
                token.take(),
                cache.clone(),
            );
            let Some(batch) = stream.next().await else { break };
            rows += batch.unwrap().num_rows();
            token = Some(stream.resume_token());
            drop(stream);
            assert_undeep(&format!("resumed after {rows} row(s) under {:?}", options.filter));
        }
        assert_eq!(rows, expected);
    }
}

/// Where the line of `public.ordered` whose `id` is `id` ends: the byte after
/// its LF.
fn ordered_line_end(dump: &Path, block: &CopyBlock, id: u32) -> u64 {
    let bytes = std::fs::read(dump).unwrap();
    let data = &bytes[block.data_offset as usize..block.terminator_offset as usize];
    let mut at = block.data_offset;
    for line in data.split_inclusive(|&b| b == b'\n') {
        at += line.len() as u64;
        if line.starts_with(format!("{id}\t").as_bytes()) {
            return at;
        }
    }
    panic!("no row with id {id}");
}

/// Where a stop is expected in `public.ordered`, for
/// [`a_sorted_block_is_read_no_further_than_its_first_row_past_the_bound`].
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// Reading ends at the row whose `id` is this.
    At(u32),
    /// A stop is planned and ends nothing: the block is read to its end.
    Unreached,
    /// No stop is planned, and the block is read to its end.
    Unplanned,
}

/// **A block sorted on a column its filter bounds is read no further than its
/// first row past the bound**, inside the one group the shipped size makes of
/// it — ascending under `<` and `<=`, descending under `>`, NULLs between
/// values stopping nothing, the term inside a nested conjunction — and read to
/// its end where the order does not close the bound, where no conjunction
/// requires the term, or under `use_statistics: false`. Serially no read
/// starts past the stopping row; split, each piece past it reads only as far
/// as its own first row.
///
/// **What the stop left unread is reported once the streams drain**: serially,
/// every byte of rows past the stopping row; split, fewer but some, a later
/// piece reading its own first row; a stop planned where the bound is never
/// passed, or passed only by the last row, is an entry that saved nothing, and
/// a filter no order closes has no entry.
#[tokio::test]
async fn a_sorted_block_is_read_no_further_than_its_first_row_past_the_bound() {
    let (_dir, dump, index) =
        gathered(&statistics_fixture(16, "default"), pgdump_query::DEFAULT_STATISTICS_GROUP_SIZE)
            .await;
    let block = index.blocks_for("public.ordered").next().unwrap();
    assert_eq!(block.statistics.as_deref().unwrap().groups.len(), 1);
    let source = Recording {
        inner: LocalFileSource::open(&dump).unwrap(),
        starts: std::sync::Mutex::new(Vec::new()),
    };
    let cache = CacheMode::Enabled(cache::colocated_path(&dump));
    let scan = ScanOptions { chunk_size: 64, ..ScanOptions::default() };
    let single = |column, op, value| Expr::Term(term(column, op, Some(value)));

    // A filter, its row count, and where its stop ends reading.
    let cases = [
        (single("id", PredicateOp::Lt, "20"), 19, Stop::At(20)),
        (single("stepped", PredicateOp::Le, "3"), 40, Stop::At(41)),
        (single("reversed", PredicateOp::Gt, "990"), 10, Stop::At(11)),
        (single("gappy", PredicateOp::Lt, "100"), 85, Stop::At(100)),
        (
            Expr::And(vec![
                Expr::And(vec![single("low_card", PredicateOp::Eq, "amber")]),
                single("id", PredicateOp::Lt, "20"),
            ]),
            4,
            Stop::At(20),
        ),
        (single("id", PredicateOp::Le, "1000"), 1000, Stop::Unreached),
        (single("id", PredicateOp::Lt, "1000"), 999, Stop::Unreached),
        (single("id", PredicateOp::Gt, "980"), 20, Stop::Unplanned),
        (single("reversed", PredicateOp::Lt, "20"), 19, Stop::Unplanned),
        (single("unsorted", PredicateOp::Lt, "20"), 20, Stop::Unplanned),
        (
            Expr::Or(vec![
                single("id", PredicateOp::Lt, "20"),
                single("low_card", PredicateOp::Eq, "absent"),
            ]),
            19,
            Stop::Unplanned,
        ),
        (Expr::Not(Box::new(single("id", PredicateOp::Ge, "20"))), 19, Stop::Unplanned),
    ];
    for (filter, expected, stops_at) in cases {
        for (use_statistics, jobs) in [(true, 1), (true, SPLIT_JOBS), (false, 1)] {
            let what = format!("{filter:?} at {jobs} job(s), statistics {use_statistics}");
            let options = with(filter.clone(), use_statistics, jobs);
            let streams = table_stream_partitions(
                &source,
                "public.ordered",
                scan.clone(),
                options,
                cache.clone(),
            )
            .await
            .unwrap();
            let pieces = streams.len();
            let mut rows = 0;
            let mut stops = Vec::new();
            for mut stream in streams {
                while let Some(batch) = stream.next().await {
                    rows += batch.unwrap().num_rows();
                }
                stops.extend(stream.early_stops());
            }
            assert_eq!(rows, expected, "{what}");
            let unread = unread_per_block(&stops);
            let starts = std::mem::take(&mut *source.starts.lock().unwrap());
            let last = *starts.iter().max().unwrap();
            let stop = if use_statistics { stops_at } else { Stop::Unplanned };
            match stop {
                Stop::At(id) => {
                    let end = ordered_line_end(&dump, block, id);
                    let past = starts.iter().filter(|&&s| s >= end).count();
                    let rest = block.terminator_offset - end;
                    let reported = unread.get(&block.header_offset).copied().flatten();
                    assert_eq!(unread.len(), 1, "{what}: {stops:?}");
                    if jobs == 1 {
                        assert_eq!(past, 0, "{what}: read at {last}, past {end}");
                        assert_eq!(reported, Some(rest), "{what}: {stops:?}");
                    } else {
                        assert!(pieces > 1, "{what}: not split");
                        assert!(past <= 4 * pieces, "{what}: {past} read(s) past {end}");
                        let reported = reported.unwrap_or(0);
                        assert!(reported > 0 && reported <= rest, "{what}: {stops:?} of {rest}");
                    }
                }
                Stop::Unreached | Stop::Unplanned => {
                    assert!(
                        last + scan.chunk_size as u64 >= block.terminator_offset,
                        "{what}: last read at {last}, the block ending at {}",
                        block.terminator_offset
                    );
                    let entries = match stop {
                        Stop::Unreached => BTreeMap::from([(block.header_offset, None)]),
                        _ => BTreeMap::new(),
                    };
                    assert_eq!(unread, entries, "{what}: {stops:?}");
                }
            }
        }
    }
}

/// **Pruned, a stop's unread bytes are the rest of its kept run, and add to
/// the skipped groups'**: under `id < 500` at [`SMALL_GROUP`] every group
/// past the one holding `499` is skipped, and the stop at `500`, where that
/// row starts inside a kept group, reports the bytes from its end to the
/// run's limit. Those, the skipped bytes and the bytes read make up the
/// block's rows but for the tail of the one row straddling the run's limit,
/// which the run owns and neither count holds. Split, a later piece of the run
/// reads its own first row, so the report is smaller but never larger.
#[tokio::test]
async fn a_pruned_stop_reports_the_rest_of_its_run_beside_the_skipped_groups() {
    let mut fired = 0;
    for version in VERSIONS {
        let (_dir, dump, index) =
            gathered(&statistics_fixture(version, "default"), SMALL_GROUP).await;
        let block = index.blocks_for("public.ordered").next().unwrap();
        let n = block.statistics.as_deref().unwrap().group_size;
        let cache = cache::colocated_path(&dump);
        let filter = Expr::all([term("id", PredicateOp::Lt, Some("500"))]);
        let end = ordered_line_end(&dump, block, 500);
        let group_of = |offset: u64| (offset - block.data_offset) / n;
        // The row 500 starts in: kept only where 499 starts there too.
        let start_of_500 = end - {
            let bytes = std::fs::read(&dump).unwrap();
            let line = &bytes[..end as usize - 1];
            (line.len() - line.iter().rposition(|&b| b == b'\n').unwrap()) as u64
        };
        let stops_inside = group_of(start_of_500) == group_of(start_of_500 - 1);
        let run_end =
            (block.data_offset + (group_of(start_of_500) + 1) * n).min(block.terminator_offset);

        let (rows, notes, _, serial) =
            answer(&dump, &cache, "public.ordered", with(filter.clone(), true, 1)).await.unwrap();
        assert_eq!(rows.unwrap().num_rows(), 499, "pg_dump {version}");
        let (_, _, skipped_bytes, bytes) = pruned(&notes).unwrap();
        let serial = unread_per_block(&serial);
        if !stops_inside {
            assert_eq!(serial, BTreeMap::from([(block.header_offset, None)]), "pg_dump {version}");
            continue;
        }
        fired += 1;
        let unread = run_end - end;
        assert_eq!(
            serial,
            BTreeMap::from([(block.header_offset, Some(unread))]),
            "pg_dump {version}"
        );
        let read = end - block.data_offset;
        let straddling = bytes - read - unread - skipped_bytes;
        assert!(straddling < n, "pg_dump {version}: {straddling} byte(s) in neither count");

        let (_, _, _, split) =
            answer(&dump, &cache, "public.ordered", with(filter, true, SPLIT_JOBS)).await.unwrap();
        let split = unread_per_block(&split)[&block.header_offset].unwrap_or(0);
        assert!(split <= unread, "pg_dump {version}: {split} of {unread}");
    }
    assert!(fired > 0, "no fixture's stop fell inside a kept group");
}

/// **A stream resumed at any batch of a stopped block continues with exactly
/// the rows it had not delivered**: a pause on the last row the filter keeps
/// resumes into the stopping row, and stops there again.
#[tokio::test]
async fn a_stopped_stream_resumes_to_the_same_rows() {
    let (_dir, dump, _index) =
        gathered(&statistics_fixture(16, "default"), pgdump_query::DEFAULT_STATISTICS_GROUP_SIZE)
            .await;
    let source = LocalFileSource::open(&dump).unwrap();
    let cache = CacheMode::Enabled(cache::colocated_path(&dump));
    let filter = Expr::Term(term("id", PredicateOp::Le, Some("19")));
    for max_rows in [1, 7, 19, 20] {
        let options = QueryOptions {
            filter: filter.clone(),
            projection: Some(vec!["id".into()]),
            max_rows,
            ..QueryOptions::default()
        };
        let mut delivered = Vec::new();
        let mut token = None;
        loop {
            let mut stream = table_stream(
                &source,
                "public.ordered",
                ScanOptions::default(),
                options.clone(),
                token.take(),
                cache.clone(),
            );
            let Some(batch) = stream.next().await else { break };
            let batch = batch.unwrap();
            let column = arrow::array::AsArray::as_primitive::<arrow::datatypes::Int32Type>(
                batch.column(0).as_ref(),
            );
            delivered.extend(column.values().iter().copied());
            token = Some(stream.resume_token());
        }
        assert_eq!(delivered, (1..=19).collect::<Vec<i32>>(), "max_rows {max_rows}");
    }
}
