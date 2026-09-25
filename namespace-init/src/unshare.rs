//! A binary run as the init of a PID namespace of its own, and signalled from
//! outside it — the position `docker run image pgdt …` puts it in
//! (`docs/design/runtime-invariants.md`, "RT19").
//!
//! **`unshare --user --map-root-user --pid --fork`, no container**: an
//! unprivileged user namespace grants the PID namespace, and `--fork` makes
//! the binary its PID 1. The signal goes to that process by its host pid, as a
//! container runtime's does — never to `unshare`, which ignores `SIGINT` and
//! `SIGTERM` while it waits. `unshare` passes the outcome on as its own: an
//! exit code as that code, a death by a signal as a death by that signal.
//!
//! **A host that refuses unprivileged user namespaces fails these tests
//! loudly** — Ubuntu's AppArmor default from 23.10 — rather than skipping
//! them (`docs/design/roadmap.md`, "A test may assume the tools `mise` pins").
//!
//! Behind `test-support`, which each binary's `[dev-dependencies]` enables.

use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// How long a signalled init has to end: long past a handler's `_exit`, and
/// short of anything the process would finish by itself.
const ENDS_WITHIN: Duration = Duration::from_secs(20);

/// `program` as the init of a fresh PID namespace, ready to take arguments.
pub fn as_namespace_init(program: &str) -> Command {
    let mut command = Command::new("unshare");
    command.args(["--user", "--map-root-user", "--pid", "--fork", program]);
    command
}

/// A spawned [`as_namespace_init`] command, and its init's host pid.
pub struct NamespaceInit {
    unshare: Child,
    init: u32,
}

impl NamespaceInit {
    /// Spawn `command`, stdout and stderr piped, and find the init `unshare`
    /// forked.
    pub fn spawn(mut command: Command) -> Self {
        let mut unshare = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("`unshare` is not runnable, so this test cannot make a PID namespace");
        let children = format!("/proc/{0}/task/{0}/children", unshare.id());
        let deadline = Instant::now() + ENDS_WITHIN;
        loop {
            if let Some(init) = std::fs::read_to_string(&children)
                .ok()
                .and_then(|text| text.split_whitespace().next()?.parse().ok())
            {
                return Self { unshare, init };
            }
            if unshare.try_wait().unwrap().is_some() {
                let out = unshare.wait_with_output().unwrap();
                panic!(
                    "`unshare` forked nothing — are unprivileged user namespaces refused here? {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            assert!(Instant::now() < deadline, "`unshare` forked nothing in {ENDS_WITHIN:?}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Send the init `signal`, by name (`"TERM"`), from outside its namespace.
    pub fn signal(&self, signal: &str) {
        let sent = Command::new("kill")
            .args(["-s", signal, &self.init.to_string()])
            .status()
            .expect("`kill` runs");
        assert!(sent.success(), "kill -s {signal} {}", self.init);
    }

    /// Wait for the process to end, `SIGKILL`ing the init past
    /// [`ENDS_WITHIN`] and failing: an init still running is the defect.
    pub fn wait(mut self) -> (ExitStatus, String) {
        let deadline = Instant::now() + ENDS_WITHIN;
        while self.unshare.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                self.signal("KILL");
                let out = self.unshare.wait_with_output().unwrap();
                panic!(
                    "the init outlived its signal by {ENDS_WITHIN:?}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let out = self.unshare.wait_with_output().unwrap();
        (out.status, String::from_utf8_lossy(&out.stderr).into_owned())
    }
}
