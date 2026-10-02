//! On-disk structure cache: a serialized [`DumpIndex`], colocated with the
//! dump file by default or at an explicit path
//! (`docs/design/decisions.md`, "The compressed source and the cache").
//!
//! **A cache that cannot be used is refused, not scanned over**
//! (`docs/design/decisions.md`, "D20"): one that is another file's, another
//! build's, or damaged is almost always a user's accident — the wrong path, or
//! a file that changed — so the four scan entry points answer
//! [`Error::CacheUnusable`] before the dump is read past its magic, naming which of
//! those it is ([`Unusable`]). [`CacheMode::with_overwrite_unusable`] opts into
//! starting cold and replacing it instead, for a source whose file is replaced
//! under a stable name; a file that is not a pgdt cache at all is refused
//! whatever is asked. Only a missing cache starts cold unasked. Writing is not
//! best-effort either: [`save`] propagates I/O failures rather than silently
//! degrading every future run of the same command back to a full scan.
//!
//! **The dump file's identity is checked, not assumed**
//! (`docs/design/decisions.md`, "D21"). Every cache records the source's
//! *stored* size and modification signal as observed at save time — bytes on
//! the device, not the addressable (possibly decompressed) length — and, for a
//! source that was fetched from somewhere, **where it came from** and the
//! server's entity tag
//! (`docs/design/decisions.md`, "D87"); [`load`]
//! re-observes the live source and compares. A stored-size mismatch means
//! every byte offset in the cache could be wrong, so the cache is unusable.
//! Neither weak signal invalidates it by default: they are surfaced on
//! [`CacheStatus::Valid`] as a [`WeakIdentity`] and an [`OriginMatch`], and
//! [`CacheMode::load`] turns each into a warning on the loaded index,
//! recomputed on every load and never persisted. A caller that asked for
//! [`StrictIdentity::time`] or [`StrictIdentity::location`] is refused
//! instead — there, or at [`CacheMode::strict_identity_refusal`] where it read
//! the status itself.
//!
//! **Two different questions hide under one word.** *Between* runs a moved,
//! copied or touched dump is a different weak identity holding the same
//! bytes, which is what the advisory default is right for. *During* a run a
//! source whose identity changes is bytes moving underneath a read that has
//! already returned some of them, which no answer survives — so that is an
//! error under every selection but `none`, which only warns, and
//! [`SourceWatch`] is the mechanism.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bincode::error::{DecodeError, EncodeError};
use serde::{Deserialize, Serialize};

use crate::diagnostic::Diagnostic;
pub use crate::error::{OVERWRITE_WAYS_OUT, Unusable};
use crate::index::{
    DumpIndex, calendar_end, non_seekable_compression_diagnostic, tiling_diagnostics,
    toc_coverage_diagnostic,
};
use crate::instrument::StatisticsScope;
use crate::io::{ByteRangeSource, KnownCompression, Origin, is_weak_tag};
use crate::map::{DataBlock, SpanBody};
use crate::{Error, Result};

/// **Bump whenever a persisted field is added, removed or reshaped.** A cache
/// written under a different version is unusable
/// ([`Unusable::UnsupportedVersion`]) rather than partially trusted
/// (`docs/design/roadmap.md`, "Four decisions that keep later phases
/// additive"); pre-1.0 a bump is free, nothing migrates (`CLAUDE.md`,
/// "Pre-1.0"), and nothing records it (`docs/design/decisions.md`, "D22").
/// A new way of *reading* an existing on-disk shape needs no bump:
/// [`CacheStatus::Incomplete`] reinterprets `scanned_through` against a size
/// already stored. A new way of *choosing* what an unchanged field holds does:
/// a block recording the request that sized it is read back as already sized
/// under it, so a cache whose sizes today's rule would not choose is as
/// unusable as one of another shape. So does a parse fix that saves other
/// bytes for the same dump: `tests/cache.rs`'s
/// `persisted_index_is_pinned_to_the_format_version` pins what every fixture
/// persists beside this number, as `golden_order_is_pinned_to_the_format_version`
/// pins the comparison order, and each fails until it is bumped.
pub const CACHE_FORMAT_VERSION: u32 = 37;

/// The bytes every cache file opens with, ahead of [`CACHE_FORMAT_VERSION`] as
/// a little-endian `u32` and then the encoded [`CacheFile`]. **Both are read
/// before the rest is decoded, and neither is ever reshaped**, so any build
/// tells a file that is not a pgdt cache ([`Unusable::NotACache`]) from
/// another build's ([`Unusable::UnsupportedVersion`]) — the difference between
/// a file no scan may overwrite and one `--overwrite-unusable-cache` may
/// ([`CacheMode::with_overwrite_unusable`]).
const CACHE_MAGIC: [u8; 8] = *b"pgdtcach";

/// The dump file's identity as observed when a cache was last saved — see
/// the module docs.
///
/// **Opaque, not a struct** (`docs/design/decisions.md`, "D21"): each variant
/// carries whatever evidence its own kind of source has, an ETag not being a
/// `SystemTime` and a local file having no origin at all
/// (`docs/design/decisions.md`, "D87"). What the three
/// comparisons below read is a *signal* — origin, modification, stored size —
/// rather than a variant, so a pairing of two kinds is compared rather than
/// refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum SourceIdentity {
    /// Backed by [`ByteRangeSource::stored_size`]/`modified` — a source that
    /// was not fetched from anywhere, decompressing or not.
    LocalFile {
        stored_size: u64,
        /// `(seconds, nanoseconds)` since the Unix epoch: L1's own on-disk
        /// vocabulary, `SystemTime` not being `Serialize`. `None` when the
        /// source exposed no mtime.
        mtime: Option<(u64, u32)>,
    },
    /// An object fetched from somewhere, which records two things a file does
    /// not: **where it came from**, which is what
    /// [`StrictIdentity::location`] binds, and the server's **entity tag**,
    /// which is the stronger of the two modification signals
    /// [`StrictIdentity::time`] binds.
    Remote {
        origin: String,
        etag: Option<String>,
        /// `Last-Modified`, in [`SourceIdentity::LocalFile`]'s on-disk
        /// vocabulary. `None` where the server sent none — which is what a
        /// substituted epoch is read as (`crate::io`'s `weak_identity`).
        last_modified: Option<(u64, u32)>,
        stored_size: u64,
    },
}

impl SourceIdentity {
    async fn observe(source: &dyn ByteRangeSource) -> Result<Self> {
        let stored_size = source.stored_size().await?;
        let modified = source
            .modified()
            .await?
            .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default())
            .map(|d| (d.as_secs(), d.subsec_nanos()));
        // A source that was fetched from somewhere says so; every other one
        // records no origin, which is the decision rather than a gap
        // (`ByteRangeSource::remote_identity`).
        Ok(match source.remote_identity() {
            Some(remote) => SourceIdentity::Remote {
                origin: remote.origin().to_string(),
                etag: remote.etag().map(ToOwned::to_owned),
                last_modified: modified,
                stored_size,
            },
            None => SourceIdentity::LocalFile { stored_size, mtime: modified },
        })
    }

    /// Bytes as stored — the half that refuses (`docs/design/decisions.md`,
    /// "D20").
    fn stored_size(&self) -> u64 {
        match self {
            SourceIdentity::LocalFile { stored_size, .. }
            | SourceIdentity::Remote { stored_size, .. } => *stored_size,
        }
    }

    /// The modification time this identity carries, as a `SystemTime`: L1's
    /// on-disk `(seconds, nanoseconds)` is backed out at the boundary, so
    /// what leaves this module is what the trait answered and what an
    /// embedder formats.
    fn modified(&self) -> Option<SystemTime> {
        let stamp = match self {
            SourceIdentity::LocalFile { mtime, .. }
            | SourceIdentity::Remote { last_modified: mtime, .. } => *mtime,
        };
        stamp.map(|(s, n)| UNIX_EPOCH + Duration::new(s, n))
    }

    /// The server's entity tag, where this identity has one at all.
    fn etag(&self) -> Option<&str> {
        match self {
            SourceIdentity::LocalFile { .. } => None,
            SourceIdentity::Remote { etag, .. } => etag.as_deref(),
        }
    }

    /// Where the object was fetched from, or `None` for a source that was not
    /// fetched from anywhere (D87).
    fn origin(&self) -> Option<&str> {
        match self {
            SourceIdentity::LocalFile { .. } => None,
            SourceIdentity::Remote { origin, .. } => Some(origin),
        }
    }

    /// What the weak half of this identity says against `live`'s — the half
    /// that warns rather than refuses (see the module docs). **The answer
    /// keeps what was compared**, so a caller refusing on it states what it
    /// saw rather than only that it looked.
    ///
    /// **The strongest signal both sides carry decides it: a strong entity
    /// tag, then `Last-Modified`, then a weak tag.** A strong tag is the
    /// server's statement that these are the same bytes, so where both sides
    /// have one it settles the question and a `Last-Modified` disagreeing
    /// with it is not consulted (`docs/design/decisions.md`, "D21"). A weak
    /// one states only equivalent content (RFC 9110, §8.8.1), which is not
    /// what a map of byte offsets needs, so it cannot outrank a moved
    /// `Last-Modified` — the reason [`crate::io`] pins no read by it either —
    /// and decides only where neither side states one. **There it can show a
    /// change and never confirm one**: opaque tags that differ, with either
    /// side's `W/` set aside, are [`WeakIdentity::TagDiffers`], and equal ones
    /// are [`WeakIdentity::Unconfirmed`] — the same reason RFC 9110, §13.1.5
    /// forbids a weak tag in `If-Range`. Rejected: comparing them weakly
    /// (§8.8.3.2) and agreeing, which answers equivalence where the map asks
    /// for bytes; exact equality, `W/` being the tag's strength and not part
    /// of it.
    fn weak_against(&self, live: &Self) -> WeakIdentity {
        let tags = self.etag().zip(live.etag());
        if let Some((cached, live)) = tags.filter(|(c, l)| !is_weak_tag(c) && !is_weak_tag(l)) {
            return tag_against(cached, live, cached == live);
        }
        match (self.modified(), live.modified()) {
            (Some(cached), Some(live)) if cached == live => WeakIdentity::Agrees,
            (Some(cached), Some(live)) => WeakIdentity::Differs { cached, live },
            (None, None) => match tags {
                Some((cached, live)) => {
                    let opaque = |tag: &'_ str| tag.strip_prefix("W/").unwrap_or(tag).to_owned();
                    if opaque(cached) == opaque(live) {
                        WeakIdentity::Unconfirmed {
                            cached: cached.to_string(),
                            live: live.to_string(),
                        }
                    } else {
                        tag_against(cached, live, false)
                    }
                }
                None => WeakIdentity::Absent { cached: None, live: None },
            },
            (cached, live) => WeakIdentity::Absent { cached, live },
        }
    }

    /// What this identity's origin says against `live`'s — the second
    /// advisory answer, and the one [`StrictIdentity::location`] binds
    /// (`docs/design/decisions.md`, "D87").
    ///
    /// **Two sources that were both fetched from nowhere agree**, which is
    /// what makes `location` inert on a local file rather than a refusal
    /// nobody asked for (D87).
    fn origin_against(&self, live: &Self) -> OriginMatch {
        match (self.origin(), live.origin()) {
            (cached, live) if cached == live => OriginMatch::Agrees,
            (cached, live) => OriginMatch::Differs {
                cached: cached.map(ToOwned::to_owned),
                live: live.map(ToOwned::to_owned),
            },
        }
    }

    /// How a run that started from this identity and now sees `live` says
    /// what moved — [`Error::SourceChangedWhileRead`]'s evidence clause.
    /// Never called where the two are equal, so it is never empty.
    fn differences(&self, live: &Self) -> String {
        let mut said = Vec::new();
        let (was, now) = (self.stored_size(), live.stored_size());
        if was != now {
            said.push(format!("its stored size went from {was} to {now} byte(s)"));
        }
        if self.modified() != live.modified() {
            said.push("its modification time moved".to_string());
        }
        if self.etag() != live.etag() {
            said.push("the server's entity tag for it changed".to_string());
        }
        if !matches!(self.origin_against(live), OriginMatch::Agrees) {
            said.push("it is no longer the same origin".to_string());
        }
        said.join(" and ")
    }
}

/// A tag comparison's answer, `matched` being whichever comparison the
/// tags' strength called for ([`SourceIdentity::weak_against`]).
fn tag_against(cached: &str, live: &str, matched: bool) -> WeakIdentity {
    if matched {
        WeakIdentity::Agrees
    } else {
        WeakIdentity::TagDiffers { cached: cached.to_string(), live: live.to_string() }
    }
}

/// What the origin recorded in a cache said against the live source's — the
/// second half of what a cache load compares, beside [`WeakIdentity`]
/// (`docs/design/decisions.md`, "D87").
///
/// **Advisory by default and only two states**, where the modification signal
/// has five: an absent origin is not silence, it is the positive statement
/// that a source was not fetched from anywhere, so two of them agree and
/// [`StrictIdentity::location`] binds nothing on a local file (D87).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginMatch {
    /// Both were fetched from the same place, or neither was fetched at all.
    Agrees,
    /// They differ — advisory by default
    /// ([`crate::diagnostic::DiagnosticKind::CacheOriginChanged`]) — with
    /// each side's, `None` for a source that records none.
    Differs { cached: Option<String>, live: Option<String> },
}

impl OriginMatch {
    /// Why [`StrictIdentity::location`] is not met, or `None` where it is —
    /// the clause [`Error::StrictIdentityUnmet`] carries, naming the two
    /// origins it compared.
    fn unmet(&self) -> Option<String> {
        let OriginMatch::Differs { cached, live } = self else { return None };
        let named = |origin: &Option<String>| match origin {
            Some(origin) => format!("`{origin}`"),
            None => "a source fetched from nowhere".to_string(),
        };
        Some(format!(
            "the cache was written for {}, and this run reads {}",
            named(cached),
            named(live)
        ))
    }
}

/// What the weak half of identity said when a cache was checked against the
/// live source it was written for (`docs/design/decisions.md`, "D21").
///
/// **Five states rather than a `bool`, because [`StrictIdentity::time`]
/// refuses on four of them.** A source that offers no modification signal at
/// all and a cache that recorded none agree on nothing: they are silent, and
/// silence is exactly what a caller asking for a guarantee is refused on. A
/// weak entity tag that agrees is the same want of a guarantee, spoken.
///
/// **Each state carries what was compared**, so the refusal built from it
/// states the evidence and not only the verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WeakIdentity {
    /// The cache's recorded modification signal is the source's.
    Agrees,
    /// The entity tags differ — both strong, the server's own statement that
    /// this is a different version of the object, which outranks whatever
    /// `Last-Modified` says (`docs/design/decisions.md`, "D21"); or either
    /// weak where neither side states a `Last-Modified`. Advisory by default,
    /// exactly as [`WeakIdentity::Differs`] is.
    TagDiffers { cached: String, live: String },
    /// They differ — advisory by default
    /// ([`crate::diagnostic::DiagnosticKind::CacheMtimeChanged`]) — with the
    /// time the cache recorded and the one the live source now reports.
    Differs { cached: SystemTime, live: SystemTime },
    /// One side has no modification signal, so there is nothing to compare;
    /// at least one of the two is `None`. Whatever the other side offered
    /// travels all the same, so a refusal can name which one is silent.
    Absent { cached: Option<SystemTime>, live: Option<SystemTime> },
    /// Neither side states a `Last-Modified`, and the entity tags are equal
    /// once a `W/` is set aside but at least one is weak: a weak tag promises
    /// equivalent content, not the bytes a map of offsets describes, so it
    /// confirms nothing ([`SourceIdentity::weak_against`]). **Silent by
    /// default, as `Absent` is** — an advisory warning reports evidence of a
    /// change, and there is none — and refused under [`StrictIdentity::time`],
    /// naming the weak tag. Both tags travel, so a source stating a strong one
    /// is not called silent.
    Unconfirmed { cached: String, live: String },
}

impl WeakIdentity {
    /// Why [`StrictIdentity::time`] is not met, or `None` where it is — the
    /// clause [`Error::StrictIdentityUnmet`] carries, naming the times it
    /// compared.
    fn unmet(&self) -> Option<String> {
        match self {
            WeakIdentity::Agrees => None,
            WeakIdentity::TagDiffers { cached, live } => Some(format!(
                "the server's entity tag for it has changed since — the cache recorded {cached}, \
                 the source now reports {live}"
            )),
            WeakIdentity::Differs { cached, live } => Some(format!(
                "its modification time has moved since — the cache recorded {}, the source now reports {}",
                epoch_stamp(*cached),
                epoch_stamp(*live)
            )),
            // The fourth pairing is not a state `weak_against` builds, and it
            // falls in with `(None, None)` rather than being refuted here:
            // this clause is a message, not the place to assert an invariant.
            WeakIdentity::Absent { cached, live } => Some(match (cached, live) {
                (Some(cached), None) => format!(
                    "the source carries no modification time to compare with the {} the cache recorded",
                    epoch_stamp(*cached)
                ),
                (None, Some(live)) => format!(
                    "the cache carries no modification time to compare with the source's {}",
                    epoch_stamp(*live)
                ),
                _ => "neither it nor the source carries a modification time to compare".to_string(),
            }),
            WeakIdentity::Unconfirmed { cached, live } => {
                let which = match (is_weak_tag(cached), is_weak_tag(live)) {
                    (true, true) => format!(
                        "the cache recorded only the weak entity tag {cached} and the source reports \
                         only {live}"
                    ),
                    (true, false) => format!(
                        "the cache recorded only the weak entity tag {cached} to compare with the \
                         source's {live}"
                    ),
                    _ => format!(
                        "the source reports only the weak entity tag {live} to compare with the \
                         {cached} the cache recorded"
                    ),
                };
                Some(format!(
                    "{which}, and no modification time — a weak tag promises equivalent content, \
                     not the same bytes, so it cannot confirm the source is unchanged"
                ))
            }
        }
    }
}

/// A modification time as a refusal states it: seconds and nanoseconds since
/// the Unix epoch, rather than a calendar date: the value is what
/// `date -d @<n>` takes. A
/// time before the epoch, which no source here has offered, reads as the
/// epoch itself.
fn epoch_stamp(t: SystemTime) -> String {
    let since = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    format!("{}.{:09}", since.as_secs(), since.subsec_nanos())
}

/// Which weak identity signals a caller has asked to *bind*, and whether a
/// source that changes under an in-flight read is an error.
///
/// **The default is today's stance**: the weak signals are advisory *between*
/// runs — data moves, and a dump copied or touched is a different weak
/// identity holding the same bytes — while a source that changes *during* a
/// run is an error, which is [`SourceWatch`]. What still refuses either way is
/// the stored size (`docs/design/decisions.md`, "D20"), which is not a weak
/// signal and is not selectable.
///
/// It rides on [`CacheMode`] beside the path because the library owns the
/// refusal: an embedder gets the same comparison without re-implementing it,
/// and `pgdt`'s `--strict-identity` only sets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StrictIdentity {
    time: bool,
    location: bool,
    in_flight: bool,
}

impl StrictIdentity {
    /// The default: nothing weak binds between runs, and a source that
    /// changes under an in-flight read is still an error.
    pub const ADVISORY: Self = Self { time: false, location: false, in_flight: true };
    /// Nothing binds at all, the in-flight check included — `pgdt
    /// --strict-identity=none`. The only way to turn that check off, because
    /// it is not a question of whether to trust a weak signal but of whether
    /// a read whose bytes changed underneath it may be believed.
    pub const NONE: Self = Self { time: false, location: false, in_flight: false };

    /// Bind the selected terms. The in-flight check is on: it is not one of
    /// the selectors.
    pub const fn binding(time: bool, location: bool) -> Self {
        Self { time, location, in_flight: true }
    }

    /// Whether the modification signals bind — the local mtime, and remotely
    /// `Last-Modified` and the ETag. Under it, **absence is a failure**: a
    /// source that can offer no such signal cannot give the guarantee that
    /// was asked for.
    pub fn time(self) -> bool {
        self.time
    }

    /// Whether the origin a cache was written for binds
    /// ([`OriginMatch`]). A local cache records no origin, so two local runs
    /// always agree and this binds nothing there.
    pub fn location(self) -> bool {
        self.location
    }

    /// Whether a source that changes under an in-flight read aborts the run
    /// rather than merely warning ([`SourceWatch`]) — and so whether a source
    /// nothing can check during a run is refused before it reads
    /// ([`SourceWatch::open`]).
    pub fn in_flight(self) -> bool {
        self.in_flight
    }
}

impl Default for StrictIdentity {
    fn default() -> Self {
        Self::ADVISORY
    }
}

/// A selection as every front end writes it — `time`, `location`, both
/// comma-separated, `advisory` or `none` — so `pgdt --strict-identity` and the
/// DataFusion provider's option read one grammar. `advisory` is
/// [`StrictIdentity::ADVISORY`] and `none` [`StrictIdentity::NONE`], and each
/// is exclusive: it says what every term is, so naming it beside another is a
/// contradiction rather than an override.
///
/// **The default has a word** because a dump's own selection replaces its
/// session's rather than adding to it: without one, a dump under a stricter
/// session loosens only to `none`, off its in-flight check. *Rejected: a word
/// only where a dump overrides a session*, which would split this grammar
/// between front ends; *`default`*, which in a per-dump value reads as the
/// session's.
impl std::str::FromStr for StrictIdentity {
    type Err = String;

    fn from_str(text: &str) -> std::result::Result<Self, String> {
        const TERMS: &str = "`time`, `location`, `advisory` and `none`";
        let mut time = false;
        let mut location = false;
        let mut whole = None;
        for term in text.split(',') {
            match term.trim() {
                "time" => time = true,
                "location" => location = true,
                word @ ("advisory" | "none") => {
                    if whole.replace(word).is_some_and(|earlier| earlier != word) {
                        return Err("`advisory` and `none` each say what every term is, so they \
                                    cannot be combined"
                            .into());
                    }
                }
                "" => return Err(format!("an empty term; write one of {TERMS}")),
                other => return Err(format!("`{other}` is not one of {TERMS}")),
            }
        }
        match (whole, time || location) {
            (Some(word), true) => {
                Err(format!("`{word}` says what every term is, so it cannot be combined with one"))
            }
            (Some("none"), false) => Ok(Self::NONE),
            (Some(_), false) => Ok(Self::ADVISORY),
            (None, _) => Ok(Self::binding(time, location)),
        }
    }
}

/// The selection as [`FromStr`](std::str::FromStr) reads it, so what a front
/// end shows back is what it would take.
impl std::fmt::Display for StrictIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match (self.in_flight, self.time, self.location) {
            (false, ..) => "none",
            (true, true, true) => "time,location",
            (true, true, false) => "time",
            (true, false, true) => "location",
            (true, false, false) => "advisory",
        })
    }
}

/// The source's identity as a run first observed it, re-read wherever asking
/// is cheap, so that bytes changing underneath a read that has already
/// returned some of them stop the run instead of producing a map or a row set
/// mixed from two versions of the file.
///
/// **It reads the source, not the path.** The comparison is made through the
/// `ByteRangeSource` this run is already holding, which for a local file is an
/// `fstat` on the descriptor it opened: that detects the dangerous case — the
/// file modified in place, truncated or rewritten — and ignores the harmless
/// one, a file replaced by rename, where the descriptor goes on reading the
/// intact inode. It is deliberately *not* [`crate::Origin`]'s probe, which is
/// taken before the run and re-opens the path.
///
/// **The cadence follows the cost of asking.** Locally the check rides the
/// cache save, which is already throttled (`docs/design/decisions.md`, "D62"),
/// so nothing is added to the read loop; a run that never saves — a disabled
/// cache, or a warm query answered wholly from the cache — is checked as its
/// reading ends: once, or once per sub-stream of a partitioned replay. Writing the rule as uniform would be false.
/// **A run that fails is checked before its failure is reported**
/// ([`Self::attribute`]), since that failure is often the change itself, and
/// **one that ends has its source compare the checks it still owes first**
/// ([`Self::finish`]).
#[derive(Debug)]
pub struct SourceWatch {
    baseline: SourceIdentity,
    strict: StrictIdentity,
}

impl SourceWatch {
    /// Observe `source`'s identity as this run's baseline, before it has read
    /// anything it would have to distrust.
    /// **The source is told whether the in-flight identity binds**, because on
    /// a provider that can have the server do the comparing the check is a
    /// precondition on every request rather than this watch's own re-read, and
    /// `--strict-identity=none` has to reach both
    /// (`docs/design/decisions.md`, "D21",
    /// [`ByteRangeSource::hint_in_flight_identity`]).
    ///
    /// **A binding run over a source nothing can check is refused here**,
    /// before it reads ([`Error::SourceUncheckable`],
    /// [`ByteRangeSource::in_flight_unchecked`]); under `NONE` it opens as any
    /// other, there being no check to turn off.
    pub async fn open(source: &dyn ByteRangeSource, strict: StrictIdentity) -> Result<Self> {
        source.hint_in_flight_identity(strict.in_flight());
        if strict.in_flight()
            && let Some(why) = source.in_flight_unchecked()
        {
            return Err(Error::SourceUncheckable { why });
        }
        Ok(Self { baseline: SourceIdentity::observe(source).await?, strict })
    }

    /// Re-observe `source` and compare. A change is
    /// [`Error::SourceChangedWhileRead`] unless [`StrictIdentity::NONE`] was
    /// asked for, where it is a warning on the status channel instead.
    pub async fn check(&self, source: &dyn ByteRangeSource) -> Result<()> {
        let live = SourceIdentity::observe(source).await?;
        if live == self.baseline {
            return Ok(());
        }
        let differences = self.baseline.differences(&live);
        if self.strict.in_flight() {
            return Err(Error::SourceChangedWhileRead { differences });
        }
        tracing::warn!(differences, "source changed while it was being read");
        Ok(())
    }

    /// The run's last word before it reports success: the source compares
    /// every check its handed-out bytes still owe
    /// ([`ByteRangeSource::complete_reads`]), then [`Self::check`]. A check
    /// that fails is attributed like any failure ([`Self::attribute`]), a
    /// rewritten block failing its own check being as often the change as a
    /// short read is.
    pub async fn finish(&self, source: &dyn ByteRangeSource) -> Result<()> {
        let completed = source.complete_reads().await;
        self.attribute(source, completed).await?;
        self.check(source).await
    }

    /// `result`, re-checked where it failed. A read or a decode is often the
    /// first thing to meet a file changed underneath the run — a truncation
    /// as a short read, a rewrite as an `.xz` error, invalid UTF-8 or a row
    /// that does not parse — and its own error names the symptom, not the
    /// cause. So a change [`Self::check`] finds is the error instead; where it
    /// finds none, where `StrictIdentity::NONE` makes it a warning, or where
    /// the re-check itself cannot be made, the failure is reported as it was
    /// (`docs/design/decisions.md`, "D21").
    pub async fn attribute<T>(&self, source: &dyn ByteRangeSource, result: Result<T>) -> Result<T> {
        let failure = match result {
            Err(failure) if !matches!(failure, Error::SourceChangedWhileRead { .. }) => failure,
            answered => return answered,
        };
        match self.check(source).await {
            Err(changed @ Error::SourceChangedWhileRead { .. }) => Err(changed),
            _ => Err(failure),
        }
    }
}

/// What produced the indexed blocks' byte offsets. Plain-format offsets are
/// raw file positions; a future archive format's would be entry-relative, so
/// the two must never be silently conflated.
///
/// A compressed source's indexed offsets are *also* plain-format offsets —
/// byte for byte what a plain scan of the same decompressed content
/// produces — so a compressed source never sets this to anything but
/// `Plain`; what changes is [`CompressionIndex`], a sibling field
/// (`docs/design/decisions.md`, "D21").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum ContainerKind {
    Plain,
}

/// What compression sits between this cache's plain-format offsets and the
/// bytes on disk — `None` for a source that needs no such index
/// (`docs/design/decisions.md`, "D21"). A sibling of
/// [`ContainerKind`], not a value of it: see that type's docs.
///
/// Built at [`save`] time from [`ByteRangeSource::seek_table`], which
/// [`crate::XzSource`] answers from the table it already built while opening,
/// so persisting a cache never re-walks the file; read back by [`claim`]
/// before any source exists, which spares every later command that walk. A
/// future gzip index is a different *shape* and gets its own sibling variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum CompressionIndex {
    Xz(xz_seek::SeekTable),
}

/// The shape of the container a cache's offsets sit under, read off the
/// persisted [`CompressionIndex`].
///
/// Three numbers, each answering a question the user is otherwise sent to
/// `xz --list` for. `max_block_uncompressed` is the largest term of what a
/// memory budget is compared against — four times over, one reader's block
/// beside the further blocks the pool keeps, plus the chunk buffer and the
/// decoder's own working memory — so it is most of the read-buffer budget
/// `--memory` has to leave when a query says the block path was declined, and
/// that query's own note states the whole of it
/// (`crate::PlanNoteKind::CompressedBlockPathDeclined`); `blocks` is how much
/// seeking the file offers at all; `streams` is what explains a slow first
/// command, a concatenated file costing one seek per stream to walk
/// (`docs/design/decisions.md`, "D18").
///
/// **Derived, not stored** — a projection of the seek table the envelope
/// already carries, computed on load like the diagnostics beside it. It is a
/// property of the *file*, which is why a cache answers it with no dump
/// present, unlike a budget decline, which depends on the run and reaches a
/// caller as `crate::PlanNoteKind::CompressedBlockPathDeclined`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CompressionShape {
    /// The container: `"xz"` today, and the only value there is. A word
    /// rather than a one-variant enum, being a name to print.
    pub container: &'static str,
    pub streams: usize,
    pub blocks: usize,
    pub max_block_uncompressed: u64,
}

impl CompressionShape {
    fn of(index: &CompressionIndex) -> Self {
        let CompressionIndex::Xz(table) = index;
        Self {
            container: "xz",
            streams: table.stream_count(),
            blocks: table.block_count(),
            max_block_uncompressed: table.max_block_uncompressed(),
        }
    }
}

/// What a cache file holds after [`CACHE_MAGIC`] and its version.
/// [`CacheFileRef`] is what a save encodes, the same fields in this order, so
/// the two change together.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheFile {
    container_kind: ContainerKind,
    compression: Option<CompressionIndex>,
    identity: SourceIdentity,
    /// The addressable (decompressed, for a compressed source) length —
    /// [`ByteRangeSource::size`] as observed at save time. Its own field, not
    /// an alias of `identity`'s stored size (`docs/design/decisions.md`,
    /// "D21"), and the number
    /// [`CacheStatus::Valid`]/[`CacheStatus::Incomplete`] hand out as
    /// `total_size` for the coverage arithmetic.
    total_size: u64,
    /// The last day of the calendar the index's unrepresentable counts were
    /// taken under ([`crate::index::calendar_end`]): a build whose calendar
    /// ends elsewhere refuses a cache holding one
    /// ([`Unusable::CalendarChanged`]; `docs/design/decisions.md`, "D96").
    calendar_end: i32,
    index: DumpIndex,
}

/// Everything a cache file persists beside its index and `total_size`, as it
/// was read — handed out on [`CacheStatus::Valid`]/[`CacheStatus::Incomplete`]
/// so a reporting caller can export the whole file, which `pgdt info --json`
/// does (`docs/design/decisions.md`, "D67").
///
/// **Opaque to Rust, whole to serde**: the fields stay private, so no caller
/// matches on [`SourceIdentity`] or reads the seek table through this, and
/// its `Serialize` is the persisted types' own, variant tags included. The
/// seek table serializes as `seek_table`, the name `compression` being
/// [`CompressionShape`]'s wherever the two sit side by side. Moved out of the
/// decoded file rather than copied, so carrying it costs nothing the load did
/// not already hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CacheEnvelope {
    format_version: u32,
    container_kind: ContainerKind,
    seek_table: Option<CompressionIndex>,
    identity: SourceIdentity,
    calendar_end: i32,
}

/// What a load says about a cache's identity without refusing it: a
/// modification signal that moved, an origin that is not this run's, or both.
///
/// **Recomputed on every load and never persisted**, exactly as the coverage
/// diagnostics are (see the module docs). A caller a term it binds fails is
/// refused by [`CacheMode::strict_identity_refusal`] instead and never reaches
/// here; one whose bound terms hold still hears of those it left unbound.
///
/// Public for that method's reason, and it is the same caller: one that
/// *reports* what a cache holds reads the full [`CacheStatus`] through
/// [`load`] and so never passes through [`CacheMode::load`], where both of
/// these are pushed. Writing the routing out a second time there is what this
/// exists to prevent.
pub fn advisory_identity_diagnostics(
    weak: &WeakIdentity,
    origin: &OriginMatch,
) -> impl Iterator<Item = Diagnostic> {
    let moved = match weak {
        WeakIdentity::Agrees | WeakIdentity::Absent { .. } | WeakIdentity::Unconfirmed { .. } => {
            None
        }
        WeakIdentity::Differs { .. } => Some(Diagnostic::cache_mtime_changed()),
        WeakIdentity::TagDiffers { .. } => Some(Diagnostic::cache_entity_tag_changed()),
    };
    let elsewhere = !matches!(origin, OriginMatch::Agrees);
    moved.into_iter().chain(elsewhere.then(Diagnostic::cache_origin_changed))
}

/// The default cache location when no explicit path is given:
/// `<dump-path>.dtcache`.
pub fn colocated_path(dump_path: &Path) -> PathBuf {
    let mut path = dump_path.as_os_str().to_owned();
    path.push(".dtcache");
    PathBuf::from(path)
}

/// What [`load`] found at a cache path, once checked against the live
/// source's identity — see the module docs.
///
/// Each unusable outcome is named and stays named all the way to the caller:
/// [`CacheMode::load`] carries it across as [`CacheLoad::Unusable`] rather
/// than answering `None` (`docs/design/decisions.md`, "D22").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheStatus {
    /// Nothing at this path.
    Missing,
    /// Something is at this path that cannot be used, and why.
    Unusable(Unusable),
    /// A usable cache whose `index.scanned_through` reaches the file's
    /// recorded size — the whole file is mapped. `weak` is what the source's
    /// current modification signal says against the one recorded at save time
    /// and `origin` what its origin says against the recorded one, neither of
    /// which itself makes the cache unusable (see the module docs).
    /// `total_size` is [`CacheFile::total_size`]: a reporting caller
    /// states coverage against it, and a cache-only caller has no live source
    /// to stat. `compression` is the container's shape where one sits under
    /// these offsets and `None` for a plain file — read off the persisted
    /// seek table, so a cache-only caller answers it with no dump present
    /// ([`CompressionShape`]). `envelope` is the rest of what the file
    /// persists, for a caller exporting it ([`CacheEnvelope`]).
    Valid {
        index: DumpIndex,
        weak: WeakIdentity,
        origin: OriginMatch,
        total_size: u64,
        compression: Option<CompressionShape>,
        envelope: CacheEnvelope,
    },
    /// A usable cache whose `index.scanned_through` falls short of
    /// `total_size` — a real, not-yet-finished scan (a preamble-only scan, or
    /// a query that stopped once its target settled), not a defect
    /// (`docs/design/decisions.md`, "D22"). What "not enough" means is
    /// caller-specific: `pgdt info` states the coverage
    /// (`docs/design/decisions.md`, "The CLI"), while a caller resuming an
    /// incremental scan (`crate::stream::map_file`,
    /// `crate::stream::table_stream`, `crate::index::preamble_only`) builds
    /// on this partial index, the same as [`Valid`](CacheStatus::Valid).
    /// `total_size`, `compression` and `envelope` are
    /// [`Valid`](CacheStatus::Valid)'s and mean the same thing.
    Incomplete {
        index: DumpIndex,
        weak: WeakIdentity,
        origin: OriginMatch,
        total_size: u64,
        compression: Option<CompressionShape>,
        envelope: CacheEnvelope,
    },
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

/// Load a cache from `path` and validate it against `source`'s current
/// identity. See [`CacheStatus`] and the module docs for what "validate"
/// means. A hard I/O error reading `path` (anything but "not found") still
/// propagates; what the file *holds* is answered as a status.
///
/// **The compression layer is compared as well as the size**: a source opened
/// without the cache's claim — an embedder's, or `pgdt` under
/// `--overwrite-unusable-cache` after recognition refused the claim — would
/// otherwise be handed offsets another file's seek table produced
/// ([`Unusable::CompressionContradicted`]; `docs/design/decisions.md`, "D20").
pub async fn load(path: &Path, source: &dyn ByteRangeSource) -> Result<CacheStatus> {
    let file = match read_cache_file(path)? {
        Ok(file) => file,
        Err(status) => return Ok(status),
    };
    let live = SourceIdentity::observe(source).await?;
    let (cached_stored_size, live_stored_size) = (file.identity.stored_size(), live.stored_size());
    if cached_stored_size != live_stored_size {
        return Ok(CacheStatus::Unusable(Unusable::SourceChanged {
            cached_stored_size,
            live_stored_size,
        }));
    }
    let cached_table = file.compression.as_ref().map(|CompressionIndex::Xz(table)| table);
    if cached_table != source.seek_table().as_ref() {
        return Ok(CacheStatus::Unusable(Unusable::CompressionContradicted));
    }
    let weak = file.identity.weak_against(&live);
    let origin = file.identity.origin_against(&live);
    Ok(status_from_file(file, weak, origin))
}

/// Read and header-check the cache at `path`, shared by [`load`],
/// [`load_offline`] and [`claim`]. `Err(status)` is an outcome that needs no
/// live source to reach; the two that compare against one are [`load`]'s.
///
/// **The header is read before anything is decoded** ([`CACHE_MAGIC`]), so
/// another build's cache is told from foreign bytes whatever its shape; the
/// rest is decoded through a buffered reader, never read whole first
/// (`docs/design/decisions.md`, "D78"). A file ending before its magic
/// ends is not a pgdt cache; one ending after it is [`Unusable::Unreadable`],
/// as any other undecodable content is; an I/O failure reading it is still an
/// error.
fn read_cache_file(path: &Path) -> Result<std::result::Result<CacheFile, CacheStatus>> {
    let unusable = |why| Ok(Err(CacheStatus::Unusable(why)));
    let mut reader = match File::open(path) {
        Ok(file) => BufReader::new(file),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Ok(Err(CacheStatus::Missing));
        }
        Err(e) => return Err(Error::Io(e)),
    };
    let mut magic = [0u8; CACHE_MAGIC.len()];
    match reader.read_exact(&mut magic) {
        Ok(()) if magic == CACHE_MAGIC => {}
        Ok(()) => return unusable(Unusable::NotACache),
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return unusable(Unusable::NotACache),
        Err(e) => return Err(Error::Io(e)),
    }
    let mut version = [0u8; 4];
    match reader.read_exact(&mut version) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::UnexpectedEof => return unusable(Unusable::Unreadable),
        Err(e) => return Err(Error::Io(e)),
    }
    let found = u32::from_le_bytes(version);
    if found != CACHE_FORMAT_VERSION {
        return unusable(Unusable::UnsupportedVersion { found });
    }
    match bincode::serde::decode_from_reader::<CacheFile, _, _>(reader, bincode::config::standard())
    {
        // Counts taken under another calendar are not this build's, and a
        // cache holding none has nothing to be wrong about.
        Ok(file)
            if file.calendar_end != calendar_end()
                && file.index.blocks().any(|block| block.unrepresentable.is_some()) =>
        {
            unusable(Unusable::CalendarChanged {
                counted_under: file.calendar_end,
                build: calendar_end(),
            })
        }
        Ok(file) => Ok(Ok(file)),
        Err(DecodeError::Io { inner, .. }) if inner.kind() != ErrorKind::UnexpectedEof => {
            Err(Error::Io(inner))
        }
        Err(_) => unusable(Unusable::Unreadable),
    }
}

/// What the cache at a path settles about a dump file **before any source
/// exists** — [`claim`]'s answer (`docs/design/decisions.md`, "D18").
///
/// Two outcomes. What a caller does with a cache this early is decide which
/// source to build, except where the cache is unusable, which every scan
/// entry point refuses unless told it may overwrite it
/// ([`CacheMode::refusal`]): opening the source first buys nothing and, for a
/// many-streams `.xz`, costs a stream-footer walk. A missing cache settles
/// [`KnownCompression::Unknown`] and no statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheClaim {
    /// What the cache settles short of a refusal.
    Settles {
        /// The file's compression layer: the table a previous walk produced,
        /// "no compression layer", or nothing known.
        compression: KnownCompression,
        /// The heap the cache's statistics hold once loaded
        /// ([`DumpIndex::statistics_heap_bytes`]), zero where nothing decoded:
        /// what a caller carving workers before its scan loads the cache bills
        /// as held (`docs/design/decisions.md`, "D85").
        statistics_heap_bytes: u64,
    },
    /// The cache cannot be used, for the reason [`load`] would answer a
    /// moment later — a stored size compared against the origin's probe rather
    /// than a source, and never [`Unusable::CompressionContradicted`], which
    /// needs the file's own bytes.
    Unusable(Unusable),
}

/// What the cache at `cache_path` settles about `origin`, answered **before
/// any source exists** (`docs/design/decisions.md`, "D18").
///
/// This is the half of the cache a caller needs *early*: recognition decides
/// which source to build, and for an `.xz` file it either walks the stream
/// footers or is handed the table a previous walk already produced. With no
/// source yet to give [`load`], identity is checked against
/// [`crate::Origin::probe`] — the same stored-size rule, with a differing
/// mtime again too weak to invalidate anything. That comparison lives here
/// rather than at the caller, which keeps the early refusal the same verdict
/// as the late one, and it reads the origin's probe rather than a `stat` so
/// that the verdict is not a statement about local files.
///
/// *Not taken:* stopping the decode short of the index, which comes after
/// `compression` and `identity` in [`CacheFile`]. *Rejected:* a sibling
/// reporting the stored size, which would decode a
/// many-thousand-entry seek table twice on the usable path to spare a walk on
/// the path that is about to fail.
pub async fn claim(cache_path: &Path, origin: &Origin) -> Result<CacheClaim> {
    let file = match read_cache_file(cache_path)? {
        Ok(file) => file,
        Err(CacheStatus::Unusable(why)) => return Ok(CacheClaim::Unusable(why)),
        Err(_) => {
            return Ok(CacheClaim::Settles {
                compression: KnownCompression::Unknown,
                statistics_heap_bytes: 0,
            });
        }
    };
    let stored_size = file.identity.stored_size();
    // The index is decoded only to be sized and dropped, its statistics freed
    // as they were decoded: inside a statistics scope (`crate::instrument`).
    let CacheFile { compression, index, .. } = file;
    let statistics_heap_bytes = index.statistics_heap_bytes();
    drop_attributed(index);
    // An origin that cannot be probed is left to the open that follows,
    // which is where that failure has a sentence to say.
    let Ok(live) = origin.probe().await else {
        return Ok(CacheClaim::Settles {
            compression: KnownCompression::Unknown,
            statistics_heap_bytes,
        });
    };
    if stored_size != live.stored_size() {
        return Ok(CacheClaim::Unusable(Unusable::SourceChanged {
            cached_stored_size: stored_size,
            live_stored_size: live.stored_size(),
        }));
    }
    let compression = match compression {
        Some(CompressionIndex::Xz(table)) => KnownCompression::Xz(table),
        None => KnownCompression::Plain,
    };
    Ok(CacheClaim::Settles { compression, statistics_heap_bytes })
}

/// Drop each block's statistics — the `Arc` sharing them included — inside a
/// statistics scope, as their decode was attributed. `index`'s own
/// allocations are *not* freed in the scope: it is a parameter, so it outlives
/// the guard and is released once this returns, which is right, their decode
/// having been attributed to nobody.
fn drop_attributed(mut index: DumpIndex) {
    let _attributed = StatisticsScope::enter();
    for span in &mut index.spans {
        if let SpanBody::Data(DataBlock::Copy(block)) = &mut span.body {
            block.statistics = None;
        }
    }
}

/// Load a cache from `path` with no live source to check it against — the
/// cache-only counterpart to [`load`] (`docs/design/decisions.md`,
/// "The compressed source and the cache"). With no live source to compare
/// against, `weak` and `origin` are always the agreeing answer here;
/// "unverified, historical" is a
/// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed by
/// [`CacheMode::load_offline`] instead.
pub async fn load_offline(path: &Path) -> Result<CacheStatus> {
    match read_cache_file(path)? {
        Ok(file) => Ok(status_from_file(file, WeakIdentity::Agrees, OriginMatch::Agrees)),
        Err(status) => Ok(status),
    }
}

/// Shared by [`load`] and [`load_offline`] once a `CacheFile` has passed its
/// header and (when live) stored-size and compression checks: `Valid` when the index's
/// own `scanned_through` reaches the file's recorded addressable length,
/// `Incomplete` otherwise, with the index's unpersisted diagnostics
/// recomputed either way. Reads [`CacheFile::total_size`] rather than
/// re-deriving one from a live source: by this point the stored sizes are
/// known equal wherever a live source exists, and `load_offline` has none.
fn status_from_file(file: CacheFile, weak: WeakIdentity, origin: OriginMatch) -> CacheStatus {
    let total_size = file.total_size;
    let mut index = file.index;
    // `DumpIndex::diagnostics` is `#[serde(skip)]`, so a loaded index arrives
    // with none. Both file-level figures are pure functions of the spans and
    // the persisted size, so they are recomputed on load rather than persisted
    // (`crate::diagnostic`, "Not persisted").
    index.diagnostics = tiling_diagnostics(&index.spans, total_size);
    index.diagnostics.push(toc_coverage_diagnostic(&index.spans));
    let table = file.compression.as_ref().map(|CompressionIndex::Xz(table)| table);
    index.diagnostics.extend(non_seekable_compression_diagnostic(table));
    // Derived for the same reason the diagnostics above are: a projection of
    // what the envelope already holds.
    let compression = file.compression.as_ref().map(CompressionShape::of);
    let envelope = CacheEnvelope {
        // Only a file whose header states this build's version is decoded.
        format_version: CACHE_FORMAT_VERSION,
        container_kind: file.container_kind,
        seek_table: file.compression,
        identity: file.identity,
        calendar_end: file.calendar_end,
    };
    if index.is_complete(total_size) {
        CacheStatus::Valid { index, weak, origin, total_size, compression, envelope }
    } else {
        CacheStatus::Incomplete { index, weak, origin, total_size, compression, envelope }
    }
}

/// What [`save`] encodes: [`CacheFile`]'s fields in [`CacheFile`]'s order,
/// the index borrowed rather than cloned, so the bytes are the ones an owned
/// `CacheFile` encodes to and [`read_cache_file`] decodes.
#[derive(Serialize)]
struct CacheFileRef<'a> {
    container_kind: ContainerKind,
    compression: Option<CompressionIndex>,
    identity: SourceIdentity,
    total_size: u64,
    calendar_end: i32,
    index: &'a DumpIndex,
}

/// Write `index` to `path` (colocated or explicit — whichever the caller
/// resolved), replacing any existing cache there, and record `source`'s
/// current stored size/mtime plus its addressable length for [`load`] to
/// check next time. Propagates I/O failures as `Error::Io` rather than
/// swallowing them — see the module docs.
///
/// **This is the writer, not the run's save**: the in-flight identity check a
/// scan makes rides [`CacheMode::save`], which is what every scan entry point
/// calls ([`SourceWatch`]).
///
/// **The index is encoded straight into a file beside `path`, which is then
/// renamed over it** (`docs/design/decisions.md`, "D78"), so no encoded copy
/// of the cache is held in memory, and at every moment `path` holds either
/// the previous save or this one, whole. A save that fails removes its file;
/// one the process is killed during leaves it, named for the cache and ending
/// `.tmp`, and the previous cache untouched. The replacement is a new file: a
/// cache path that was a symbolic link becomes a regular file, and the file
/// takes default permissions rather than the old one's. Nothing is synced, so
/// the guarantee is against a killed process, not a lost machine.
pub async fn save(path: &Path, source: &dyn ByteRangeSource, index: &DumpIndex) -> Result<()> {
    let identity = SourceIdentity::observe(source).await?;
    let total_size = source.size().await?;
    let file = CacheFileRef {
        container_kind: ContainerKind::Plain,
        compression: source.seek_table().map(CompressionIndex::Xz),
        identity,
        total_size,
        calendar_end: calendar_end(),
        index,
    };
    write_beside(path, |writer| {
        writer.write_all(&CACHE_MAGIC)?;
        writer.write_all(&CACHE_FORMAT_VERSION.to_le_bytes())?;
        bincode::serde::encode_into_std_write(&file, writer, bincode::config::standard())
            .map(drop)
            .map_err(|e| match e {
                EncodeError::Io { inner, .. } => Error::Io(inner),
                other => Error::CacheEncode(other),
            })
    })
}

/// Run `encode` into a new file [`beside`] `path`, then rename that file over
/// `path`; on any failure, remove it and leave `path` as it was.
fn write_beside(
    path: &Path,
    encode: impl FnOnce(&mut BufWriter<File>) -> Result<()>,
) -> Result<()> {
    let beside = beside(path);
    let file = OpenOptions::new().write(true).create_new(true).open(&beside)?;
    let written = (|| {
        let mut writer = BufWriter::new(file);
        encode(&mut writer)?;
        writer.flush()?;
        // Closed before the rename, which some platforms need.
        drop(writer);
        std::fs::rename(&beside, path)?;
        Ok(())
    })();
    if written.is_err() {
        // Best-effort: the error being returned is the one worth reporting.
        let _ = std::fs::remove_file(&beside);
    }
    written
}

/// The file a save writes before renaming it over `path`: in `path`'s own
/// directory, so the rename never crosses a filesystem, and named
/// `<cache file name>.<pid>-<n>.tmp`, unique to this process and this save,
/// so two saves of one cache — two processes, or two tasks of one — never
/// write into one file.
fn beside(path: &Path) -> PathBuf {
    static SAVES: AtomicU64 = AtomicU64::new(0);
    let mut name: OsString = path.file_name().map(ToOwned::to_owned).unwrap_or_default();
    name.push(format!(".{}-{}.tmp", std::process::id(), SAVES.fetch_add(1, Ordering::Relaxed)));
    path.with_file_name(name)
}

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
    /// (`crate::stream::map_file`, `crate::stream::map_for_query`, which
    /// serves both `crate::table_stream` and `crate::table_stream_partitions`,
    /// and `crate::index::preamble_only`) builds forward from whatever partial
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

    /// What a scan entry point does about a cache it cannot use: the
    /// refusal, or `None` where this mode may start cold and replace it at
    /// its first save (`docs/design/decisions.md`, "D20"). One decision for
    /// every caller — the four scan entry points on [`CacheLoad::Unusable`],
    /// and one pre-empting them on [`CacheClaim::Unusable`] or on recognition
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
    /// [`crate::diagnostic::DiagnosticKind::CacheOffline`] pushed onto it
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

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::{LocalFileSource, ScanOptions, StatisticsRequest, XzSource, map_file};

    /// Real `pg_dump` output whose columns gather bounds and dictionaries.
    fn statistics_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/16/statistics/default.sql")
    }

    /// Every name in `dir`, sorted.
    fn listing(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// Streaming changed where the bytes go, not what they are: a save writes
    /// exactly what the owned [`CacheFile`] encodes to in one buffer, for a
    /// plain source and for a compressed one carrying its seek table, with
    /// statistics gathered — and reads back through the reader as the index it
    /// wrote. The same file cut short reads as [`Unusable::Unreadable`] —
    /// ours, so overwritable — never as an I/O error, and cut inside its magic
    /// as not a pgdt cache.
    #[tokio::test]
    async fn a_save_writes_the_bytes_the_whole_file_encodes_to() {
        let dir = tempfile::tempdir().unwrap();
        let compressed = dir.path().join("default.sql.xz");
        let out = Command::new("xz")
            .args(["--block-size=65536", "-c"])
            .arg(statistics_fixture())
            .output()
            .expect("`xz` is not runnable, so this test cannot build its fixture; install it.");
        assert!(out.status.success(), "xz failed: {}", String::from_utf8_lossy(&out.stderr));
        std::fs::write(&compressed, &out.stdout).unwrap();

        let plain = LocalFileSource::open(statistics_fixture()).unwrap();
        let xz = XzSource::open(&compressed).unwrap();
        let sources: [(&str, &dyn ByteRangeSource); 2] = [("plain", &plain), ("xz", &xz)];
        for (name, source) in sources {
            let run = map_file(
                source,
                &ScanOptions::default(),
                &CacheMode::DISABLED,
                &StatisticsRequest::default(),
            )
            .await
            .unwrap();
            assert!(
                run.index.spans.iter().any(|span| matches!(
                    &span.body,
                    SpanBody::Data(DataBlock::Copy(block)) if block.statistics.as_ref().is_some_and(
                        |statistics| statistics.columns.iter().flatten().any(|column| {
                            column.bounds.is_some() && column.dictionary.is_some()
                        })
                    )
                )),
                "{name}: the encoding under test must carry statistics"
            );
            assert_eq!(source.seek_table().is_some(), name == "xz");

            let path = dir.path().join(format!("{name}.dtcache"));
            save(&path, source, &run.index).await.unwrap();
            let whole = CacheFile {
                container_kind: ContainerKind::Plain,
                compression: source.seek_table().map(CompressionIndex::Xz),
                identity: SourceIdentity::observe(source).await.unwrap(),
                total_size: source.size().await.unwrap(),
                calendar_end: calendar_end(),
                index: run.index.clone(),
            };
            let bytes = std::fs::read(&path).unwrap();
            let header = [&CACHE_MAGIC[..], &CACHE_FORMAT_VERSION.to_le_bytes()].concat();
            assert_eq!(
                bytes,
                [
                    header.clone(),
                    bincode::serde::encode_to_vec(&whole, bincode::config::standard()).unwrap()
                ]
                .concat(),
                "{name}: the streamed save's bytes"
            );
            let Ok(Ok(read)) = read_cache_file(&path) else {
                panic!("{name}: the streamed save must read back");
            };
            // Diagnostics are not persisted, so the index is compared by its
            // encoding: what reads back encodes to what was written.
            assert_eq!(
                [
                    header,
                    bincode::serde::encode_to_vec(&read, bincode::config::standard()).unwrap()
                ]
                .concat(),
                bytes,
                "{name}: the file read back"
            );

            std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
            assert!(
                matches!(
                    read_cache_file(&path),
                    Ok(Err(CacheStatus::Unusable(Unusable::Unreadable)))
                ),
                "{name}: a cache cut short is unreadable, not an I/O failure"
            );
            std::fs::write(&path, &bytes[..CACHE_MAGIC.len() - 1]).unwrap();
            assert!(
                matches!(
                    read_cache_file(&path),
                    Ok(Err(CacheStatus::Unusable(Unusable::NotACache)))
                ),
                "{name}: a file cut inside the magic never said it was a pgdt cache"
            );
        }
    }

    /// **A cache counted under another calendar is refused, and one holding
    /// no count is not**: counts past a calendar's end are the calendar's, so
    /// a build whose calendar ends elsewhere reads none of them as current,
    /// while a map at the metadata level counted nothing to be wrong about
    /// (`docs/design/decisions.md`, "D96"). The refusal is ours, so a `parse`
    /// told it may overwrite the cache replaces it.
    #[tokio::test]
    async fn a_cache_counted_under_another_calendar_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let dump = dir.path().join("dump.sql");
        std::fs::copy(statistics_fixture(), &dump).unwrap();
        let source = LocalFileSource::open(&dump).unwrap();
        let mode = CacheMode::enabled(dir.path().join("scratch.dtcache"));
        let options = ScanOptions::default();
        let data = map_file(&source, &options, &mode, &StatisticsRequest::DATA).await.unwrap();
        let metadata =
            map_file(&source, &options, &CacheMode::DISABLED, &StatisticsRequest::METADATA)
                .await
                .unwrap();

        let other = calendar_end() - 1;
        for (name, index, refused) in
            [("data", &data.index, true), ("metadata", &metadata.index, false)]
        {
            let path = dir.path().join(format!("{name}.dtcache"));
            let file = CacheFileRef {
                container_kind: ContainerKind::Plain,
                compression: None,
                identity: SourceIdentity::observe(&source).await.unwrap(),
                total_size: source.size().await.unwrap(),
                calendar_end: other,
                index,
            };
            write_beside(&path, |writer| {
                writer.write_all(&CACHE_MAGIC)?;
                writer.write_all(&CACHE_FORMAT_VERSION.to_le_bytes())?;
                bincode::serde::encode_into_std_write(&file, writer, bincode::config::standard())
                    .map(drop)
                    .map_err(Error::CacheEncode)
            })
            .unwrap();
            let status = load(&path, &source).await.unwrap();
            if refused {
                let why = Unusable::CalendarChanged { counted_under: other, build: calendar_end() };
                assert_eq!(status, CacheStatus::Unusable(why.clone()), "{name}");
                assert!(why.overwritable());
                let message =
                    Error::CacheUnusable { path: path.clone(), unusable: why }.to_string();
                assert!(
                    message.contains("262142-12-30") && message.contains("262142-12-31"),
                    "{message}"
                );
            } else {
                assert!(matches!(status, CacheStatus::Valid { .. }), "{name}: {status:?}");
            }
        }
    }

    /// A kill mid-save leaves the previous cache whole. No test races a
    /// signal (`docs/design/decisions.md`, "D73"), so the save is stopped at
    /// the moment a kill would land — partway through its encoding — and what
    /// it leaves on disk is read there: the previous cache, untouched, and the
    /// partial file beside it. A save that fails removes that file, and one
    /// that finishes replaces the cache with nothing left beside it.
    #[test]
    fn a_save_leaves_the_previous_cache_whole_until_it_renames() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dump.sql.dtcache");
        std::fs::write(&path, b"the previous cache").unwrap();

        let failed = write_beside(&path, |writer| {
            writer.write_all(b"the first half of the next")?;
            writer.flush()?;
            assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
            let names = listing(dir.path());
            assert_eq!(names.len(), 2, "the cache and the save's own file: {names:?}");
            let partial = names.iter().find(|name| *name != "dump.sql.dtcache").unwrap();
            assert!(
                partial.starts_with("dump.sql.dtcache.") && partial.ends_with(".tmp"),
                "the partial file is named for its cache: {partial}"
            );
            assert_eq!(
                std::fs::read(dir.path().join(partial)).unwrap(),
                b"the first half of the next"
            );
            Err(Error::CacheModeMismatch("the save stops here"))
        });
        assert!(matches!(failed, Err(Error::CacheModeMismatch(_))));
        assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
        assert_eq!(listing(dir.path()), ["dump.sql.dtcache"]);

        write_beside(&path, |writer| {
            writer.write_all(b"the next cache")?;
            assert_eq!(std::fs::read(&path).unwrap(), b"the previous cache");
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"the next cache");
        assert_eq!(listing(dir.path()), ["dump.sql.dtcache"]);
    }
}
