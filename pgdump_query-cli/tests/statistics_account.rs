//! The statistics account reconciled against the instrument build's live heap
//! (`docs/design/decisions.md`, "D81").
//!
//! **Compiled only with `--features introspect`**, the one build whose
//! allocator counts what the library attributes to statistics
//! (`pgdump_query::instrument`):
//!
//! ```sh
//! cargo test -p pgdump_query-cli --features introspect --test statistics_account \
//!     --target-dir <own> -- --nocapture
//! ```
//!
//! Two generated shapes — a table of reasonable width and one of wide text —
//! each through four `parse` legs: serial at a stated group size small enough
//! to gather thousands of groups, split across four workers, a re-parse at
//! half that size that loads the first leg's cache and back-fills every
//! block, and flagless. Half rather than twice: a group twice as wide holds
//! more distinct texts than a dictionary keeps, and a leg gathering almost
//! nothing judges nothing. Each leg's report is held to the tolerance
//! registered before the first reading:
//!
//! - **at return**, the account and the live statistics bytes agree within
//!   [`RETURN_PER_MILLE`] of live plus [`RETURN_FLOOR`], either way;
//! - **at every update of the account**, it is short of live by no more than
//!   `STATISTICS_SLACK_PER_MILLE` of live plus [`UPDATE_FLOOR`];
//! - **at the peak**, the account's is short of live's by no more than the
//!   same.
//!
//! The per-leg readings are printed, for a sitting's `runs/` artifact.
#![cfg(feature = "introspect")]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

mod common;
use common::{pgdq, stderr_of};

/// The proportional half of the agreement at return, per mille of live.
const RETURN_PER_MILLE: u64 = 10;
/// The fixed half of the agreement at return.
const RETURN_FLOOR: u64 = 64 << 10;
/// The fixed half of the shortfall allowed at an update and at the peak.
const UPDATE_FLOOR: u64 = 1 << 20;
/// The least live statistics a leg at a stated group size must end holding,
/// so that the proportional tolerances are what it is judged by. A flagless
/// leg gathers too few groups of either shape to meet it, and is judged by
/// the floors.
const MEANINGFUL: u64 = 8 << 20;

/// Each generated input's size.
const INPUT_BYTES: usize = 48 << 20;

/// SplitMix64, seeded, so an input is the same bytes every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

const WORDS: &[&str] = &[
    "lorem",
    "ipsum",
    "dolor",
    "sit",
    "amet",
    "consectetur",
    "adipiscing",
    "elit",
    "sed",
    "do",
    "eiusmod",
    "tempor",
    "incididunt",
    "ut",
    "labore",
    "et",
    "dolore",
    "magna",
    "aliqua",
    "enim",
];

fn words(rng: &mut Rng, count: u64) -> String {
    let mut out = String::new();
    for i in 0..count {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
    }
    out
}

/// A dump of one table, `columns` declared as given, rows from `row` until
/// the data passes [`INPUT_BYTES`].
fn write_dump(
    path: &Path,
    table: &str,
    columns: &[(String, &str)],
    mut row: impl FnMut(&mut Rng, u64) -> String,
) {
    let mut text = String::new();
    let declared: Vec<String> = columns.iter().map(|(n, t)| format!("    {n} {t}")).collect();
    writeln!(text, "CREATE TABLE {table} (\n{}\n);\n", declared.join(",\n")).unwrap();
    let names: Vec<&str> = columns.iter().map(|(n, _)| n.as_str()).collect();
    writeln!(text, "COPY {table} ({}) FROM stdin;", names.join(", ")).unwrap();
    let mut rng = Rng(0x0574_7157_11c5);
    let start = text.len();
    let mut id = 0;
    while text.len() - start < INPUT_BYTES {
        id += 1;
        text.push_str(&row(&mut rng, id));
        text.push('\n');
    }
    text.push_str("\\.\n\n");
    std::fs::File::create(path).unwrap().write_all(text.as_bytes()).unwrap();
}

/// Rows of reasonable width: eight columns of the common kinds, a short text
/// from a small pool, a SKU from a larger one and a note of a few words.
fn reasonable(path: &Path) {
    let columns = [
        ("id", "bigint"),
        ("customer", "integer"),
        ("placed", "timestamp without time zone"),
        ("amount", "numeric(12,2)"),
        ("status", "text"),
        ("sku", "character varying(32)"),
        ("note", "text"),
        ("paid", "boolean"),
    ]
    .map(|(n, t)| (n.to_string(), t));
    write_dump(path, "public.orders", &columns, |rng, id| {
        let status =
            ["new", "paid", "packed", "shipped", "returned", "void"][rng.below(6) as usize];
        let note_words = 3 + rng.below(10);
        let note = words(rng, note_words);
        format!(
            "{id}\t{}\t2026-{:02}-{:02} {:02}:{:02}:{:02}\t{}.{:02}\t{status}\tSKU-{:05}\t{}\t{}",
            rng.below(100_000),
            1 + rng.below(12),
            1 + rng.below(28),
            rng.below(24),
            rng.below(60),
            rng.below(60),
            rng.below(10_000),
            rng.below(100),
            rng.below(500),
            note,
            if rng.below(2) == 0 { "t" } else { "f" },
        )
    });
}

/// Rows of wide text: sixty-four text columns, each drawing one of its own
/// forty-eight phrases, so a group's dictionary on every column holds.
fn wide_text(path: &Path) {
    const COLUMNS: usize = 64;
    let mut pools = Rng(0x7ea7);
    let phrases: Vec<Vec<String>> = (0..COLUMNS)
        .map(|c| (0..48).map(|p| format!("c{c}p{p} {}", words(&mut pools, 4 + p % 20))).collect())
        .collect();
    let columns: Vec<(String, &str)> = (0..COLUMNS).map(|c| (format!("t{c}"), "text")).collect();
    write_dump(path, "public.wide", &columns, |rng, _| {
        let fields: Vec<&str> = phrases
            .iter()
            .map(|pool| pool[rng.below(pool.len() as u64) as usize].as_str())
            .collect();
        fields.join("\t")
    });
}

/// `pgdq parse` over `dump` with `extra`, and the report it wrote.
fn parse(dump: &Path, report: &Path, extra: &[&str]) -> HashMap<String, u64> {
    let _ = std::fs::remove_file(report);
    let mut command = pgdq();
    command.args(["parse", "--source", dump.to_str().unwrap()]).args(extra);
    let out = command.env("PGDQ_INTROSPECT_OUT", report).output().unwrap();
    assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
    std::fs::read_to_string(report)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter_map(|(key, value)| Some((key.to_string(), value.parse().ok()?)))
        .collect()
}

/// Hold one leg's report to the registered tolerance, and say what it read.
fn reconcile(label: &str, report: &HashMap<String, u64>, stated: bool) -> String {
    let get = |key: &str| *report.get(key).unwrap_or_else(|| panic!("{label}: no {key}"));
    let (account, live) = (get("statistics_account_bytes"), get("statistics_live_bytes"));
    let (account_peak, live_peak) =
        (get("statistics_account_peak_bytes"), get("statistics_live_peak_bytes"));
    let slack = get("statistics_slack_per_mille");
    let line = format!(
        "{label}: at return account={account} live={live}; peak account={account_peak} \
         live={live_peak}; checks={}; worst shortfall={} at live={}, past slack={}; \
         term peaks retained={} loaded={} gathering={} pieces={} interned={}",
        get("statistics_checks"),
        get("statistics_worst_shortfall_bytes"),
        get("statistics_worst_shortfall_live_bytes"),
        get("statistics_worst_shortfall_past_slack_bytes"),
        get("statistics_account_retained_peak_bytes"),
        get("statistics_account_loaded_peak_bytes"),
        get("statistics_account_gathering_peak_bytes"),
        get("statistics_account_pieces_peak_bytes"),
        get("statistics_account_interned_peak_bytes"),
    );
    assert!(!stated || live >= MEANINGFUL, "{line}\n{label}: too little gathered to judge");
    assert!(get("statistics_checks") > 0, "{line}");
    let at_return = live * RETURN_PER_MILLE / 1000 + RETURN_FLOOR;
    assert!(account.abs_diff(live) <= at_return, "{line}\n{label}: disagree at return");
    assert!(
        get("statistics_worst_shortfall_past_slack_bytes") <= UPDATE_FLOOR,
        "{line}\n{label}: short at an update"
    );
    let at_peak = live_peak * slack / 1000 + UPDATE_FLOOR;
    assert!(live_peak.saturating_sub(account_peak) <= at_peak, "{line}\n{label}: short at peak");
    eprintln!("{line}");
    line
}

/// A generated shape: its name, its generator, the group size its first legs
/// state and the size its back-fill states.
type Shape = (&'static str, fn(&Path), &'static str, &'static str);

/// A private copy of `source` named `name` in `dir`, so each leg writes its
/// own cache beside it.
fn copy(dir: &Path, source: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::copy(source, &path).unwrap();
    path
}

/// **The account never falls short of the live statistics bytes past the
/// registered tolerance**, over both shapes and all four legs.
#[test]
fn the_account_reconciles_with_the_live_statistics_heap() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.txt");
    let mut lines = Vec::new();
    let shapes: [Shape; 2] =
        [("reasonable", reasonable, "4096", "2048"), ("wide-text", wide_text, "16384", "8192")];
    for (shape, generate, size, refill_size) in shapes {
        let input = dir.path().join(format!("{shape}.sql"));
        generate(&input);

        let serial = copy(dir.path(), &input, &format!("{shape}-serial.sql"));
        let read = parse(&serial, &report, &["--statistics-group-size", size, "--jobs", "1"]);
        lines.push(reconcile(&format!("{shape} serial"), &read, true));

        let split = copy(dir.path(), &input, &format!("{shape}-split.sql"));
        let workers = [
            "--statistics-group-size",
            size,
            "--jobs",
            "4",
            "--parallel-memory",
            "1073741824",
            "--chunk-size",
            "1048576",
        ];
        let read = parse(&split, &report, &workers);
        assert!(read["statistics_account_pieces_peak_bytes"] > 0, "{shape}: nothing split");
        lines.push(reconcile(&format!("{shape} split"), &read, true));

        let refill = ["--statistics-group-size", refill_size, "--jobs", "1"];
        let read = parse(&serial, &report, &refill);
        assert!(read["statistics_account_loaded_peak_bytes"] > 0, "{shape}: nothing loaded");
        lines.push(reconcile(&format!("{shape} back-fill"), &read, true));

        let flagless = copy(dir.path(), &input, &format!("{shape}-flagless.sql"));
        let read = parse(&flagless, &report, &[]);
        lines.push(reconcile(&format!("{shape} flagless"), &read, false));

        for name in ["serial", "split", "flagless"] {
            let _ = std::fs::remove_file(dir.path().join(format!("{shape}-{name}.sql")));
        }
    }
    assert_eq!(lines.len(), 8);
}
