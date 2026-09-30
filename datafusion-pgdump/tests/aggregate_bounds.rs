//! An ungrouped aggregate's dynamic filter over a column whose first rows are
//! NULL, under the provider's default settings: DataFusion 55.1 loses a bound
//! and a scan pruning under what is left answers wrongly (`KD56`).
//!
//! **This pins upstream behaviour and fails on purpose when it moves**: the
//! flag-on cases assert the wrong answer, so the pin that carries the fix
//! fails them, and that failure is the cue to act on the register entry.
// upstream: UF1

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::util::pretty::pretty_format_batches;
use datafusion::catalog::TableProvider;
use datafusion::execution::session_state::SessionStateBuilder;
use datafusion::prelude::{SessionConfig, SessionContext};
use datafusion_pgdump::{PgDump, PgDumpOptions};
use pgdump_query::cache::{self, CacheMode};
use pgdump_query::{LocalFileSource, ScanOptions, StatisticsRequest, map_file};

mod in_order;
use in_order::{Order, RunScansInOrder};

const ROWS: i64 = 1024;
const GROUP_ROWS: u64 = 64;

/// `t(id, x, a, b)`: `x` NULL in the first two rows, the one batch partition
/// 0 aggregates first, and least in group 5 (1..=64), `1000 + id` elsewhere;
/// `a` 0 and 5000 in the first two rows, between them elsewhere; `b` NULL in
/// the first group, `id` after it.
fn dump_text() -> String {
    let key = "SU7cePawx2YpItQB43yc2KGawpnpmyZHD2gbEDT4iCqSCuRINr6Gct9fLgRrwX7";
    let mut s = format!(
        "--\n-- PostgreSQL database dump\n--\n\n\\restrict {key}\n\n\
-- Dumped from database version 16.15 (Debian 16.15-1.pgdg13+2)\n\
-- Dumped by pg_dump version 16.15 (Debian 16.15-1.pgdg13+2)\n\n\
SET statement_timeout = 0;\nSET lock_timeout = 0;\nSET idle_in_transaction_session_timeout = 0;\n\
SET client_encoding = 'UTF8';\nSET standard_conforming_strings = on;\n\
SELECT pg_catalog.set_config('search_path', '', false);\nSET check_function_bodies = false;\n\
SET xmloption = content;\nSET client_min_messages = warning;\nSET row_security = off;\n\n\
SET default_tablespace = '';\n\nSET default_table_access_method = heap;\n\n\
--\n-- Name: t; Type: TABLE; Schema: public; Owner: postgres\n--\n\n\
CREATE TABLE public.t (\n    id integer,\n    x integer,\n    a integer,\n    b integer\n);\n\n\n\
ALTER TABLE public.t OWNER TO postgres;\n\n\
--\n-- Data for Name: t; Type: TABLE DATA; Schema: public; Owner: postgres\n--\n\n\
COPY public.t (id, x, a, b) FROM stdin;\n"
    );
    for i in 1..=ROWS {
        let x = match i {
            1 | 2 => "\\N".to_owned(),
            _ if (i - 1) / 64 == 5 => (i - 320).to_string(),
            _ => (1000 + i).to_string(),
        };
        let a = match i {
            1 => 0,
            2 => 5000,
            _ => (i % 1000) + 1,
        };
        let b = if i <= 64 { "\\N".to_owned() } else { i.to_string() };
        s.push_str(&format!("{i}\t{x}\t{a}\t{b}\n"));
    }
    s.push_str(&format!(
        "\\.\n\n\n--\n-- PostgreSQL database dump complete\n--\n\n\\unrestrict {key}\n\n"
    ));
    s
}

/// The dump written into `dir` beside a complete, data-level cache.
async fn parsed(dir: &Path) -> PathBuf {
    let copy = dir.join("default.sql");
    std::fs::write(&copy, dump_text()).unwrap();
    let source = LocalFileSource::open(&copy).unwrap();
    let cache = CacheMode::enabled(cache::colocated_path(&copy));
    let request = StatisticsRequest {
        group_size: Some(NonZeroU64::new(GROUP_ROWS).unwrap()),
        ..StatisticsRequest::DATA
    };
    map_file(&source, &ScanOptions::default(), &cache, &request).await.unwrap();
    copy
}

/// Two partitions a scan and batches two rows long, the provider's settings
/// its defaults, and the aggregate's filter on or off.
fn session(aggregate_filter: bool, order: Order) -> SessionContext {
    let options = SessionConfig::new().with_target_partitions(2).with_batch_size(2).set_bool(
        "datafusion.optimizer.enable_aggregate_dynamic_filter_pushdown",
        aggregate_filter,
    );
    SessionContext::new_with_state(
        SessionStateBuilder::new_with_default_features()
            .with_config(options)
            .with_physical_optimizer_rule(Arc::new(RunScansInOrder(order)))
            .build(),
    )
}

/// `sql`'s one row under each partition order, its cells joined by `|`.
async fn answers(aggregate_filter: bool, sql: &str) -> Vec<(Order, String)> {
    let dir = tempfile::tempdir().unwrap();
    let copy = parsed(dir.path()).await;
    let dump = PgDump::open(copy.to_str().unwrap(), PgDumpOptions::default()).await.unwrap();
    let mut out = Vec::new();
    for order in Order::ALL {
        let ctx = session(aggregate_filter, order);
        let provider: Arc<dyn TableProvider> = dump.table(None, None, "t").unwrap();
        ctx.register_table("t", provider).unwrap();
        let batches = ctx.sql(sql).await.unwrap().collect().await.unwrap();
        let shown = pretty_format_batches(&batches).unwrap().to_string();
        let row = shown.lines().nth(3).unwrap().split_whitespace().collect::<String>();
        out.push((order, row));
    }
    out
}

/// A static filter keeps the map from answering, so the rows are read.
const ONE_COLUMN: &str = "SELECT MIN(x), MAX(x) FROM t WHERE id > 0";
const ONE_COLUMN_ANSWER: &str = "|1|2024|";
const TWO_COLUMNS: &str = "SELECT MIN(a), MAX(a), MIN(b), MAX(b) FROM t WHERE id > 0";
const TWO_COLUMNS_ANSWER: &str = "|0|5000|65|1024|";

fn every_order_answers(got: &[(Order, String)], answer: &str) {
    let wrong: Vec<_> = got.iter().filter(|(_, row)| row != answer).collect();
    assert!(wrong.is_empty(), "want {answer}, got {wrong:?}");
}

fn some_order_misses(got: &[(Order, String)], answer: &str) {
    assert!(
        got.iter().any(|(_, row)| row != answer),
        "every order answers {answer}: the pin carries upstream's fix, so act on UF1"
    );
}

/// **A `MIN`'s bound is lost once a partition's first batch holds none of its
/// column**, and a group below every `MIN` seen is pruned.
#[tokio::test]
async fn a_null_first_batch_loses_the_min_bound() {
    some_order_misses(&answers(true, ONE_COLUMN).await, ONE_COLUMN_ANSWER);
}

/// **One column's bounds prune rows another column's aggregates still need**
/// while that column has seen only NULLs.
#[tokio::test]
async fn one_columns_bounds_prune_anothers_first_values() {
    some_order_misses(&answers(true, TWO_COLUMNS).await, TWO_COLUMNS_ANSWER);
}

/// **With the aggregate's filter off, both answer in every order.**
#[tokio::test]
async fn without_the_aggregate_filter_both_answer() {
    every_order_answers(&answers(false, ONE_COLUMN).await, ONE_COLUMN_ANSWER);
    every_order_answers(&answers(false, TWO_COLUMNS).await, TWO_COLUMNS_ANSWER);
}
