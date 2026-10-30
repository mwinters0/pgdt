//! How a caller wants the cache handled for one operation ([`CacheMode`]),
//! the identity signals it binds among them, and the judgement and refusal
//! every open makes under it; and a cache read with no source at all
//! ([`OfflineCache`]).

use std::path::{Path, PathBuf};

use super::identity::{SourceWatch, StrictIdentity, advisory_identity_diagnostics};
use super::store::{load_offline, save};
use super::{CacheStatus, LoadedCache, colocated_path};
use pgdt_core::{Error, Result, Unusable};
use pgdt_io::{ByteRangeSource, Origin};
use pgdt_model::internal::DiagnosticExt;
use pgdt_model::{Diagnostic, DumpIndex};

/// How a caller wants the structure cache handled for one operation, and
/// which identity signals bind while it does. [`CacheMode::resolve`],
/// [`CacheMode::enabled`] and [`CacheMode::DISABLED`] each leave
/// [`StrictIdentity::ADVISORY`], an enabled one refusing an unusable cache;
/// [`CacheMode::with_strict_identity`] and
/// [`CacheMode::with_overwrite_unusable`] state anything else, as a variant
/// built directly does.
/// **A caller states a disabled cache itself**: no path means one, so a
/// front end's spelling of "no cache" is its own to parse.
///
/// **Every mode has a live source to check against.** Reading a cache with no
/// source at all is [`OfflineCache`], a type no live entry point takes, so
/// the one cannot be handed where the other is wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheMode {
    /// Read/write the cache at this path (the colocated default, or an
    /// explicit path). `overwrite_unusable` is whether a scan starts cold
    /// over a cache it cannot use and replaces it rather than refusing
    /// ([`CacheMode::refusal`]).
    Enabled { path: PathBuf, strict: StrictIdentity, overwrite_unusable: bool },
    /// The caller explicitly opted out: any existing file at what would
    /// otherwise be the resolved location is ignored, and a fresh scan is
    /// not persisted. **It still carries a strictness**, because a run that
    /// writes no cache is still a run that can have the file rewritten
    /// underneath it ([`SourceWatch`]).
    Disabled { strict: StrictIdentity },
}

impl CacheMode {
    /// A disabled cache at the default strictness.
    pub const DISABLED: CacheMode = CacheMode::Disabled { strict: StrictIdentity::ADVISORY };

    /// The cache at `path`, at the default strictness.
    pub fn enabled(path: impl Into<PathBuf>) -> CacheMode {
        CacheMode::Enabled {
            path: path.into(),
            strict: StrictIdentity::ADVISORY,
            overwrite_unusable: false,
        }
    }

    /// The cache at `cache_path`, or where none is given the default for the
    /// source it is for.
    ///
    /// **The default is the name of the dump plus `.pgdt`, and only where
    /// it sits differs.** A local dump's cache sits *beside* it, so the
    /// pairing is the filesystem's. An object fetched over a network has no
    /// beside, so its cache is named after the URL's last path segment and
    /// sits in the working directory — predictable by reading the URL, where
    /// a hashed name would not be
    /// (`docs/design/decisions.md`, "D87").
    ///
    /// *Consequence, stated rather than defended away:* two same-named dumps
    /// of equal stored size from different hosts, read in one working
    /// directory, read each other's map. The default path is what makes that
    /// reachable; the origin the cache records is what makes it visible
    /// ([`OriginMatch`]).
    pub fn resolve(origin: &Origin, cache_path: Option<&Path>) -> CacheMode {
        match cache_path {
            Some(p) => CacheMode::enabled(p),
            // A remote origin always names an object: a URL that names none is
            // refused when the origin is built, so there is no third case.
            None => CacheMode::enabled(colocated_path(
                origin.local_path().unwrap_or(Path::new(origin.remote_name().unwrap_or_default())),
            )),
        }
    }

    /// State which identity signals bind.
    pub fn with_strict_identity(self, strict: StrictIdentity) -> CacheMode {
        match self {
            CacheMode::Enabled { path, overwrite_unusable, .. } => {
                CacheMode::Enabled { path, strict, overwrite_unusable }
            }
            CacheMode::Disabled { .. } => CacheMode::Disabled { strict },
        }
    }

    /// State whether a scan may start cold over a cache it cannot use and
    /// replace it — `pgdt --overwrite-unusable-cache`, for a source whose file
    /// is replaced under a stable name. A file that is not a pgdt cache is
    /// refused all the same ([`Unusable::overwritable`]). Only
    /// [`CacheMode::Enabled`] has a cache to replace; a disabled mode is
    /// returned unchanged.
    pub fn with_overwrite_unusable(self, overwrite_unusable: bool) -> CacheMode {
        match self {
            CacheMode::Enabled { path, strict, .. } => {
                CacheMode::Enabled { path, strict, overwrite_unusable }
            }
            disabled @ CacheMode::Disabled { .. } => disabled,
        }
    }

    /// Which identity signals this mode binds.
    pub fn strict_identity(&self) -> StrictIdentity {
        match self {
            CacheMode::Enabled { strict, .. } | CacheMode::Disabled { strict } => *strict,
        }
    }

    /// The path this mode reads and writes, or `None` where it is disabled.
    pub fn path(&self) -> Option<&Path> {
        match self {
            CacheMode::Enabled { path, .. } => Some(path),
            CacheMode::Disabled { .. } => None,
        }
    }

    /// A loaded cache judged on its weak identity: refused where this mode
    /// binds a signal it fails, and otherwise carrying
    /// [`advisory_identity_diagnostics`] on its index — reported rather than
    /// acted on, too weak to invalidate, and recomputed on every load (see the
    /// module docs). **The one judgement every load with a live source
    /// makes**, so one selection means one thing on every command
    /// (`docs/design/decisions.md`, "D21").
    ///
    /// Only [`CacheMode::Enabled`] can refuse: [`CacheMode::Disabled`] loads
    /// no cache to compare.
    pub(super) fn judged(&self, mut loaded: LoadedCache) -> Result<LoadedCache> {
        if let CacheMode::Enabled { path, strict, .. } = self {
            // One selector at a time, in the order they are written: a run
            // that bound both and fails both is told about the modification
            // signal first, there being no second sentence to print after a
            // refusal.
            let unmet = [
                strict.time().then(|| ("time", loaded.weak.unmet())),
                strict.location().then(|| ("location", loaded.origin.unmet())),
            ];
            if let Some(refusal) = unmet.into_iter().flatten().find_map(|(term, unmet)| {
                unmet.map(|unmet| Error::StrictIdentityUnmet { path: path.clone(), term, unmet })
            }) {
                return Err(refusal);
            }
        }
        loaded
            .index
            .diagnostics
            .extend(advisory_identity_diagnostics(&loaded.weak, &loaded.origin));
        Ok(loaded)
    }

    /// What a scan entry point does about a cache it cannot use: the refusal,
    /// or `None` where this mode may start cold and replace it at its first
    /// save (`docs/design/decisions.md`, "D20"). The one decision every open
    /// makes ([`CacheMode::open`], [`CacheMode::open_source`]), whichever
    /// step finds the cache unusable — the claim, recognition refusing it, or
    /// the live source — so a refusal is word for word the same wherever it
    /// is made. The path lives here rather than on [`Unusable`] because
    /// [`CacheMode::Disabled`] has none.
    ///
    /// Only [`CacheMode::Enabled`] finds a cache to refuse; a disabled mode
    /// reads none, so it starts cold over whatever is there, as it always
    /// does.
    pub fn refusal(&self, unusable: &Unusable) -> Option<Error> {
        match self {
            CacheMode::Enabled { overwrite_unusable: true, .. } if unusable.overwritable() => None,
            CacheMode::Enabled { path, .. } => {
                Some(Error::CacheUnusable { path: path.clone(), unusable: unusable.clone() })
            }
            CacheMode::Disabled { .. } => None,
        }
    }

    /// Persist `index` per this mode. A disabled cache is a no-op — nothing
    /// is written, by design. A caller whose operation exists to populate the
    /// cache rejects a disabled mode up front with
    /// [`CacheMode::require_enabled`] instead of relying on this. `watch` is
    /// what an enabled save checks before it writes ([`save`]).
    pub async fn save(
        &self,
        watch: &SourceWatch,
        source: &dyn ByteRangeSource,
        index: &DumpIndex,
    ) -> Result<()> {
        match self {
            CacheMode::Enabled { path, .. } => {
                // **The check precedes the write**, so a run whose source
                // moved underneath it (`docs/design/decisions.md`, "D21")
                // writes no partial map and no statistics, and leaves
                // whatever is already at `path` exactly as it is
                // (`docs/design/decisions.md`, "D20").
                watch.check(source).await?;
                save(path, source, index).await
            }
            // **No check here**: a disabled cache writes nothing, so there is
            // no save for one to ride, and this run's checks are the ones its
            // readers make as they end ([`SourceWatch`]).
            CacheMode::Disabled { .. } => Ok(()),
        }
    }

    /// The path this mode resolves to, or `Error::CacheDisabled` if the
    /// caller disabled the cache but `operation` requires one — so a
    /// disabled cache is rejected up front rather than discovered when a
    /// write silently no-ops.
    pub fn require_enabled(&self, operation: &'static str) -> Result<&Path> {
        match self {
            CacheMode::Enabled { path, .. } => Ok(path),
            CacheMode::Disabled { .. } => Err(Error::CacheDisabled { operation }),
        }
    }
}

/// A cache read with **no live dump source at all** — answered strictly from
/// the file at its path (`docs/design/decisions.md`,
/// "The compressed source and the cache"), as `pgdt info` with no `--source`
/// does. Its own type rather than a [`CacheMode`], since every entry point
/// that takes a mode reads a live source; it carries no strictness, there
/// being nothing to compare against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineCache {
    path: PathBuf,
}

impl OfflineCache {
    /// The cache at `path`.
    pub fn new(path: impl Into<PathBuf>) -> OfflineCache {
        OfflineCache { path: path.into() }
    }

    /// The path this cache is read from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the cache. A loaded one is answered whole, complete or not —
    /// with no scan to fall back on a caller needs the coverage — and gets a
    /// [`pgdt_model::DiagnosticKind::CacheOffline`] pushed onto it
    /// unconditionally: unverified, historical. A hard I/O error reading the
    /// path propagates; what the file *holds* is answered as a status.
    pub async fn load(&self) -> Result<CacheStatus> {
        let mut status = load_offline(&self.path).await?;
        if let CacheStatus::Loaded(loaded) = &mut status {
            loaded.index.diagnostics.push(Diagnostic::cache_offline());
        }
        Ok(status)
    }
}
