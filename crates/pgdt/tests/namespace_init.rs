//! `pgdt` as its PID namespace's init: every command ends on a signal that
//! would end it anywhere else, exiting `128 + n` because the kernel will not
//! let it die by one, but `sql` on `SIGINT` in the REPL, which upstream's
//! `ctrl_c` answers there (`docs/design/decisions.md`, "D26";
//! `docs/design/runtime-invariants.md`, "RT19").
//!
//! Each run is held at a request its origin stalls — the oracle's for the
//! native commands, a listener that never answers for `sql` — so the signal
//! lands in a wait rather than racing the start-up.

mod common;

use std::net::TcpListener;
use std::os::unix::process::ExitStatusExt as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use common::fixture;
use common::oracle::Oracle;
use namespace_init::unshare::{NamespaceInit, as_namespace_init};

/// Long enough that a stall ending by itself would be unmistakable.
const STALL: Duration = Duration::from_secs(60);

fn dump_bytes() -> Vec<u8> {
    std::fs::read(fixture("16/edge_cases/default.sql")).unwrap()
}

/// `pgdt args…` as its namespace's init, held until the oracle has received
/// its `ordinal`th request.
fn held_at(
    oracle: &Oracle,
    ordinal: usize,
    dir: &tempfile::TempDir,
    args: &[&str],
) -> NamespaceInit {
    let mut command = as_namespace_init(env!("CARGO_BIN_EXE_pgdt"));
    command.args(args).current_dir(dir.path());
    let init = NamespaceInit::spawn(command);
    let deadline = Instant::now() + Duration::from_secs(20);
    while oracle.request_count() < ordinal {
        assert!(Instant::now() < deadline, "request {ordinal} never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
    init
}

/// **As init, a command ends on the signals that end it elsewhere**, not only
/// Ctrl-C and `docker stop`'s: each is held at the origin's probe, before
/// `parse` has installed its guard, and signalled from the host.
#[test]
fn as_its_namespace_s_init_every_command_ends_on_a_signal() {
    let cases: [(&str, &str, i32); 5] = [
        ("query", "INT", 130),
        ("query", "TERM", 143),
        ("query", "HUP", 129),
        ("query", "QUIT", 131),
        ("parse", "TERM", 143),
    ];
    for (command, signal, code) in cases {
        let oracle = Oracle::serving(dump_bytes()).stalling_request(1, STALL).start();
        let dir = tempfile::tempdir().unwrap();
        let url = oracle.url();
        let args: Vec<&str> = match command {
            "query" => vec!["query", "--source", &url, "--table", "public.widgets"],
            _ => vec!["parse", "--source", &url],
        };
        let init = held_at(&oracle, 1, &dir, &args);
        init.signal(signal);
        let (status, stderr) = init.wait();
        // An exit, not a death by the signal: the kernel shields an init from
        // its own raise, so `128 + n` is what a runtime would report anyway.
        assert_eq!(status.signal(), None, "{command} {signal}: {status:?} {stderr}");
        assert_eq!(status.code(), Some(code), "{command} {signal}: {stderr}");
    }
}

/// **An interrupted `parse` as init saves, then exits 130**: the guard keeps
/// `SIGINT` for its scan, and the re-raise it ends with elsewhere would be
/// dropped here.
///
/// Held at the first read past the file's midpoint, by which the preamble is
/// banked, so there is a scan to save. Which request that is comes from a run
/// nothing stalls.
#[test]
fn as_its_namespace_s_init_an_interrupted_parse_saves_and_exits_130() {
    let args = |url: &str, cache: &str| {
        ["parse", "--source", url, "--dtcache", cache, "--chunk-size", "512", "--jobs", "1"]
            .map(str::to_owned)
    };
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("dump.dtcache");
    let cache = cache.to_str().unwrap();

    let reference = Oracle::serving(dump_bytes()).start();
    let out = common::pgdt().args(args(&reference.url(), cache)).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_file(cache).unwrap();
    let midpoint = dump_bytes().len() / 2;
    let ordinal = 1 + reference
        .requests()
        .iter()
        .position(|request| {
            let start = request.range().and_then(|r| r.strip_prefix("bytes=")?.split('-').next());
            start.and_then(|s| s.parse::<usize>().ok()).is_some_and(|s| s >= midpoint)
        })
        .expect("a read past the midpoint");

    let oracle = Oracle::serving(dump_bytes()).stalling_request(ordinal, STALL).start();
    let init =
        held_at(&oracle, ordinal, &dir, &args(&oracle.url(), cache).each_ref().map(String::as_str));
    init.signal("INT");
    let (status, stderr) = init.wait();

    assert_eq!(status.signal(), None, "{status:?} {stderr}");
    assert_eq!(status.code(), Some(130), "{stderr}");
    assert!(stderr.contains("interrupted at byte"), "{stderr}");
    assert!(!stderr.contains("interrupted at byte 0 "), "a scan was banked: {stderr}");
    assert!(stderr.contains(cache), "the saved cache is named: {stderr}");
    assert!(std::path::Path::new(cache).exists(), "and is on disk");
}

/// **`sql -c` ends on `SIGINT`, and the REPL on `SIGTERM`**, each held at a
/// `--dump` whose origin accepts the connection and never answers. The
/// connection arriving is what says the handlers are installed: they are,
/// before any dump is registered.
#[test]
fn as_its_namespace_s_init_the_sql_shell_ends_on_a_signal() {
    let cases: [(&[&str], &str, i32); 2] = [(&["-c", "SELECT 1"], "INT", 130), (&[], "TERM", 143)];
    for (mode, signal, code) in cases {
        let origin = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        origin.set_nonblocking(true).unwrap();
        let url = format!("http://{}/dump.sql", origin.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();

        let mut command = as_namespace_init(env!("CARGO_BIN_EXE_pgdt"));
        command
            .args(["sql", "-q", "--dump", &url])
            .args(mode)
            .current_dir(dir.path())
            .stdin(Stdio::null());
        let init = NamespaceInit::spawn(command);
        let deadline = Instant::now() + Duration::from_secs(20);
        let _held = loop {
            match origin.accept() {
                Ok((connection, _)) => break connection,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "the dump's origin was never asked");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        };

        init.signal(signal);
        let (status, stderr) = init.wait();
        assert_eq!(status.signal(), None, "{mode:?} {signal}: {status:?} {stderr}");
        assert_eq!(status.code(), Some(code), "{mode:?} {signal}: {stderr}");
    }
}
