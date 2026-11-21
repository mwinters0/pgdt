"""The koji invocation: an hour on another medium, so printed."""

from __future__ import annotations

import release

from measurement import config
from measurement import shapes


KOJI_DUMP = config._env("PGDT_KOJI_DUMP", "/mnt/wd12t/fedora/koji/koji-2026-07-23.dump")


def koji_recipe(cfg: config.Config, name: str, wrap: bool, jobs: int = shapes.SWEEP_JOBS) -> str:
    """The koji invocation, printed rather than run.

    koji is deliberately outside the sweep — a different medium, ~54 minutes,
    and a byte-for-byte regression check rather than a throughput figure — but
    the *recipe* was living in three hand-maintained copies, which is how a
    documented command was found that no longer ran. This is the one copy.

    Three things here have each cost a run, and a test asserts all three:

    * `exec`, so `pgdt` is PID 1 and `nerdctl stop` reaches the interrupt guard.
      A compound command cannot be `exec`'d, which is why nothing is appended to
      report the exit status — `nerdctl inspect` reports it either way, for a
      run that finished *or* was signalled.
    * the cgroup limit, which is part of the apparatus.
    * `--cache` under the mounted `/out`. The dump is read-only, so the
      colocated default lands in the container's ephemeral layer and is
      destroyed with it — an hour of scanning thrown away with no error, since
      the write itself succeeds.

    **The worker count is a parameter here, where every other invocation this
    module builds pins `SWEEP_JOBS`.** koji's leg is not a table in the
    register: what it checks is that a scan of the real sample comes back
    byte-identical, and the parallel scan's verification is a leg at some
    count against a serial one. So the count is stated on the command line the
    way it is everywhere else — nothing inherits the CLI default — and *which*
    count is the caller's to say. The wrap sequence states one count for both
    of its legs, since resuming a scan under a different arrangement is a
    second variable in a check that has one.
    """
    mounts = (
        f'  -v "{cfg.bin_pgdt}:/pgdt:ro" \\\n'
        f'  -v "{cfg.out_dir}:/out" \\\n'
        f'  -v "{KOJI_DUMP}:/dump.sql:ro" \\\n'
    )
    def leg(container: str, cache: str, log: str) -> str:
        return (
            f"sudo nerdctl run -d --name {container} "
            f"-m {cfg.memory} --memory-swap {cfg.memory} \\\n"
            + mounts
            + f"  {cfg.image} \\\n"
            f"  sh -c 'exec /pgdt parse --source /dump.sql --cache /out/{cache} "
            f"--jobs {jobs} {shapes.NO_STATISTICS} >> /out/{log} 2>&1'"
        )

    out = [
        f"(cd scripts && uv run release.py build --target {release.NATIVE_TARGET})"
        "   # the register's build, in the release image",
        "mkdir -p runs",
        "",
    ]
    if not wrap:
        out += [
            leg(name, f"{name}.pgdt", f"{name}-scan.log"),
            "",
            f"# still going?   sudo nerdctl inspect -f '{{{{.State.Status}}}}' {name}",
            f"# peak RSS:      sudo grep VmHWM /proc/$(sudo nerdctl inspect -f "
            "'{{.State.Pid}}' " + name + ")/status",
            "#   Read it while the run is still going: the kernel keeps the high-water mark,",
            "#   so one read covers everything up to it, and it is gone the moment the",
            "#   process exits. `exec` above makes pgdt PID 1, so that is the pid to read.",
            "#   The container cgroup's memory.peak is the wrong instrument here — it is",
            "#   charged the page cache of a 784 GB read and reports the limit, not pgdt.",
            f"# exit status:   sudo nerdctl inspect -f '{{{{.State.ExitCode}}}}' {name}",
            "#   143 = SIGTERM, which is what `nerdctl stop` sends: the image sets no",
            "#   STOPSIGNAL, and --stop-signal on `run` is accepted and then ignored.",
            "#   For the SIGINT arm: sudo nerdctl kill -s SIGINT " + name + "  (exit 130)",
            f"# wall clock:    sudo nerdctl inspect -f "
            "'{{.State.StartedAt}} {{.State.FinishedAt}}' " + name,
            f"# the log:       runs/{name}-scan.log",
        ]
    else:
        out += [
            "# leg 1 — cold, interrupted partway.",
            leg(f"{name}-wrap1", f"{name}-wrap.pgdt", f"{name}-wrap-scan.log"),
            f"sleep 1200 && sudo nerdctl stop -t 120 {name}-wrap1",
            f"sudo nerdctl inspect -f '{{{{.State.ExitCode}}}}' {name}-wrap1   # 143 (SIGTERM)",
            "",
            "# the interrupted cache must come back typed — both counts zero",
            f"sudo nerdctl run --rm -m {cfg.memory} --memory-swap {cfg.memory} \\",
            # keep the trailing line-continuation: the mounts run straight on
            # into the image name below.
            mounts.rstrip("\n"),
            f"  {cfg.image} /pgdt info --cache /out/{name}-wrap.pgdt --detail \\",
            "  | grep -c 'not declared\\|metadata not scanned'",
            "",
            "# leg 2 — resume the identical command, then compare to a full run's cache",
            f"sudo nerdctl rm -f {name}-wrap1",
            leg(f"{name}-wrap2", f"{name}-wrap.pgdt", f"{name}-wrap-scan.log"),
            f"cmp runs/{name}-wrap.pgdt runs/<a previous full run>.pgdt",
        ]
    return "\n".join(out)


def cmd_koji(wrap: bool, jobs: int) -> int:
    cfg = config.Config()
    print(
        "# koji is not part of the sweep: a different medium, ~54 minutes, and a\n"
        "# byte-for-byte regression check rather than a throughput figure. Run this\n"
        "# detached and read it in a later session (CLAUDE.md, \"Long-running processes\").\n"
        f"# The scan below runs at --jobs {jobs}; --koji-jobs states another.\n"
    )
    print(koji_recipe(cfg, "pgdt-koji", wrap, jobs))
    return 0
