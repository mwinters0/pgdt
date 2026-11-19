//! **A line at the real bound is read, and one a byte longer refused**,
//! through `pgdt parse` and `pgdt query` over a plain and an `.xz` source
//! (`docs/design/decisions.md`, "D164"): the constant reaches every path a
//! command takes, and nothing downstream of it bounds a line lower.
//!
//! The library's paths are held to both edges of the bound at a small one
//! (`pgdt_dump::ReadOptions::line_bound`, a field only `test-support`
//! builds have); what only a run at the real one shows is the wiring, which
//! a small bound stands in for. Each dump is a gibibyte, so this binary runs
//! alone in its nextest group of one thread (`.config/nextest.toml`): one
//! such test, never real-bound tests throughout, and never the small bound
//! with the constant asserted alone, which proves nothing of the wiring.
//! It never reaches the provider or the back-fill; and live scans' line
//! charges are one unit test (`two_live_scans_ledgers_lend_no_byte_twice`),
//! no join over two dumps.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod common;
use common::{run, stderr_of, stdout_of};
use pgdt_core::LINE_BOUND;

/// `COPY public.t (id, v)` whose first row's line, its LF included, is
/// `line` bytes long, its second a short one, written to `dir` as `name`
/// without the file ever being held whole.
fn dump(dir: &Path, name: &str, line: usize) -> PathBuf {
    let path = dir.join(name);
    let mut out = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(&path).unwrap());
    out.write_all(b"COPY public.t (id, v) FROM stdin;\n1\t").unwrap();
    let block = vec![b'x'; 1 << 20];
    let mut left = line - b"1\t\n".len();
    while left > 0 {
        let n = left.min(block.len());
        out.write_all(&block[..n]).unwrap();
        left -= n;
    }
    out.write_all(b"\n2\tshort\n\\.\n").unwrap();
    out.into_inner().unwrap().sync_all().unwrap();
    path
}

/// `plain` compressed by `xz` beside it, in blocks a fraction of the line,
/// so the line crosses several.
fn xz(plain: &Path) -> PathBuf {
    let status = Command::new("xz")
        .args(["-0", "-T0", "--block-size=64MiB", "-k", "-f"])
        .arg(plain)
        .status()
        .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
    assert!(status.success(), "xz failed on {}", plain.display());
    std::fs::remove_file(plain).unwrap();
    plain.with_extension("sql.xz")
}

fn parse(source: &Path) -> Output {
    run(&["parse", "--source", source.to_str().unwrap()])
}

fn query(source: &Path) -> Output {
    let source = source.to_str().unwrap();
    run(&["query", "--source", source, "--table", "public.t", "--column", "id", "--cache", "none"])
}

/// Both commands read the dump whose first line is the bound, printing
/// both rows, and refuse the one whose first line passes it by a byte,
/// naming the line's start, its table and the bound.
fn reads_the_bound_and_refuses_past_it(compressed: bool) {
    let dir = tempfile::tempdir().unwrap();
    for (line, refused) in [(LINE_BOUND, false), (LINE_BOUND + 1, true)] {
        let mut source = dump(dir.path(), &format!("line-{line}.sql"), line);
        if compressed {
            source = xz(&source);
        }
        for (what, out) in [("parse", parse(&source)), ("query", query(&source))] {
            let context = format!("{what} of a {line}-byte line, compressed {compressed}");
            if refused {
                assert!(!out.status.success(), "{context}: {}", stdout_of(&out));
                let stderr = stderr_of(&out);
                let named = format!(
                    "the line at offset 34 (`COPY public.t`) is longer than {LINE_BOUND} bytes"
                );
                assert!(stderr.contains(&named), "{context}: {stderr}");
            } else {
                assert!(out.status.success(), "{context}: {}", stderr_of(&out));
                if what == "query" {
                    let printed: Vec<String> = stdout_of(&out).lines().map(str::to_owned).collect();
                    assert_eq!(printed, ["id", "1", "2"], "{context}");
                }
            }
        }
        // A gibibyte apiece: the next dump is not written beside this one.
        std::fs::remove_file(&source).unwrap();
    }
}

#[test]
fn a_plain_line_at_the_bound_is_read_and_one_past_it_refused() {
    reads_the_bound_and_refuses_past_it(false);
}

#[test]
fn an_xz_line_at_the_bound_is_read_and_one_past_it_refused() {
    reads_the_bound_and_refuses_past_it(true);
}
