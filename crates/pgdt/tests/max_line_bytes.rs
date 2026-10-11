//! `pgdt parse --max-line-bytes` / `pgdt query --max-line-bytes` — the line
//! limit's CLI surface.
//!
//! The library holds what the limit means (`scan.rs`'s
//! `line_length_limit_is_enforced`). What only the binary can say is that the
//! flag reaches both scanning commands' options — a flag parsed and dropped
//! would leave a dump with a value past the default refused with no way
//! through — and that a stated limit reaches the serial scan and the leader's
//! split alike. The dump is the `statistics` fixture, whose `long_value` table
//! holds a value longer than a read chunk.
//!
//! **The refusing limit sits well under one read chunk**, because the limit
//! is checked as each read completes: a line may overrun it by up to a chunk
//! before it is refused, so a limit just under the long line would pass it.

use std::process::Output;

mod common;
use common::{fixture, run, sandboxed, stderr_of, stdout_of};

const DUMP: &str = "16/statistics/default.sql";

/// Far below the long value, and below a read chunk.
const REFUSING: &str = "65536";

fn parse(extra: &[&str]) -> Output {
    let (_dir, dump) = sandboxed(DUMP, "long.sql");
    let mut args = vec!["parse", "--source", dump.to_str().unwrap()];
    args.extend_from_slice(extra);
    run(&args)
}

fn query(extra: &[&str]) -> Output {
    let dump = fixture(DUMP);
    let mut args = vec![
        "query",
        "--source",
        dump.to_str().unwrap(),
        "--table",
        "public.long_value",
        "--column",
        "id",
        "--dtcache",
        "none",
    ];
    args.extend_from_slice(extra);
    run(&args)
}

fn assert_refused(out: &Output, what: &str) {
    assert!(!out.status.success(), "{what} should refuse the long line");
    assert!(
        stderr_of(out).contains("exceeds the 65536-byte line limit"),
        "{what} should name the stated limit: {}",
        stderr_of(out)
    );
}

#[test]
fn parse_takes_the_stated_limit_on_the_serial_scan_and_the_leader_split() {
    assert!(parse(&[]).status.success(), "the default limit holds the long value");
    assert_refused(&parse(&["--max-line-bytes", REFUSING]), "a serial parse");
    // Small enough that the long value's block is cut into pieces, so the
    // limit is the split's to apply as well as the serial scanner's.
    assert_refused(
        &parse(&[
            "--max-line-bytes",
            REFUSING,
            "--jobs",
            "8",
            "--chunk-size",
            "512",
            "--statistics-level",
            "metadata",
        ]),
        "a split parse",
    );
}

#[test]
fn query_takes_the_stated_limit_and_a_raised_one_changes_no_row() {
    let reference = query(&[]);
    assert!(reference.status.success(), "{}", stderr_of(&reference));
    assert_refused(&query(&["--max-line-bytes", REFUSING]), "a query");
    let raised = query(&["--max-line-bytes", "268435456"]);
    assert!(raised.status.success(), "{}", stderr_of(&raised));
    assert_eq!(stdout_of(&raised), stdout_of(&reference));
}

#[test]
fn a_zero_line_limit_is_refused() {
    let out = query(&["--max-line-bytes", "0"]);
    assert!(!out.status.success());
    assert!(stderr_of(&out).contains("line limit of 0"), "{}", stderr_of(&out));
}
