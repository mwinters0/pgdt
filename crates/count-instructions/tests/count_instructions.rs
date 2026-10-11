//! `count-instructions` run as the harness runs it: around a command, its
//! streams read back and its exit status asked.
//!
//! Every test that counts needs `perf_event_open` for one's own processes,
//! which `perf_event_paranoid` up to 2 grants without a capability; where the
//! kernel refuses it, the counter exits 1 before running anything and these
//! tests say so rather than pass.

use std::process::{Command, Output};

const COUNT_INSTRUCTIONS: &str = env!("CARGO_BIN_EXE_count-instructions");

/// What a child of the self-invoked test binary spins through, per thread.
const CHILD_ITERATIONS: u64 = 50_000_000;
const CHILD_THREADS: u64 = 4;
const CHILD_VAR: &str = "COUNT_INSTRUCTIONS_TEST_CHILD";

fn run(args: &[&str]) -> Output {
    Command::new(COUNT_INSTRUCTIONS).args(args).output().expect("count-instructions starts")
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
fn a_clean_exit_reports_one_count_and_its_code() {
    let out = run(&["true"]);
    let text = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert_eq!(values(&text, "instructions_u").len(), 1, "{text}");
    assert!(values(&text, "signal").is_empty(), "{text}");
}

#[test]
fn the_commands_own_exit_code_passes_through() {
    let out = run(&["sh", "-c", "exit 7"]);
    let text = stderr(&out);
    assert_eq!(out.status.code(), Some(7), "{text}");
    assert_eq!(values(&text, "instructions_u").len(), 1, "{text}");
}

#[test]
fn a_signal_death_exits_128_plus_its_number_and_names_it() {
    let out = run(&["sh", "-c", "kill -TERM $$"]);
    let text = stderr(&out);
    assert_eq!(out.status.code(), Some(128 + libc::SIGTERM), "{text}");
    assert_eq!(values(&text, "instructions_u").len(), 1, "{text}");
    assert_eq!(values(&text, "signal"), vec![libc::SIGTERM as u64], "{text}");
}

#[test]
fn stdout_is_the_commands_alone() {
    let out = run(&["echo", "hello"]);
    assert_eq!(out.stdout, b"hello\n", "{}", stderr(&out));
}

#[test]
fn a_command_that_cannot_start_reports_no_count() {
    let out = run(&["/nonexistent/count-instructions-test-program"]);
    assert_eq!(out.status.code(), Some(127));
    assert!(values(&stderr(&out), "instructions_u").is_empty());
}

#[test]
fn no_command_is_a_usage_error() {
    let out = run(&[]);
    assert_eq!(out.status.code(), Some(2));
}

/// The count is every thread's: a child whose `CHILD_THREADS` threads each
/// retire at least `CHILD_ITERATIONS` instructions reads at least their sum,
/// which a counter on the main thread alone could not.
#[test]
fn the_count_is_every_threads() {
    let me = std::env::current_exe().expect("the test binary's path");
    let out = Command::new(COUNT_INSTRUCTIONS)
        .arg(me)
        .args(["--exact", "child_spins_in_threads", "--ignored", "--nocapture"])
        .env(CHILD_VAR, "1")
        .output()
        .expect("count-instructions starts");
    let text = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{text}");
    let count = values(&text, "instructions_u");
    assert_eq!(count.len(), 1, "{text}");
    assert!(count[0] >= CHILD_THREADS * CHILD_ITERATIONS, "{text}");
}

/// The child half of `the_count_is_every_threads`, which runs it with
/// `CHILD_VAR` set; a run without it does nothing.
#[test]
#[ignore = "run by the_count_is_every_threads as its child"]
fn child_spins_in_threads() {
    if std::env::var_os(CHILD_VAR).is_none() {
        return;
    }
    let threads: Vec<_> = (0..CHILD_THREADS)
        .map(|_| {
            std::thread::spawn(|| {
                let mut acc = 0u64;
                for i in 0..CHILD_ITERATIONS {
                    acc = std::hint::black_box(acc.wrapping_add(i));
                }
                acc
            })
        })
        .collect();
    for thread in threads {
        std::hint::black_box(thread.join().expect("a spinning thread"));
    }
}
