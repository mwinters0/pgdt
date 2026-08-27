//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/architecture.md`, "The cache").
//!
//! Reading is best-effort — the cache is never required for correctness, so
//! a missing, foreign, unrecognised-version, or size-mismatched file just
//! means "scan instead," never a hard error. Which of the four it was is
//! still reported — see [`CacheStatus`] — because `pgdq info` has no "scan
//! instead" to fall back on and has to say what went wrong.
//! Writing is not best-effort: [`save`] propagates I/O failures rather than
//! silently falling back to running without a cache, since a write failure
//! (read-only mount, permissions, disk full) means something is actually
//! wrong and swallowing it would silently degrade every future run of the
//! same command back to a full scan.
//!
//! **The dump file's identity is checked, not assumed**
//! (`docs/design/architecture.md`, "The cache"). Every cache records the
//! source's size and mtime as observed at save time; [`load`] re-observes
//! the live source and compares. A size mismatch means every byte offset in
//! the cache could be wrong, so the cache is invalidated the same way a
//! foreign or wrong-version file is — silently, per this module's
//! best-effort contract, not as a hard error. An mtime mismatch is weaker
//! evidence (mtime granularity and preservation vary too much across
//! filesystems to be conclusive) and does **not** invalidate the cache; it
//! is surfaced on [`CacheStatus::Valid`], and [`CacheMode::load`] turns it
//! into a [`crate::diagnostic::DiagnosticKind::CacheMtimeChanged`] on the
//! loaded index rather than acting on it. That diagnostic is recomputed on
//! every load and never persisted — the mismatch is between the cache and
//! *this* run's observation, so a stored one would be a warning about a
//! check that has since passed.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use crate::index::DumpIndex;
use crate::io::ByteRangeSource;
use crate::{Error, Result};

/// Bumped whenever the on-disk shape changes incompatibly. A cache written
/// under a different version is unusable ([`CacheStatus::UnsupportedVersion`])
/// rather than partially trusted (`docs/design/roadmap.md`,
/// "Four decisions that keep later phases additive") —
/// the three fields reserved on [`DumpIndex`]/[`crate::index::CopyBlock`] are
/// what let most future additions avoid needing a bump at all.
///
/// Pre-1.0, a bump is free and nothing migrates (`CLAUDE.md`, "Pre-1.0"), so
/// the rule is simply: **bump whenever a persisted field is added, removed or
/// reshaped.** `git log -p` on this constant is the version history.
///
/// What does *not* need a bump: a new way of *reading* an existing on-disk
/// shape. [`CacheStatus::Incomplete`] is the worked example — it reinterprets
/// `scanned_through` against a size already stored, changing nothing on
/// disk.
const FORMAT_VERSION: u32 = 12;

/// The dump file's size and modification time as observed when a cache was
/// last saved — see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SourceIdentity {
    size: u64,
    /// `(seconds, nanoseconds)` since the Unix epoch — `SystemTime` itself
    /// isn't `Serialize`, and this is L1's own on-disk vocabulary rather
    /// than borrowing `std`'s. `None` when the source exposed no mtime.
    mtime: Option<(u64, u32)>,
}

impl SourceIdentity {
    async fn observe<S: ByteRangeSource>(source: &S) -> Result<Self> {
        let size = source.size().await?;
        let mtime = source
            .modified()
            .await?
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default())
            .map(|d| (d.as_secs(), d.subsec_nanos()));
        Ok(Self { size, mtime })
    }
}

/// What produced the indexed blocks' byte offsets. Plain-format offsets are
/// raw file positions; a future archive format's (`docs/design/roadmap.md`,
/// P8 Track B)
/// are entry-relative, so the two must never be silently conflated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ContainerKind {
    Plain,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    format_version: u32,
    container_kind: ContainerKind,
    identity: SourceIdentity,
    index: DumpIndex,
}

/// The default cache location when no explicit path is given:
/// `<dump-path>.dqcache`.
pub fn colocated_path(dump_path: &Path) -> PathBuf {
    let mut path = dump_path.as_os_str().to_owned();
    path.push(".dqcache");
    PathBuf::from(path)
}

/// What [`load`] found at a cache path, once checked against the live
/// source's identity — see the module docs.
///
/// **The four unusable outcomes are named separately, not collapsed.** Every
/// caller that can only respond by scanning treats them alike and says so
/// ([`CacheMode::load`] folds all four into `None`), but a caller that cannot
/// scan — `pgdq info`, which reports from the cache and never reads the dump —
/// has a different sentence to say for each, and "you have never parsed this
/// file" is not the same fact as "your file changed since you parsed it" even
/// though both end in `pgdq parse`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// Nothing at this path.
    Missing,
    /// Something is at this path, but it does not decode as a cache at all —
    /// foreign bytes, or a truncated write.
    Unreadable,
    /// A cache written by a build whose on-disk shape this one does not
    /// recognise (`format_version`/`container_kind`). Pre-1.0 these are free
    /// and frequent, and nothing migrates
    /// (`docs/design/roadmap.md`, "Pre-1.0").
    UnsupportedVersion,
    /// A readable cache whose recorded source size disagrees with the live
    /// source's, so every byte offset in it could be wrong. Both sizes are
    /// carried because "the file changed" is the fact a reporting caller
    /// states, and the two numbers are the evidence for it.
    SourceChanged { cached_size: u64, live_size: u64 },
    /// A usable cache whose `index.scanned_through` reaches the file's
    /// recorded size — the whole file is mapped. `mtime_changed` is `true`
    /// when the source's current mtime differs from the one recorded at save
    /// time — weaker evidence than a size mismatch (see the module docs), so
    /// it does not itself make the cache unusable. `total_size` is the
    /// recorded [`SourceIdentity::size`], carried for the same reason
    /// [`Incomplete`](CacheStatus::Incomplete) carries it: a reporting caller
    /// states coverage against it, and a cache-only caller has no live source
    /// to stat.
    Valid { index: DumpIndex, mtime_changed: bool, total_size: u64 },
    /// A usable cache whose `index.scanned_through` falls short of
    /// `total_size` — a real, not-yet-finished scan (e.g. a preamble-only
    /// scan, or a query that stopped once its target settled), not a defect
    /// (`docs/design/architecture.md`, "The cache", the former out-of-band item M1). What "not enough"
    /// means is caller-specific: `pgdq info` reports whatever this holds and
    /// states the coverage (`docs/design/architecture.md`, "CLI surface"),
    /// while a caller that resumes an incremental scan from wherever it left
    /// off (`crate::stream::map_file`, `crate::stream::table_stream`,
    /// `crate::index::preamble_only`) wants exactly this partial index to
    /// build on, the same as [`Valid`](CacheStatus::Valid) —
    /// [`CacheMode::load`] treats it that way. `total_size` is the cache's
    /// own recorded [`SourceIdentity::size`], which — once this case or
    /// `Valid` is reached — is already known to equal the live source's size
    /// when one is available, so it serves a cache-only caller (no live
    /// source to stat) the same way it serves a live one.
    Incomplete { index: DumpIndex, mtime_changed: bool, total_size: u64 },
}

/// Load a cache from `path` and validate it against `source`'s current
/// identity. See [`CacheStatus`] and the module docs for what "validate"
/// means. A hard I/O error reading `path` (anything but "not found") still
/// propagates — only the cache's own *content* is best-effort.
pub async fn load<S: ByteRangeSource>(path: &Path, source: &S) -> Result<CacheStatus> {
    let file = match read_cache_file(path)? {
        Ok(file) => file,
        Err(status) => return Ok(status),
    };
    let live = SourceIdentity::observe(source).await?;
    if file.identity.size != live.size {
        return Ok(CacheStatus::SourceChanged {
            cached_size: file.identity.size,
            live_size: live.size,
        });
    }
    let mtime_changed = file.identity.mtime != live.mtime;
    Ok(status_from_file(file, mtime_changed))
}

/// Read and envelope-check the cache at `path`, shared by [`load`] and
/// [`load_offline`]. `Err(status)` is one of the three unusable outcomes that
/// need no live source to reach; only a size mismatch does, and that is
/// [`load`]'s alone.
fn read_cache_file(path: &Path) -> Result<std::result::Result<CacheFile, CacheStatus>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Err(CacheStatus::Missing));
        }
        Err(e) => return Err(Error::Io(e)),
    };
    let file: CacheFile =
        match bincode::serde::decode_from_slice(&bytes, bincode::config::standard()) {
            Ok((file, _)) => file,
            Err(_) => return Ok(Err(CacheStatus::Unreadable)),
        };
    if file.format_version != FORMAT_VERSION || file.container_kind != ContainerKind::Plain {
        return Ok(Err(CacheStatus::UnsupportedVersion));
    }
    Ok(Ok(file))
}

/// Load a cache from `path` with no live source to check it against — the
/// cache-only counterpart to [`load`]
/// (`docs/design/architecture.md`, "The cache"). There is nothing to compare the recorded mtime to, so
/// `mtime_changed` is always `false` here; the "this is unverified,
/// historical data" fact cache-only mode carries instead is a
/// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed by
/// [`CacheMode::load_offline`], not this flag.
pub async fn load_offline(path: &Path) -> Result<CacheStatus> {
    match read_cache_file(path)? {
        Ok(file) => Ok(status_from_file(file, false)),
        Err(status) => Ok(status),
    }
}

/// Shared by [`load`] and [`load_offline`] once a `CacheFile` has passed its
/// format/container/(when live) size checks: `Valid` when the index's own
/// `scanned_through` reaches the file's recorded size, `Incomplete`
/// otherwise, with the index's unpersisted diagnostics recomputed either
/// way. Reads `file.identity.size` rather than re-stating a live size
/// — by the time either caller reaches this point the two are already known
/// equal wherever a live one exists (a live-size mismatch returns
/// [`CacheStatus::SourceChanged`] earlier in [`load`]), and `load_offline` has no live size to read at all.
fn status_from_file(file: CacheFile, mtime_changed: bool) -> CacheStatus {
    let total_size = file.identity.size;
    let mut index = file.index;
    // `DumpIndex::diagnostics` is `#[serde(skip)]`, so a loaded index arrives
    // with none. Both file-level figures are pure functions of the spans and
    // O(spans) to compute, which is what lets them be recomputed on load
    // rather than persisted (`crate::diagnostic`, "Not persisted") — and a
    // caller that reports from a cache without ever scanning (`pgdq info`)
    // would otherwise silently lose the TOC-coverage figure.
    index.diagnostics = crate::index::tiling_diagnostics(&index.spans, total_size);
    index.diagnostics.push(crate::index::toc_coverage_diagnostic(&index.spans));
    if index.is_complete(total_size) {
        CacheStatus::Valid { index, mtime_changed, total_size }
    } else {
        CacheStatus::Incomplete { index, mtime_changed, total_size }
    }
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), overwriting any existing cache there, and record `source`'s
/// current size/mtime for [`load`] to check next time. Propagates I/O
/// failures as `Error::Io` rather than swallowing them — see the module
/// docs.
pub async fn save<S: ByteRangeSource>(path: &Path, source: &S, index: &DumpIndex) -> Result<()> {
    let identity = SourceIdentity::observe(source).await?;
    let file = CacheFile {
        format_version: FORMAT_VERSION,
        container_kind: ContainerKind::Plain,
        identity,
        index: index.clone(),
    };
    let bytes = bincode::serde::encode_to_vec(&file, bincode::config::standard())?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// How a caller wants the structure cache handled for one operation.
/// Constructed via [`CacheMode::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheMode {
    /// Read/write the cache at this path (the colocated default, or an
    /// explicit path).
    Enabled(PathBuf),
    /// The caller explicitly opted out: any existing file at what would
    /// otherwise be the resolved location is ignored, and a fresh scan is
    /// not persisted.
    Disabled,
    /// No live dump source at all — answer strictly from the cache at this
    /// path (`docs/design/architecture.md`, "The cache"). Never constructed by [`CacheMode::resolve`]; a caller
    /// builds it directly (`pgdq info` with no `--source`). Rejected by
    /// every method below that takes a live `source` — being handed
    /// `Offline` while a live source is in hand is a caller contract
    /// violation, not a degenerate case to tolerate — and [`load_offline`]
    /// is its own counterpart, rejecting `Enabled`/`Disabled` the other way.
    Offline(PathBuf),
}

impl CacheMode {
    /// Resolve a `--cache-path`-style argument against `dump_path`: `None`
    /// selects the colocated default (`<dump_path>.dqcache`), the literal
    /// path `none` disables the cache, and any other path is used as-is.
    pub fn resolve(dump_path: &Path, cache_path: Option<&Path>) -> CacheMode {
        match cache_path {
            None => CacheMode::Enabled(colocated_path(dump_path)),
            Some(p) if p == Path::new("none") => CacheMode::Disabled,
            Some(p) => CacheMode::Enabled(p.to_path_buf()),
        }
    }

    /// Load the cache this mode points at, discarding the
    /// [`CacheStatus::Valid::mtime_changed`] bit (see the module docs — a
    /// caller that wants it can call [`load`] directly instead). A disabled
    /// cache always yields `Ok(None)` — even if a file happens to sit at
    /// what would otherwise be its resolved location, per
    /// [`CacheMode::Disabled`]'s contract that existing files are ignored,
    /// never read.
    ///
    /// `Incomplete` is treated exactly like `Valid` — returned as `Some`,
    /// not folded into `None` — because every caller of this method
    /// (`crate::stream::map_file`, `crate::stream::table_stream`,
    /// `crate::index::preamble_only`) wants whatever partial map already
    /// exists to build forward from. Folding `Incomplete` into `None` here
    /// would make every partial cache invisible to the next scan, which is
    /// exactly the incremental caching `docs/design/architecture.md`,
    /// "Query: mapping and streaming are separate passes" depends on. A
    /// caller that instead *reports* what a cache holds reaches for
    /// [`load`]/[`CacheMode::load_offline`] and the full [`CacheStatus`],
    /// which is what `pgdq info` does.
    pub async fn load<S: ByteRangeSource>(&self, source: &S) -> Result<Option<DumpIndex>> {
        match self {
            CacheMode::Enabled(path) => match load(path, source).await? {
                // All four unusable outcomes collapse here: this method's
                // callers can only respond by scanning, so telling them apart
                // would be information with no consumer. `pgdq info`, which
                // has no such response, matches on [`CacheStatus`] directly.
                CacheStatus::Missing
                | CacheStatus::Unreadable
                | CacheStatus::UnsupportedVersion
                | CacheStatus::SourceChanged { .. } => Ok(None),
                CacheStatus::Valid { mut index, mtime_changed, .. }
                | CacheStatus::Incomplete { mut index, mtime_changed, .. } => {
                    // Reported rather than acted on: too weak to invalidate
                    // (see the module docs), and pointless to persist — the
                    // mismatch is between the cache and *this* run's
                    // observation, so it is recomputed on every load.
                    if mtime_changed {
                        index
                            .diagnostics
                            .push(crate::diagnostic::Diagnostic::cache_mtime_changed());
                    }
                    Ok(Some(index))
                }
            },
            CacheMode::Disabled => Ok(None),
            CacheMode::Offline(_) => Err(Error::CacheModeMismatch(
                "a live dump source requires CacheMode::Enabled or CacheMode::Disabled, not Offline",
            )),
        }
    }

    /// Load this mode's cache with no live source to check it against — the
    /// cache-only counterpart to [`CacheMode::load`]
    /// (`docs/design/architecture.md`, "The cache"). Unlike `load`, this returns the full [`CacheStatus`]
    /// rather than collapsing it to `Option<DumpIndex>`: cache-only mode has
    /// no scan to fall back on, so a caller needs to tell `Valid` apart from
    /// `Incomplete` to decide whether it has enough to answer from. Every
    /// successful load (`Valid` or `Incomplete`) gets a
    /// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed onto it
    /// unconditionally — there is no live file to compare against, so the
    /// result is unverified and historical regardless of completeness.
    pub async fn load_offline(&self) -> Result<CacheStatus> {
        match self {
            CacheMode::Offline(path) => {
                let mut status = load_offline(path).await?;
                match &mut status {
                    CacheStatus::Missing
                    | CacheStatus::Unreadable
                    | CacheStatus::UnsupportedVersion
                    | CacheStatus::SourceChanged { .. } => {}
                    CacheStatus::Valid { index, .. } | CacheStatus::Incomplete { index, .. } => {
                        index.diagnostics.push(crate::diagnostic::Diagnostic::cache_offline());
                    }
                }
                Ok(status)
            }
            CacheMode::Enabled(_) | CacheMode::Disabled => {
                Err(Error::CacheModeMismatch("cache-only access requires CacheMode::Offline"))
            }
        }
    }

    /// Persist `index` per this mode. A disabled cache is a no-op — nothing
    /// is written, by design, not as a fallback. Callers whose operation's
    /// whole purpose is to populate the cache should reject a disabled mode
    /// up front with [`CacheMode::require_enabled`] instead of relying on
    /// this silently doing nothing.
    pub async fn save<S: ByteRangeSource>(&self, source: &S, index: &DumpIndex) -> Result<()> {
        match self {
            CacheMode::Enabled(path) => save(path, source, index).await,
            CacheMode::Disabled => Ok(()),
            CacheMode::Offline(_) => {
                Err(Error::CacheModeMismatch("cache-only mode never has a fresh scan to persist"))
            }
        }
    }

    /// The path this mode resolves to, or `Error::CacheDisabled` if the
    /// caller disabled the cache but `operation` requires one. Use this to
    /// reject an incompatible `--dqcache none` up front, rather than
    /// discovering it only when a write silently no-ops.
    pub fn require_enabled(&self, operation: &'static str) -> Result<&Path> {
        match self {
            CacheMode::Enabled(path) => Ok(path),
            CacheMode::Disabled => Err(Error::CacheDisabled { operation }),
            CacheMode::Offline(_) => Err(Error::CacheModeMismatch(
                "cache-only mode has no live source to scan and persist",
            )),
        }
    }
}
