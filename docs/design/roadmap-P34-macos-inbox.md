# P34 inbox — facts filed for its grilling

Evidence P34 (macOS) will need. **This is a queue, not a document**: when P34
is grilled, walk every entry, fold it into the spec or discard it as stale, and
delete this file. See `docs/process.md`, "Inboxes: facts filed by destination".

---

## Memory discovery reads Linux files only, and finds nothing elsewhere

**Fact.** A flagless run sizes itself from `/proc/meminfo`'s `MemAvailable`
and the cgroup limit (`pgdump_query/src/io.rs`, `available_memory_in`,
`discover_memory_limit_in`, `Parallelism::discover_holding_in`;
`datafusion-pgdump/src/budget.rs`, `ScanBudget::discover_in`). On macOS each
read fails into `None`: the CLI falls to `DEFAULT_MEMORY_BUDGET`, a source
recommending worker memory (`.xz`) keeps its whole recommendation under an
uncapped bound, and the provider reports `AllowanceOrigin::NoneFound`.
Elsewhere the code compiles there: the I/O layer is `std::os::unix`,
`namespace-init`'s ending signals are `target_os = "linux"` with an empty
fallback, and `pgdt/src/introspect.rs`'s glibc arms have a non-gnu stub. There
is no `target_os = "macos"` arm anywhere. Whether the workspace compiles for an
Apple target has never been tried.

**Why P34 cares.** [`roadmap.md`](roadmap.md), "A default runs as fast as the
allocation permits" makes the discovered path the normal one, so a macOS
binary needs a discovery path and the `RT<n>` entries it rests on before it
ships — which is why P29 ships Linux only. A compile against both Apple targets
is the cheapest first slice.

**Origin.** Filed 2026-10-01 by the session sketching P29; moved here
2026-10-06 when P29's grilling took macOS out of its scope.

---

## The C dependencies must build for Apple targets

**Fact.** `pgdt` statically links aws-lc (`aws-lc-sys`, whose 0.45 carries `cc`
builders for both Apple targets; on Linux that builder runs and CMake never
does, unbuilt for Apple),
ring's BoringSSL-derived C and assembly, liblzma (`liblzma-sys`, `static`),
zstd, mimalloc, BLAKE3's C and assembly and `psm`. On the Apple targets the
linked crate set adds `core-foundation`, `security-framework` and `errno`
(`MIT OR Apache-2.0`) and drops `linux-raw-sys` and `openssl-probe`; the licence
picture is otherwise identical to Linux's.

**Why P34 cares.** Building from a Linux host means cross-compiling every one of
those C builds against an Apple SDK; building on a macOS runner means a CI leg
the local release rehearsal cannot reproduce. An arm64 Mach-O also needs at
least an ad-hoc signature to run.

**Origin.** Filed 2026-10-06 by P29's grilling, from a `cargo metadata` /
`cargo tree` walk of `pgdt`'s default-feature closure per target.
