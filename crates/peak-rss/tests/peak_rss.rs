//! `peak-rss` run as the harness runs it: around a command, its streams read
//! back and its exit status asked.

use std::process::{Command, Output};

const PEAK_RSS: &str = env!("CARGO_BIN_EXE_peak-rss");

/// What a child of the self-invoked test binary allocates and touches.
const CHILD_BYTES: usize = 64 << 20;
const CHILD_VAR: &str = "PEAK_RSS_TEST_CHILD";

fn run(args: &[&str]) -> Output {
    Command::new(PEAK_RSS).args(args).output().expect("peak-rss starts")
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("stderr is UTF-8")
}

/// Every `key=<n>` line of `text`, in order.
fn values(text: &str, key: &str) -> Vec<u64> {
    let prefix = format!("{key}=");
    text.lines()
        .filter_map(|l| l.strip_prefix(&prefix))
        .map(|v| v.parse().expect("a decimal reading"))
        .collect()
}

#[test]
fn a_clean_exit_reports_one_reading_and_its_code() {
    let out = run(&["true"]);
    assert_eq!(out.status.code(), Some(0));
    let text = stderr(&out);
    assert_eq!(values(&text, "maxrss_kib").len(), 1, "{text}");
    assert!(values(&text, "signal").is_empty(), "{text}");
}

#[test]
fn the_commands_own_exit_code_passes_through() {
    let out = run(&["sh", "-c", "exit 7"]);
    assert_eq!(out.status.code(), Some(7));
    assert_eq!(values(&stderr(&out), "maxrss_kib").len(), 1);
}

#[test]
fn a_signal_death_exits_128_plus_its_number_and_names_it() {
    let out = run(&["sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(128 + libc::SIGTERM));
    let text = stderr(&out);
    assert_eq!(values(&text, "maxrss_kib").len(), 1, "{text}");
    assert_eq!(values(&text, "signal"), vec![libc::SIGTERM as u64], "{text}");
}

#[test]
fn stdout_is_the_commands_alone() {
    let out = run(&["echo", "hello"]);
    assert_eq!(out.stdout, b"hello\n");
}

#[test]
fn a_command_that_cannot_start_reports_no_reading() {
    let out = run(&["/nonexistent/peak-rss-test-program"]);
    assert_eq!(out.status.code(), Some(127));
    assert!(values(&stderr(&out), "maxrss_kib").is_empty());
}

#[test]
fn no_command_is_a_usage_error() {
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
}

/// The reading is the child's peak: a child that touched `CHILD_BYTES` reads
/// at least that much.
#[test]
fn the_reading_is_the_childs_peak() {
    let me = std::env::current_exe().expect("the test binary's path");
    let out = Command::new(PEAK_RSS)
        .arg(me)
        .args(["--exact", "child_touches_its_bytes", "--ignored", "--nocapture"])
        .env(CHILD_VAR, "1")
        .output()
        .expect("peak-rss starts");
    let text = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    let kib = values(&text, "maxrss_kib");
    assert_eq!(kib.len(), 1, "{text}");
    assert!(kib[0] >= (CHILD_BYTES >> 10) as u64, "{text}");
}

/// The child half of `the_reading_is_the_childs_peak`, which runs it with
/// `CHILD_VAR` set; a run without it does nothing.
#[test]
#[ignore = "run by the_reading_is_the_childs_peak as its child"]
fn child_touches_its_bytes() {
    if std::env::var_os(CHILD_VAR).is_none() {
        return;
    }
    let mut bytes = vec![0u8; CHILD_BYTES];
    for page in bytes.chunks_mut(4096) {
        page[0] = 1;
    }
    std::hint::black_box(&bytes);
}
