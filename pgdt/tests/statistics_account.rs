//! The statistics account reconciled against the instrument build's live heap
//! (`docs/design/decisions.md`, "D81").
//!
//! **Compiled only with `--features introspect`**, the one build whose
//! allocator counts what the library attributes to statistics
//! (`pgdump_query::instrument`):
//!
//! ```sh
//! cargo test -p pgdt --features introspect --test statistics_account \
//!     --target-dir <own> -- --nocapture
//! ```
//!
//! Three generated shapes. A table of reasonable width and one of wide text
//! each go through four `parse` legs: serial at a stated group size small
//! enough to gather thousands of groups, split across four workers, a re-parse
//! at half that size that loads the first leg's cache and back-fills every
//! block, and flagless. Half rather than twice: a group twice as wide holds
//! more distinct texts than a dictionary keeps, and a leg gathering almost
//! nothing judges nothing. The third, sixty-four text columns of distinct
//! values at least `DICTIONARY_ENTRY_MAX_BYTES` long, goes through a flagless leg alone:
//! a group of about sixty rows grows each column's open group by a stored
//! value a row, towards the most a column holds open, so an observer passes a
//! step every few rows. Each leg's report is held to the tolerance registered
//! before the first reading, **in bytes and either way, with no proportional
//! slack** — `S` being `STATISTICS_ACCOUNT_CHARGE_STEP`, the most growth an observer holds
//! uncharged, and `R` the input's longest row, the most a column's decode holds
//! beside it:
//!
//! - **at every update of the account**, it differs from the live statistics
//!   bytes by no more than `S` for each observer open then, plus `R` — the
//!   report's `statistics_worst_*_past_allowance_bytes`, the allowance being
//!   `S` for each observer;
//! - **at the peak**, the account's differs from live's by no more than the
//!   largest allowance any update was given, plus `R`;
//! - **at return**, with no observer open, by no more than `R`.
//!
//! An allocation the account charges ahead of making it counts as made when
//! the account is judged short and as not yet made when it is judged over
//! (`pgdump_query::instrument`).
//!
//! The per-leg readings are printed, for a sitting's `runs/` artifact.
#![cfg(feature = "introspect")]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

mod common;
use common::{pgdt, stderr_of};

/// The least live statistics a leg at a stated group size must end holding,
/// so that it judges a pass that gathered. A flagless leg gathers too few
/// groups to meet it.
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
/// the data passes [`INPUT_BYTES`]; answers its longest row, in bytes.
fn write_dump(
    path: &Path,
    table: &str,
    columns: &[(String, &str)],
    mut row: impl FnMut(&mut Rng, u64) -> String,
) -> u64 {
    let mut text = String::new();
    let declared: Vec<String> = columns.iter().map(|(n, t)| format!("    {n} {t}")).collect();
    writeln!(text, "CREATE TABLE {table} (\n{}\n);\n", declared.join(",\n")).unwrap();
    let names: Vec<&str> = columns.iter().map(|(n, _)| n.as_str()).collect();
    writeln!(text, "COPY {table} ({}) FROM stdin;", names.join(", ")).unwrap();
    let mut rng = Rng(0x0574_7157_11c5);
    let start = text.len();
    let (mut id, mut longest) = (0, 0);
    while text.len() - start < INPUT_BYTES {
        id += 1;
        let line = row(&mut rng, id);
        longest = longest.max(line.len() as u64);
        text.push_str(&line);
        text.push('\n');
    }
    text.push_str("\\.\n\n");
    std::fs::File::create(path).unwrap().write_all(text.as_bytes()).unwrap();
    longest
}

/// Rows of reasonable width: eight columns of the common kinds, a short text
/// from a small pool, a SKU from a larger one and a note of a few words.
fn reasonable(path: &Path) -> u64 {
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
    })
}

/// Rows of wide text: sixty-four text columns, each drawing one of its own
/// forty-eight phrases, so a group's dictionary on every column holds.
fn wide_text(path: &Path) -> u64 {
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
    })
}

/// Rows of long distinct text: sixty-four text columns whose values are each
/// `DICTIONARY_ENTRY_MAX_BYTES` bytes long and never repeat, but for one in sixty-four a
/// byte longer — so a group's distinct texts grow row by row to the most a
/// dictionary keeps before one passes a cap.
fn long_text(path: &Path) -> u64 {
    const COLUMNS: usize = 64;
    const DICTIONARY_ENTRY_MAX_BYTES: usize = 256;
    let columns: Vec<(String, &str)> = (0..COLUMNS).map(|c| (format!("l{c}"), "text")).collect();
    write_dump(path, "public.long", &columns, |rng, id| {
        let fields: Vec<String> = (0..COLUMNS)
            .map(|c| {
                let length = DICTIONARY_ENTRY_MAX_BYTES + usize::from(rng.below(64) == 0);
                let mut value = format!("{id}-{c}-{} ", rng.next());
                while value.len() < length {
                    value.push_str(WORDS[rng.below(WORDS.len() as u64) as usize]);
                }
                value.truncate(length);
                value
            })
            .collect();
        fields.join("\t")
    })
}

/// `pgdt parse` over `dump` with `extra`, and the report it wrote.
fn parse(dump: &Path, report: &Path, extra: &[&str]) -> HashMap<String, u64> {
    let _ = std::fs::remove_file(report);
    let mut command = pgdt();
    command.args(["parse", "--source", dump.to_str().unwrap()]).args(extra);
    let out = command.env("PGDT_INTROSPECT_OUT", report).output().unwrap();
    assert!(out.status.success(), "{extra:?}: {}", stderr_of(&out));
    std::fs::read_to_string(report)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter_map(|(key, value)| Some((key.to_string(), value.parse().ok()?)))
        .collect()
}

/// Hold one leg's report to the registered tolerance, `longest` being the
/// input's longest row, and say what it read.
fn reconcile(label: &str, report: &HashMap<String, u64>, stated: bool, longest: u64) -> String {
    let get = |key: &str| *report.get(key).unwrap_or_else(|| panic!("{label}: no {key}"));
    let (account, live) = (get("statistics_account_bytes"), get("statistics_live_bytes"));
    let (account_peak, live_peak) =
        (get("statistics_account_peak_bytes"), get("statistics_live_peak_bytes"));
    let allowance = get("statistics_allowance_peak_bytes");
    let line = format!(
        "{label}: at return account={account} live={live}; peak account={account_peak} \
         live={live_peak}; checks={}; allowance peak={allowance}; longest row={longest}; \
         worst short={} past allowance={}; worst over={} past allowance={}; term peaks retained={} \
         loaded={} gathering={} pieces={} interned={}",
        get("statistics_checks"),
        get("statistics_worst_short_bytes"),
        get("statistics_worst_short_past_allowance_bytes"),
        get("statistics_worst_over_bytes"),
        get("statistics_worst_over_past_allowance_bytes"),
        get("statistics_account_retained_peak_bytes"),
        get("statistics_account_loaded_peak_bytes"),
        get("statistics_account_gathering_peak_bytes"),
        get("statistics_account_pieces_peak_bytes"),
        get("statistics_account_interned_peak_bytes"),
    );
    assert!(!stated || live >= MEANINGFUL, "{line}\n{label}: too little gathered to judge");
    assert!(get("statistics_checks") > 0, "{line}");
    assert!(
        get("statistics_worst_short_past_allowance_bytes") <= longest,
        "{line}\n{label}: short at an update"
    );
    assert!(
        get("statistics_worst_over_past_allowance_bytes") <= longest,
        "{line}\n{label}: over at an update"
    );
    let at_peak = allowance + longest;
    assert!(account_peak.abs_diff(live_peak) <= at_peak, "{line}\n{label}: disagree at peak");
    assert!(account.abs_diff(live) <= longest, "{line}\n{label}: disagree at return");
    eprintln!("{line}");
    line
}

/// A generated shape: its name, its generator, the group size its first legs
/// state and the size its back-fill states.
type Shape = (&'static str, fn(&Path) -> u64, &'static str, &'static str);

/// A private copy of `source` named `name` in `dir`, so each leg writes its
/// own cache beside it.
fn copy(dir: &Path, source: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::copy(source, &path).unwrap();
    path
}

/// **The account agrees with the live statistics bytes within the registered
/// tolerance**, over every shape and leg.
#[test]
fn the_account_reconciles_with_the_live_statistics_heap() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report.txt");
    let mut lines = Vec::new();
    let shapes: [Shape; 2] =
        [("reasonable", reasonable, "4096", "2048"), ("wide-text", wide_text, "16384", "8192")];
    for (shape, generate, size, refill_size) in shapes {
        let input = dir.path().join(format!("{shape}.sql"));
        let longest = generate(&input);

        let serial = copy(dir.path(), &input, &format!("{shape}-serial.sql"));
        let read = parse(&serial, &report, &["--row-group-size", size, "--jobs", "1"]);
        lines.push(reconcile(&format!("{shape} serial"), &read, true, longest));

        let split = copy(dir.path(), &input, &format!("{shape}-split.sql"));
        let workers = [
            "--row-group-size",
            size,
            "--jobs",
            "4",
            "--memory",
            "1073741824",
            "--chunk-size",
            "1048576",
        ];
        let read = parse(&split, &report, &workers);
        assert!(read["statistics_account_pieces_peak_bytes"] > 0, "{shape}: nothing split");
        lines.push(reconcile(&format!("{shape} split"), &read, true, longest));

        let refill = ["--row-group-size", refill_size, "--jobs", "1"];
        let read = parse(&serial, &report, &refill);
        assert!(read["statistics_account_loaded_peak_bytes"] > 0, "{shape}: nothing loaded");
        lines.push(reconcile(&format!("{shape} back-fill"), &read, true, longest));

        let flagless = copy(dir.path(), &input, &format!("{shape}-flagless.sql"));
        let read = parse(&flagless, &report, &[]);
        lines.push(reconcile(&format!("{shape} flagless"), &read, false, longest));

        for name in ["", "-serial", "-split", "-flagless"] {
            let _ = std::fs::remove_file(dir.path().join(format!("{shape}{name}.sql")));
        }
    }
    let input = dir.path().join("long-text.sql");
    let longest = long_text(&input);
    let read = parse(&input, &report, &[]);
    lines.push(reconcile("long-text flagless", &read, false, longest));
    assert_eq!(lines.len(), 9);
}
