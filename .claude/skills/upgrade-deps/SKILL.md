---
name: upgrade-deps
description: Upgrade a dependency — DataFusion, arrow, chrono, any crate pin, the Rust release `rust-toolchain.toml` pins, a tool `mise.toml` pins, the release image's digest — checking `docs/status/upstream.md` for what the new version fixes and acting on it, and walking the invariants it moves. Use when bumping a version in a Cargo.toml or `mise.toml`, or running `cargo update`.
---

**Invoke the `process` skill first**: an upgrade is out-of-band work under its
admission rule unless it changes a decision, and what it strikes is struck by
the rules there. `vendor/xz-seek` is not upgraded here — `CLAUDE.local.md`'s
`xz-seek` section and `scripts/vendor_xz_seek.py` govern it. **Invoke the
`upstream-issue` skill too**: it holds the register's rules, which step 5
applies.

## Upgrading

1. **Scope the move.** Name each crate and its versions, from and to. `arrow`
   moves with DataFusion in one change (`docs/design/roadmap.md`, "Arrow
   follows DataFusion"). A DataFusion major re-applies the shell's marked lines
   to upstream's new `datafusion-cli` `main.rs`, per that file's header.
   **The compiler and the release image move here too**, each an apparatus
   change: `rust-toolchain.toml`'s `channel`, and `release/Dockerfile`'s
   `FROM` digest, which pins every package the image installs
   (`docs/design/roadmap-P29-releases.md`, "When the pin moves"). **So do
   `mise.toml`'s tools**, each pinned to an exact version that the image
   installs too: a bump there moves the image's tag, and `upstream.md` and the
   invariants are walked for a tool as for a crate.
2. **Read `docs/status/upstream.md` whole**, and test each entry naming a
   moved crate against its **Fixed when**: read the new release's source — a
   local upstream checkout where `CLAUDE.local.md` names one, else the crate
   under `~/.cargo/registry/src` — or check the commit with `git merge-base
   --is-ancestor <commit> <tag>` and the changelog. Note each verdict.
3. **Walk the invariants** whose **Verified against** names a moved crate
   (`rg -n 'Verified against.*<crate>' docs/design/*invariants.md`), running
   each one's **Re-verify** at the new version.
4. **Bump, build and `mise run check`.** A test pinning upstream behaviour —
   every **Watch** test, and each one the shell's header names — fails on
   purpose when upstream moves. Read each failure against the register and the
   invariants before touching it, and never edit one to pass without acting on
   what it caught. **A `Cargo.lock` change also runs `cd scripts && uv run
   release.py notices`**: a licence the allow-list lacks, or a file
   `release/about.toml` clarifies having moved, fails here rather than at a
   release; re-read the crate's files and amend that config in this change.
5. **Act on each verdict.** A fix that shipped: carry out **When it lands**,
   strike the entry, and confirm `upstream.py` is green. A fix that did not:
   rewrite the entry's **Upstream** field to that day's state. A new defect
   the upgrade brought: add an entry.
6. **Figures.** `measure.py --stale` watches the paths each figure declares,
   which a lockfile change can move without turning a figure red. Before
   claiming a figure holds across the upgrade, read `docs/design/measurements.md`,
   "The apparatus", and invoke the `evidence` skill.
7. **Record it.** An out-of-band row unless the upgrade changes a decision a
   spec records; a history entry naming each entry the upgrade resolved or
   rewrote and each invariant re-verified; one commit.
