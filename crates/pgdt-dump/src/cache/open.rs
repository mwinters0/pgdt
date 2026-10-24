//! A source opened through its cache ([`CacheMode::open`]): the cache read
//! once, the source opened on what it settles, the watch taken, and the cache
//! judged against the live source — and what that hands back ([`Opened`],
//! [`Held`]). A source already in hand is opened the same way, short of the
//! step that builds it ([`CacheMode::open_source`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::file::{CacheFile, CacheStatus, LoadedCache, read_cache_file};
use super::identity::SourceWatch;
use super::mode::CacheMode;
use super::store::{against_source, drop_attributed, settled_by};
use pgdt_core::{Result, Unusable};
use pgdt_io::{ByteRangeSource, KnownCompression, Origin, Recognized};

/// A source opened through its cache: the source, the watch its run keeps on
/// it, what the cache held, and the mode it was opened under — every step of
/// the open protocol taken, in its order, by [`CacheMode::open`] or
/// [`CacheMode::open_source`]. The mapping pass and `crate::preamble_only`
/// take one whole, so the cache they build forward from is the one the open
/// decoded and judged, and the cache they save to is the one it read.
///
/// **Only an open builds one**: its fields are readable, but it is
/// `#[non_exhaustive]`, so no crate above assembles one from the parts of
/// different opens, and a destructuring names `..`.
#[non_exhaustive]
pub struct Opened {
    /// The source, ready to read: built, where [`CacheMode::open`] loaded a
    /// cache, from the compression layer it settled, so an `.xz` file whose
    /// seek table the cache holds is not walked again
    /// (`docs/design/decisions.md`, "D18"); opened claiming nothing where it
    /// did not, and built by the caller under [`CacheMode::open_source`].
    pub source: Arc<dyn ByteRangeSource>,
    /// The baseline every later check of the source compares against, taken
    /// before the cache was judged against it.
    pub watch: SourceWatch,
    /// What the cache held.
    pub cache: Held,
    /// The mode the cache was opened under: the path a pass saves to, and
    /// the selection its watch and its refusals were taken under. Whole, not
    /// narrowed to where it saves: its strictness and `overwrite_unusable` are
    /// the open's own values, and a narrower type would be a second name and
    /// a second `save`.
    pub mode: CacheMode,
}

/// What an open found at the mode's path, short of a refusal.
///
/// **An unusable cache is no outcome here**: it is the open's refusal, the
/// one a scan entry point makes (`CacheMode::refusal`), unless the mode may
/// replace it, which is [`Held::Replacing`].
// One value a run, moved once or twice and never stored in bulk, so boxing
// the loaded cache would buy an allocation and save nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Held {
    /// A cache this build reads, judged on its identity: complete or not
    /// ([`LoadedCache::is_complete`]), carrying the advisory diagnostics its
    /// weak signals earned.
    Loaded(LoadedCache),
    /// Nothing at the path, which every scan starts cold on unasked.
    Missing,
    /// The mode is disabled: no path was consulted, and whatever sits where
    /// the cache would otherwise be is untouched. A statement about the
    /// caller (`docs/design/decisions.md`, "D22").
    Disabled,
    /// A cache this mode cannot use and may replace, and why: a scan starts
    /// cold over it and writes over it at its first save. Nothing the cache
    /// recorded reaches the source: [`CacheMode::open`] opens it claiming
    /// nothing, and [`CacheMode::open_source`]'s was built before the cache
    /// was read.
    Replacing(Unusable),
}

impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened")
            .field("watch", &self.watch)
            .field("cache", &self.cache)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

impl CacheMode {
    /// Open `origin` through the cache this mode names, **decoding the cache
    /// once**.
    ///
    /// The steps, in their order:
    ///
    /// 1. **What the cache settles before a source exists**: its compression
    ///    layer, or its refusal — a cache this build cannot read, or one whose
    ///    stored size the origin's probe contradicts — so an `.xz` file never
    ///    walks its stream footers to reach a refusal the cache alone settles.
    /// 2. **The source, opened on that layer**, a contradiction between the
    ///    two refused as [`Unusable::CompressionContradicted`]
    ///    (`docs/design/decisions.md`, "D18").
    /// 3. **The watch** ([`SourceWatch::open`]), taken before the cache is
    ///    believed, so a source that changes from here on is caught. A source
    ///    nothing can check during a run opens all the same, and is refused
    ///    where a read of it begins ([`SourceWatch::require_checkable`]).
    /// 4. **The cache judged against the live source**: the stored size and
    ///    the compression layer again, which refuse, then the weak signals,
    ///    which refuse only where the mode binds them and otherwise warn
    ///    (`docs/design/decisions.md`, "D21").
    ///
    /// **Every refusal is word for word the one a scan entry point makes**,
    /// being `CacheMode::refusal`'s, and a mode that may replace the cache
    /// opens the source claiming nothing and answers [`Held::Replacing`]
    /// instead (`docs/design/decisions.md`, "D20").
    ///
    /// **A gzip first pass the open builds writes its windows** to a file
    /// with no name in the enabled cache's directory, or else the OS temp
    /// directory (`docs/design/decisions.md`, "D115").
    pub async fn open(&self, origin: &Origin) -> Result<Opened> {
        let spill = self.spill_dir();
        let claimed = match self.path() {
            None => Claimed::Nothing(Held::Disabled),
            Some(path) => match read_cache_file(path)? {
                Ok(file) => match settled_by(&file, origin).await {
                    Ok(known) => Claimed::File(file, known),
                    Err(why) => {
                        drop_attributed(file.index);
                        Claimed::Nothing(self.replacing(why)?)
                    }
                },
                Err(CacheStatus::Unusable(why)) => Claimed::Nothing(self.replacing(why)?),
                Err(_) => Claimed::Nothing(Held::Missing),
            },
        };
        let (source, file) = match claimed {
            Claimed::File(file, known) => match pgdt_io::open(origin, known, &spill).await? {
                Recognized::Source(source) => (source, Ok(file)),
                Recognized::Mismatch => {
                    drop_attributed(file.index);
                    let held = self.replacing(Unusable::CompressionContradicted)?;
                    (open_claiming_nothing(origin, &spill).await?, Err(held))
                }
            },
            Claimed::Nothing(held) => (open_claiming_nothing(origin, &spill).await?, Err(held)),
        };
        let watch = SourceWatch::open(source.as_ref(), self.strict_identity()).await?;
        let file = match file {
            Ok(file) => file,
            Err(cache) => return Ok(self.opened(source, watch, cache)),
        };
        match against_source(file, source.as_ref()).await? {
            CacheStatus::Loaded(loaded) => {
                Ok(self.opened(source, watch, Held::Loaded(self.judged(loaded)?)))
            }
            // The file moved between the probe and the open, so the source
            // was built on a claim about another file — or the probe failed
            // and it was built on none, the cache being another file's:
            // opened again on none.
            CacheStatus::Unusable(why) => {
                let cache = self.replacing(why)?;
                let source = open_claiming_nothing(origin, &spill).await?;
                let watch = SourceWatch::open(source.as_ref(), self.strict_identity()).await?;
                Ok(self.opened(source, watch, cache))
            }
            CacheStatus::Missing => unreachable!("a decoded file is never missing"),
        }
    }

    /// Open `source`, **already in hand**, through the cache this mode
    /// names: [`CacheMode::open`] short of building the source, for a caller
    /// that built its own — a wrapper over another, or one opened before the
    /// cache was chosen.
    ///
    /// The watch is taken first, then the cache decoded once and judged
    /// against the source — its stored size and compression layer, which
    /// refuse, then the weak signals — each refusal word for word
    /// [`CacheMode::open`]'s. **Nothing the cache records reaches `source`**,
    /// which was built before it was read: a caller holding an `.xz` file
    /// whose seek table a cache holds opens it from its [`Origin`] instead,
    /// so the stream footers are not walked again
    /// (`docs/design/decisions.md`, "D18").
    pub async fn open_source(&self, source: Arc<dyn ByteRangeSource>) -> Result<Opened> {
        let watch = SourceWatch::open(source.as_ref(), self.strict_identity()).await?;
        let cache = match self.path() {
            None => Held::Disabled,
            Some(path) => match read_cache_file(path)? {
                Ok(file) => match against_source(file, source.as_ref()).await? {
                    CacheStatus::Loaded(loaded) => Held::Loaded(self.judged(loaded)?),
                    CacheStatus::Unusable(why) => self.replacing(why)?,
                    CacheStatus::Missing => unreachable!("a decoded file is never missing"),
                },
                Err(CacheStatus::Unusable(why)) => self.replacing(why)?,
                Err(_) => Held::Missing,
            },
        };
        Ok(self.opened(source, watch, cache))
    }

    /// Where a gzip file's first pass, opened under this mode, writes the
    /// windows it places: the enabled cache's directory, which a save renames
    /// into, or the OS temp directory where the cache is disabled
    /// (`docs/design/decisions.md`, "D115").
    fn spill_dir(&self) -> PathBuf {
        match self.path().map(Path::parent) {
            Some(Some(dir)) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            Some(_) => PathBuf::from("."),
            None => std::env::temp_dir(),
        }
    }

    /// What an open hands back, under this mode.
    fn opened(&self, source: Arc<dyn ByteRangeSource>, watch: SourceWatch, cache: Held) -> Opened {
        Opened { source, watch, cache, mode: self.clone() }
    }

    /// `why`'s refusal, or [`Held::Replacing`] where this mode may replace
    /// the cache.
    fn replacing(&self, why: Unusable) -> Result<Held> {
        match self.refusal(&why) {
            Some(refusal) => Err(refusal),
            None => Ok(Held::Replacing(why)),
        }
    }
}

/// What the cache settled before the source was opened.
enum Claimed {
    /// A decoded cache file, and the compression layer it settles.
    File(CacheFile, KnownCompression),
    /// No cache to believe, and what the open will say of it.
    Nothing(Held),
}

/// Open `origin` with nothing claimed about its compression layer, which
/// recognition then cannot be contradicted on, a first pass spilling into
/// `spill`.
async fn open_claiming_nothing(origin: &Origin, spill: &Path) -> Result<Arc<dyn ByteRangeSource>> {
    match pgdt_io::open(origin, KnownCompression::Unknown, spill).await? {
        Recognized::Source(source) => Ok(source),
        Recognized::Mismatch => unreachable!("`Unknown` claims nothing to contradict"),
    }
}

#[cfg(test)]
mod tests;
