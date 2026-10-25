#!/usr/bin/env python3
"""Run a reading in a memory-limited container, and refuse one that is not.

**Every run of a built binary a reading takes is made in the release image
under a memory limit** (`docs/design/measurements.md`, "The apparatus"): a
figure, a profile, a heaptrack recording, a traced save count, a hand probe.
This machine-protecting limit is not a figure's budget; a figure's container
states its own. `measure.py` runs its figures in containers of their own and
everything else it starts through `argv` here, and its printed recipes and any
hand-run reading run this script.

It runs the command in the release image (`release.ensure_image`), whose glibc
binaries built there are held to and which carries `perf`, `heaptrack`,
`strace` and libc's debug symbols, as the invoking user, `-m` and
`--memory-swap` both at `--memory`. **Every path the command names is mounted
at the same path**, the repository included, so a recipe line reads exactly as
it would on the host: an absolute argument, the path after a `=`, a relative one
that exists, each mounted at its own path and its real one. A binary built on
the host may need a newer glibc than the image's; build it in the image
(`release.py build`) instead.

`--hook` is the check: a Claude Code `PreToolUse` hook (`.claude/settings.json`)
handing it a Bash command, which it refuses when the command runs a built
binary, `cargo run`, `cargo bench` or a profiler outside a container
(`uncontained`). The printed recipes are held to the same classifier in
`test_measure.py`.

Usage:

    python3 scripts/contained.py [--memory 4g] [-e NAME=VALUE] [--mount DIR] -- <command> [args...]
    python3 scripts/contained.py --print -- <command>   # the container line, not run
    python3 scripts/contained.py --hook < event.json    # exit 2 refuses
    cd scripts && uv run python -m unittest test_contained
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path
from typing import Iterable, Sequence

REPO = Path(__file__).resolve().parent.parent

#: The limit a run is given unless it states one: above any reading's resident
#: set the register has taken, far below the machine.
DEFAULT_MEMORY = "4g"

#: The largest limit a run may state, as a fraction of the machine's memory: a
#: limit near the machine's is no limit.
MAX_FRACTION = 0.5

#: The container runtime, as `release.py` names it.
CONTAINER = os.environ.get("PGDT_RELEASE_CONTAINER", "sudo nerdctl")

#: Each path never mounted whole, however a command names it.
UNMOUNTABLE = frozenset(
    Path(p) for p in ("/", "/dev", "/etc", "/home", "/mnt", "/proc", "/sys", "/tmp", "/usr", "/var")
)

#: Trees nothing under is mounted: the container has its own. `/dev/shm` is the
#: exception, being where warm inputs are staged.
VIRTUAL = (Path("/proc"), Path("/sys"), Path("/dev"))
STAGED = Path("/dev/shm")

_SIZE_RE = re.compile(r"^(\d+)([kmgt]?)b?$", re.IGNORECASE)


def size_bytes(size: str) -> int:
    """A container limit as bytes: `512m`, `4g`, `1073741824`."""
    match = _SIZE_RE.match(size.strip())
    if match is None:
        raise ValueError(f"not a memory size: {size!r}")
    number, unit = match.groups()
    return int(number) * 1024 ** "_kmgt".index(unit.lower() or "_")


def machine_bytes(meminfo: Path = Path("/proc/meminfo")) -> int:
    """The machine's memory, off `MemTotal`."""
    for line in meminfo.read_text().splitlines():
        if line.startswith("MemTotal:"):
            return int(line.split()[1]) * 1024
    raise ValueError(f"no MemTotal in {meminfo}")


def check_memory(size: str, machine: int) -> None:
    """Refuse a limit past `MAX_FRACTION` of the machine's memory."""
    if size_bytes(size) > machine * MAX_FRACTION:
        raise ValueError(
            f"--memory {size} is over {MAX_FRACTION:.0%} of this machine's "
            f"{machine / 1024**3:.1f} GiB: a limit near the machine's is no limit"
        )


def _existing(path: Path) -> Path | None:
    """The directory a mount carries `path` in: `path` if it is one, the
    directory of a file or of a path not yet written, `None` where that is one
    never mounted whole."""
    if any(v == path or v in path.parents for v in VIRTUAL) and STAGED not in path.parents:
        return None
    for candidate in (path, *path.parents):
        if candidate.is_dir():
            return None if candidate in UNMOUNTABLE else candidate
        if candidate.exists():
            # A file is carried by its directory: a cache is saved by a rename
            # over it, which a file's own bind refuses.
            continue
    return None


def _path_words(words: Iterable[str]) -> Iterable[str]:
    """Each word, the part of it after a `=`, and the words of one holding a
    shell string, which may name paths of its own."""
    for word in words:
        yield word
        yield word.partition("=")[2]
        if any(c.isspace() for c in word):
            try:
                inner = shlex.split(word, comments=True)
            except ValueError:
                continue
            for part in inner:
                yield part
                yield part.partition("=")[2]


def mounts(
    words: Iterable[str], cwd: Path, extra: Iterable[Path] = ()
) -> list[tuple[Path, Path]]:
    """The `(source, destination)` binds that let `words` run as written: the
    repository and what its top-level links lead to, the working directory,
    the directory of each path a word names, and each of `extra`, which a
    program reads without its command naming it — the binary `perf report`
    resolves a recording's symbols from.

    Each is mounted at its real path, so a link inside another mount leads
    there, and at the path as named where that differs and no mount already
    holds it, so a link outside every mount still resolves."""
    named: list[Path] = [REPO, cwd, *(p for p in REPO.iterdir() if p.is_symlink()), *extra]
    for part in _path_words(words):
        if part.startswith("/"):
            named.append(Path(part))
        elif part and not part.startswith("-") and (cwd / part).exists():
            named.append(cwd / part)
    dirs = []
    for path in named:
        at = _existing(Path(os.path.abspath(path)))
        if at is not None and at not in dirs:
            dirs.append(at)
    binds: list[tuple[Path, Path]] = []
    for real in sorted({Path(os.path.realpath(at)) for at in dirs}, key=lambda d: len(d.parts)):
        if not any(dest in real.parents for _, dest in binds):
            binds.append((real, real))
    for at in dirs:
        real = Path(os.path.realpath(at))
        held = any(dest == at or dest in at.parents for _, dest in binds)
        if real != at and not held:
            binds.append((real, at))
    # A parent before anything mounted inside it.
    return sorted(binds, key=lambda b: (len(b[1].parts), str(b[1])))


def argv(
    command: Sequence[str],
    memory: str = DEFAULT_MEMORY,
    env: Sequence[str] = (),
    image: str | None = None,
    cwd: Path | None = None,
    container: Sequence[str] | None = None,
    mount: Sequence[Path] = (),
) -> list[str]:
    """The container run executing `command` under `memory`.

    `image` is the release image's tag, built first when `None`
    (`release.ensure_image`). **Seccomp is unconfined**, as the suite's run is
    (`release.run_in_image`), so `perf_event_open` reaches the kernel, whose
    `perf_event_paranoid` then decides as it would on the host."""
    cwd = Path.cwd() if cwd is None else cwd
    container = shlex.split(CONTAINER) if container is None else list(container)
    if image is None:
        import release

        image = release.ensure_image(container)
    out = [
        *container, "run", "--rm",
        "-m", memory, "--memory-swap", memory,
        "--security-opt", "seccomp=unconfined",
        "--user", f"{os.getuid()}:{os.getgid()}",
        "-e", "HOME=/tmp",
        "-w", str(cwd),
    ]  # fmt: skip
    for pair in env:
        out += ["-e", pair]
    for source, at in mounts([*command, *env], cwd, mount):
        out += ["-v", f"{source}:{at}"]
    return [*out, image, *command]


# --- the check ---------------------------------------------------------------

#: Programs that run their arguments in a container: a segment naming one is
#: contained whatever it runs.
CONTAINERS = frozenset({"nerdctl", "docker", "podman", "ctr"})

#: Words that run the rest of a command line as a program, each with the flags
#: that take a value after them.
WRAPPERS: dict[str, frozenset[str]] = {
    "sudo": frozenset({"-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U"}),
    "env": frozenset({"-u", "-C", "-S"}),
    "time": frozenset({"-f", "-o"}),
    "exec": frozenset({"-a"}),
    "nice": frozenset({"-n"}),
    "ionice": frozenset({"-c", "-n", "-p"}),
    "nohup": frozenset(),
    "setsid": frozenset(),
    "stdbuf": frozenset({"-i", "-o", "-e"}),
    "command": frozenset(),
    "builtin": frozenset(),
    "taskset": frozenset(),
    "timeout": frozenset({"-s", "-k"}),
}

#: Words naming the program that follows them positionally: `timeout`'s
#: duration, `taskset`'s mask.
_POSITIONAL = {"timeout": 1, "taskset": 1}

#: Shells whose `-c` string is itself checked.
SHELLS = frozenset({"sh", "bash", "zsh", "dash"})

#: Instruments that run what they are handed: refused outside a container.
#: `perf` only for the subcommands that run a program; `heaptrack_print` and
#: `perf report` read a file.
INSTRUMENTS = frozenset({"heaptrack", "valgrind", "strace", "ltrace", "hyperfine", "samply"})
PERF_RUNNING = frozenset({"record", "stat", "trace", "top", "mem", "sched", "c2c", "kmem", "lock"})

#: A program this project builds, by its file name.
BUILT_RE = re.compile(r"^(pgdt(-.+)?|gzip-inflate|peak-rss|count-instructions|gz_read|xz_decode)$")

#: Where a binary this project builds lives, as a path component.
BUILT_DIRS = ("target", "runs", "alloc-builds", "release-image")

#: Arguments that ask a binary about itself and run nothing.
SELF_REPORTS = frozenset({"--version", "-V", "--help", "-h"})

_HEREDOC_RE = re.compile(r"<<-?\s*(['\"]?)([A-Za-z_][A-Za-z0-9_]*)\1")


def _strip_heredocs(command: str) -> str:
    """`command` without its heredoc bodies, which are data, not commands."""
    lines = command.split("\n")
    out: list[str] = []
    ending: list[str] = []
    for line in lines:
        if ending:
            if line.strip() == ending[0]:
                ending.pop(0)
            continue
        out.append(line)
        ending += [m.group(2) for m in _HEREDOC_RE.finditer(line)]
    return "\n".join(out)


def segments(command: str) -> list[list[str]]:
    """`command`'s simple commands, as words, split at `;`, `&&`, `||`, `|`,
    `&`, newlines and parentheses outside quotes; comments and heredoc bodies
    dropped."""
    lexer = shlex.shlex(_strip_heredocs(command), posix=True, punctuation_chars="();<>|&\n")
    lexer.whitespace = " \t\r"
    lexer.whitespace_split = True
    out: list[list[str]] = [[]]
    redirect = False
    for word in lexer:
        if word and all(c in "();|&\n" for c in word):
            out.append([])
            redirect = False
        elif word and all(c in "<>" for c in word):
            redirect = True
        elif redirect:
            redirect = False
        elif word not in ("", "$"):
            out[-1].append(word)
    return [seg for seg in out if seg]


def _program(words: list[str]) -> list[str]:
    """`words` from the program a wrapper or an assignment leaves running."""
    i = 0
    while i < len(words):
        word = words[i]
        name = os.path.basename(word)
        if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", word):
            i += 1
        elif name in WRAPPERS:
            takes = WRAPPERS[name]
            i += 1
            while i < len(words) and words[i].startswith("-"):
                i += 2 if words[i] in takes else 1
            while i < len(words) and name == "env" and "=" in words[i]:
                i += 1
            i += _POSITIONAL.get(name, 0)
        else:
            return words[i:]
    return []


def _built(program: str) -> bool:
    name = os.path.basename(program)
    if BUILT_RE.match(name) and ("/" in program or name == "pgdt"):
        return True
    parts = Path(program).parts
    return "/" in program and any(d in parts[:-1] for d in BUILT_DIRS)


def uncontained(command: str) -> str | None:
    """The simple command in `command` that runs a reading outside a
    container, as text, or `None`.

    A reading is a built binary run (bar `--version` and `--help`), `cargo
    run` or `cargo bench`, or an instrument running a program; a segment
    whose program is a container runtime is contained. A command that does not
    parse is let through: the hook is a guard, not a shell."""
    try:
        found = segments(command)
    except ValueError:
        return None
    for seg in found:
        words = _program(seg)
        if not words:
            continue
        name = os.path.basename(words[0])
        args = words[1:]
        if name in CONTAINERS:
            continue
        if name in SHELLS and "-c" in args[:-1]:
            inner = uncontained(args[args.index("-c") + 1])
            if inner is not None:
                return inner
            continue
        refused = (
            (_built(words[0]) and not (args and set(args) <= SELF_REPORTS))
            or (name == "cargo" and next((a for a in args if not a.startswith("+")), "") in ("run", "bench", "r"))
            or name in INSTRUMENTS
            or (name == "perf" and next((a for a in args if not a.startswith("-")), "") in PERF_RUNNING)
        )
        if refused:
            return shlex.join(seg)
    return None


REFUSAL = (
    "Refused: `{segment}` runs a reading outside a memory-limited container. "
    "Every run of a built binary, `cargo run`, `cargo bench` or a profiler is made "
    "in the release image under a limit (docs/design/measurements.md, \"The apparatus\"): "
    "`python3 scripts/contained.py [--memory 4g] -- <command>`, binaries built in the "
    "image by `cd scripts && uv run release.py build`."
)


def hook(stdin: str) -> int:
    """A `PreToolUse` event: 2, the refusal on stderr, for an uncontained
    Bash command; 0 otherwise."""
    try:
        event = json.loads(stdin)
    except json.JSONDecodeError:
        return 0
    if event.get("tool_name") != "Bash":
        return 0
    command = (event.get("tool_input") or {}).get("command") or ""
    segment = uncontained(command)
    if segment is None:
        return 0
    print(REFUSAL.format(segment=segment), file=sys.stderr)
    return 2


def main(args: Sequence[str] | None = None) -> int:
    p = argparse.ArgumentParser(
        prog="contained.py",
        description=__doc__.split("\n\n")[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    p.add_argument("--memory", default=DEFAULT_MEMORY, help=f"the container's limit (default: {DEFAULT_MEMORY})")
    p.add_argument("-e", "--env", action="append", default=[], help="NAME=VALUE in the container (repeatable)")
    p.add_argument(
        "--mount", action="append", default=[], type=Path,
        help="a directory to mount at its own path that the command does not name (repeatable)",
    )  # fmt: skip
    p.add_argument("--print", action="store_true", help="print the container line rather than run it")
    p.add_argument("--hook", action="store_true", help="read a PreToolUse event on stdin; exit 2 refuses")
    p.add_argument("command", nargs=argparse.REMAINDER, help="the command, after --")
    parsed = p.parse_args(args)
    if parsed.hook:
        return hook(sys.stdin.read())
    command = parsed.command[1:] if parsed.command[:1] == ["--"] else parsed.command
    if not command:
        p.error("a command, after --")
    try:
        check_memory(parsed.memory, machine_bytes())
    except ValueError as e:
        print(f"contained.py: {e}", file=sys.stderr)
        return 2
    line = argv(command, parsed.memory, parsed.env, mount=parsed.mount)
    if parsed.print:
        print(shlex.join(line))
        return 0
    return subprocess.run(line).returncode


if __name__ == "__main__":
    sys.exit(main())
