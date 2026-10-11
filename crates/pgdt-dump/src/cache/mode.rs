//! How a caller wants the cache handled for one operation ([`CacheMode`]),
//! the identity signals it binds among them, and what a load answers a
//! caller holding a live source ([`CacheLoad`]).

use std::path::{Path, PathBuf};

use super::identity::{
    OriginMatch, SourceWatch, StrictIdentity, WeakIdentity, advisory_identity_diagnostics,
};
use super::{CacheStatus, colocated_path, load, load_offline, save};
use pgdt_core::{Error, Result, Unusable};
use pgdt_io::{ByteRangeSource, Origin};
use pgdt_model::internal::DiagnosticExt;
use pgdt_model::{Diagnostic, DumpIndex};

/// How a caller wants the structure cache handled for one operation, and
/// which identity signals bind while it does. Constructed via
/// [`CacheMode::resolve`], [`CacheMode::enabled`] or [`CacheMode::DISABLED`],
/// each of which leaves [`StrictIdentity::ADVISORY`] and refuses an unusable
/// cache; [`CacheMode::with_strict_identity`] and
/// [`CacheMode::with_overwrite_unusable`] are what state anything else.
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
    /// No live dump source at all — answer strictly from the cache at this
    /// path (`docs/design/decisions.md`,
    /// "The compressed source and the cache"). Never constructed by
    /// [`CacheMode::resolve`]; a caller builds it directly (`pgdt info` with
    /// no `--source`). Every method below that takes a live `source` rejects
    /// it as a caller-contract violation, and [`CacheMode::load_offline`]
    /// rejects `Enabled`/`Disabled` the other way. It carries no strictness: with no
    /// source there is nothing to compare against.
    Offline(PathBuf),
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

    /// Resolve a `--dtcache`-style argument against the source it is for:
    /// `None` selects the default, the literal path `none` disables the
    /// cache, and any other path is used as-is.
    ///
    /// **The default is the name of the dump plus `.dtcache`, and only where
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
            Some(p) if p == Path::new("none") => CacheMode::DISABLED,
            Some(p) => CacheMode::enabled(p),
            // A remote origin always names an object: a URL that names none is
            // refused when the origin is built, so there is no third case.
            None => CacheMode::enabled(colocated_path(
                origin.local_path().unwrap_or(Path::new(origin.remote_name().unwrap_or_default())),
            )),
        }
    }

    /// State which identity signals bind. [`CacheMode::Offline`] has no live
    /// source to compare against and is returned unchanged.
    pub fn with_strict_identity(self, strict: StrictIdentity) -> CacheMode {
        match self {
            CacheMode::Enabled { path, overwrite_unusable, .. } => {
                CacheMode::Enabled { path, strict, overwrite_unusable }
            }
            CacheMode::Disabled { .. } => CacheMode::Disabled { strict },
            CacheMode::Offline(path) => CacheMode::Offline(path),
        }
    }

    /// State whether a scan may start cold over a cache it cannot use and
    /// replace it — `pgdt --overwrite-unusable-cache`, for a source whose file
    /// is replaced under a stable name. A file that is not a pgdt cache is
    /// refused all the same ([`Unusable::overwritable`]). Only
    /// [`CacheMode::Enabled`] has a cache to replace; the others are returned
    /// unchanged.
    pub fn with_overwrite_unusable(self, overwrite_unusable: bool) -> CacheMode {
        match self {
            CacheMode::Enabled { path, strict, .. } => {
                CacheMode::Enabled { path, strict, overwrite_unusable }
            }
            other => other,
        }
    }

    /// Which identity signals this mode binds — [`StrictIdentity::ADVISORY`]
    /// for the cache-only mode, which checks nothing.
    pub fn strict_identity(&self) -> StrictIdentity {
        match self {
            CacheMode::Enabled { strict, .. } | CacheMode::Disabled { strict } => *strict,
            CacheMode::Offline(_) => StrictIdentity::ADVISORY,
        }
    }

    /// Load the cache this mode points at, turning the
    /// [`CacheStatus::Valid`] identity answers into the advisory diagnostics
    /// of [`advisory_identity_diagnostics`] on the index — a moved time, a
    /// changed entity tag, or a different origin (see the module docs). A
    /// disabled cache always
    /// yields [`CacheLoad::Disabled`], even if a file sits at what would
    /// otherwise be its resolved location.
    ///
    /// **Under a binding [`StrictIdentity`] the same answers are a refusal**:
    /// the caller asked for a guarantee, so a cache whose modification signal
    /// has moved, whose origin differs, or a source that can offer neither,
    /// stops the run instead of carrying a diagnostic nobody has to read.
    /// `time` and `location` each bind their own half
    /// ([`CacheMode::strict_identity_refusal`]). It is not this method's only
    /// `Err`: an [`CacheMode::Offline`] mode is a
    /// [`Error::CacheModeMismatch`], and [`load`]'s own I/O failures
    /// propagate.
    ///
    /// `Incomplete` is treated exactly like `Valid` — both are
    /// [`CacheLoad::Index`] — because every caller of this method
    /// (`pgdt_mapping::map_file`, `pgdt_query`'s `map_for_query`, which
    /// serves both `pgdt_query::table_stream` and `pgdt_query::table_stream_partitions`,
    /// and `crate::preamble::preamble_only`) builds forward from whatever partial
    /// map exists (`docs/design/decisions.md`, "D22"). A caller that instead
    /// *reports* what a cache holds reaches for
    /// [`load`]/[`CacheMode::load_offline`] and the full [`CacheStatus`], as
    /// `pgdt info` does. Neither an unusable cache's reason nor the caller's
    /// own opt-out is collapsed, and such a caller asks
    /// [`CacheMode::strict_identity_refusal`] for the check this method makes
    /// inline. An unusable cache is answered, not refused, here: whether to
    /// refuse is [`CacheMode::refusal`]'s, asked by the caller that would
    /// scan.
    pub async fn load(&self, source: &dyn ByteRangeSource) -> Result<CacheLoad> {
        match self {
            CacheMode::Enabled { path, .. } => Ok(match load(path, source).await? {
                CacheStatus::Missing => CacheLoad::Missing,
                CacheStatus::Unusable(why) => CacheLoad::Unusable(why),
                CacheStatus::Valid { mut index, weak, origin, .. }
                | CacheStatus::Incomplete { mut index, weak, origin, .. } => {
                    if let Some(refusal) = self.strict_identity_refusal(&weak, &origin) {
                        return Err(refusal);
                    }
                    // Reported rather than acted on: too weak to invalidate,
                    // and recomputed on every load (see the module docs).
                    index.diagnostics.extend(advisory_identity_diagnostics(&weak, &origin));
                    CacheLoad::Index(index)
                }
            }),
            CacheMode::Disabled { .. } => Ok(CacheLoad::Disabled),
            CacheMode::Offline(_) => Err(Error::CacheModeMismatch(
                "a live dump source requires CacheMode::Enabled or CacheMode::Disabled, not Offline",
            )),
        }
    }

    /// The refusal [`CacheMode::load`] makes on `weak`, or `None` where this
    /// mode binds nothing that `weak` fails — exposed because a caller that
    /// *reports* what a cache holds reads the full [`CacheStatus`] through
    /// [`load`] and so never passes through `load`'s own check
    /// (`pgdt info`). The comparison stays here rather than at that caller,
    /// so one selection means one thing on every command
    /// (`docs/design/decisions.md`, "D21").
    ///
    /// Only [`CacheMode::Enabled`] can refuse: [`CacheMode::Disabled`] loads
    /// no cache to compare, and [`CacheMode::Offline`] has no live source, so
    /// its identity is null rather than absent and a selection never reaches
    /// it.
    pub fn strict_identity_refusal(
        &self,
        weak: &WeakIdentity,
        origin: &OriginMatch,
    ) -> Option<Error> {
        let CacheMode::Enabled { path, strict, .. } = self else { return None };
        // One selector at a time, in the order they are written: a run that
        // bound both and fails both is told about the modification signal
        // first, there being no second sentence to print after a refusal.
        let unmet = [
            strict.time().then(|| ("time", weak.unmet())),
            strict.location().then(|| ("location", origin.unmet())),
        ];
        unmet.into_iter().flatten().find_map(|(term, unmet)| {
            unmet.map(|unmet| Error::StrictIdentityUnmet { path: path.clone(), term, unmet })
        })
    }

    /// What a scan entry point does about a cache it cannot use: the refusal,
    /// or `None` where this mode may start cold and replace it at its first
    /// save (`docs/design/decisions.md`, "D20"). One decision for every caller
    /// — the four scan entry points on [`CacheLoad::Unusable`], and one
    /// pre-empting them on
    /// [`CacheClaim::Unusable`](super::CacheClaim::Unusable) or on recognition
    /// refusing the cache's claim — so a refusal is word for word the one it
    /// pre-empts. The path lives here rather than on [`Unusable`] because
    /// [`CacheMode::Disabled`] has none.
    ///
    /// Only [`CacheMode::Enabled`] finds a cache to refuse; `Disabled` and
    /// `Offline` answer the caller-contract error.
    pub fn refusal(&self, unusable: &Unusable) -> Option<Error> {
        match self {
            CacheMode::Enabled { overwrite_unusable: true, .. } if unusable.overwritable() => None,
            CacheMode::Enabled { path, .. } => {
                Some(Error::CacheUnusable { path: path.clone(), unusable: unusable.clone() })
            }
            CacheMode::Disabled { .. } | CacheMode::Offline(_) => Some(Error::CacheModeMismatch(
                "an unusable cache is only reachable from CacheMode::Enabled",
            )),
        }
    }

    /// Load this mode's cache with no live source to check it against — the
    /// cache-only counterpart to [`CacheMode::load`]
    /// (`docs/design/decisions.md`,
    /// "The compressed source and the cache"). Returns the full
    /// [`CacheStatus`] rather than a [`CacheLoad`]: with no scan to fall back
    /// on, a caller needs `Valid` apart from `Incomplete` — the one
    /// distinction `load` exists to erase. Every successful load gets a
    /// [`pgdt_model::DiagnosticKind::CacheOffline`] pushed onto it
    /// unconditionally.
    pub async fn load_offline(&self) -> Result<CacheStatus> {
        match self {
            CacheMode::Offline(path) => {
                let mut status = load_offline(path).await?;
                match &mut status {
                    CacheStatus::Missing | CacheStatus::Unusable(_) => {}
                    CacheStatus::Valid { index, .. } | CacheStatus::Incomplete { index, .. } => {
                        index.diagnostics.push(Diagnostic::cache_offline());
                    }
                }
                Ok(status)
            }
            CacheMode::Enabled { .. } | CacheMode::Disabled { .. } => {
                Err(Error::CacheModeMismatch("cache-only access requires CacheMode::Offline"))
            }
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
                // **The check precedes the write** and is the whole of D12's
                // "saves nothing": a run whose source moved underneath it
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
            CacheMode::Offline(_) => {
                Err(Error::CacheModeMismatch("cache-only mode never has a fresh scan to persist"))
            }
        }
    }

    /// The path this mode resolves to, or `Error::CacheDisabled` if the
    /// caller disabled the cache but `operation` requires one — so an
    /// incompatible `--dtcache none` is rejected up front rather than
    /// discovered when a write silently no-ops.
    pub fn require_enabled(&self, operation: &'static str) -> Result<&Path> {
        match self {
            CacheMode::Enabled { path, .. } => Ok(path),
            CacheMode::Disabled { .. } => Err(Error::CacheDisabled { operation }),
            CacheMode::Offline(_) => Err(Error::CacheModeMismatch(
                "cache-only mode has no live source to scan and persist",
            )),
        }
    }
}

/// What [`CacheMode::load`] answered a caller that holds a live source: an
/// index to build forward from, or the reason there is none
/// (`docs/design/decisions.md`, "D22").
///
/// `Disabled` is a statement about the *caller*, with no [`CacheStatus`] to
/// correspond to; the other three carry that status across, `Missing` and
/// `Unusable` unchanged and `Valid` and `Incomplete` as the one `Index`,
/// because a caller holding a live source builds forward from either — see
/// [`CacheMode::load`].
///
/// *Rejected:* an accessor collapsing the reasons back to `Option` for the
/// callers that do not care. It rebuilds the collapse under a shorter name at
/// exactly the sites that must not have it, and the callers that genuinely do
/// not care are tests, which can say so in a `let … else`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheLoad {
    /// A usable index — a [`CacheStatus::Valid`] or
    /// [`CacheStatus::Incomplete`] cache, carrying whatever diagnostics the
    /// load computed about the cache *file*.
    Index(DumpIndex),
    /// [`CacheMode::Disabled`]: no path was consulted, and whatever sits at
    /// the location this mode would otherwise have resolved is untouched.
    Disabled,
    /// [`CacheStatus::Missing`] — nothing at the cache path, the one outcome
    /// every scan entry point starts cold on unasked.
    Missing,
    /// [`CacheStatus::Unusable`]. **The four scan entry points refuse on it**
    /// before the dump is read past its magic, unless the mode may overwrite it —
    /// [`CacheMode::refusal`] is that decision, in one place
    /// (`docs/design/decisions.md`, "D20").
    Unusable(Unusable),
}
