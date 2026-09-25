//! `datafusion-cli-pgdump` as its PID namespace's init: it ends on a signal
//! that would end it anywhere else, exiting `128 + n`, but on `SIGINT` in the
//! REPL, which upstream's `ctrl_c` answers there (`docs/design/decisions.md`,
//! "D26"; `docs/design/runtime-invariants.md`, "RT19").

use std::net::TcpListener;
use std::os::unix::process::ExitStatusExt as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

use namespace_init::unshare::{NamespaceInit, as_namespace_init};

/// **`-c` ends on `SIGINT`, and the REPL on `SIGTERM`**, each held at a
/// `--dump` whose origin accepts the connection and never answers. The
/// connection arriving is what says the handlers are installed: they are,
/// before any dump is registered.
#[test]
fn as_its_namespace_s_init_the_shell_ends_on_a_signal() {
    let cases: [(&[&str], &str, i32); 2] = [(&["-c", "SELECT 1"], "INT", 130), (&[], "TERM", 143)];
    for (mode, signal, code) in cases {
        let origin = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        origin.set_nonblocking(true).unwrap();
        let url = format!("http://{}/dump.sql", origin.local_addr().unwrap());
        let dir = tempfile::tempdir().unwrap();

        let mut command = as_namespace_init(env!("CARGO_BIN_EXE_datafusion-cli-pgdump"));
        command
            .args(["-q", "--dump", &url])
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
