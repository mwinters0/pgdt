//! Every identity a cache compares: what it records of the dump file it was
//! saved from ([`SourceIdentity`], reached through `internal` for the tests
//! that write a cache by hand), what that record says against a live
//! source's ([`WeakIdentity`], [`OriginMatch`]) and the warnings a load
//! carries for them, and the watch a run keeps on its source while it reads
//! ([`SourceWatch`]) — see the parent module's docs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::StrictIdentity;
use pgdt_core::{Error, Result};
use pgdt_io::{ByteRangeSource, internal::is_weak_tag};
use pgdt_model::Diagnostic;

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
    /// ([`pgdt_model::DiagnosticKind::CacheOriginChanged`]) — with
    /// each side's, `None` for a source that records none.
    Differs { cached: Option<String>, live: Option<String> },
}

impl OriginMatch {
    /// Why [`StrictIdentity::location`] is not met, or `None` where it is —
    /// the clause [`Error::StrictIdentityUnmet`] carries, naming the two
    /// origins it compared.
    pub(super) fn unmet(&self) -> Option<String> {
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
    /// ([`pgdt_model::DiagnosticKind::CacheMtimeChanged`]) — with the
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
    pub(super) fn unmet(&self) -> Option<String> {
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

/// What a load says about a cache's identity without refusing it: a
/// modification signal that moved, an origin that is not this run's, or both.
///
/// **Recomputed on every load and never persisted**, exactly as the coverage
/// diagnostics are (see the module docs). A caller a term it binds fails is
/// refused by
/// [`CacheMode::strict_identity_refusal`](super::CacheMode::strict_identity_refusal)
/// instead and never reaches here; one whose bound terms hold still hears of
/// those it left unbound.
///
/// Public for that method's reason, and it is the same caller: one that
/// *reports* what a cache holds reads the full
/// [`CacheStatus`](super::CacheStatus) through [`load`](super::load) and so
/// never passes through [`CacheMode::load`](super::CacheMode::load), where both
/// of these are pushed. Writing the routing out a second time there is what
/// this exists to prevent.
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
/// intact inode. It is deliberately *not* [`pgdt_io::Origin`]'s probe, which is
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
pub enum SourceIdentity {
    /// Backed by [`ByteRangeSource::stored_size`]/`modified` — a source that
    /// was not fetched from anywhere, decompressing or not.
    LocalFile {
        stored_size: u64,
        /// `(seconds, nanoseconds)` since the Unix epoch: the model's own on-disk
        /// vocabulary, `SystemTime` not being `Serialize`. `None` when the
        /// source exposed no mtime.
        mtime: Option<(u64, u32)>,
    },
    /// An object fetched from somewhere, which records two things a file does
    /// not: **where it came from**, which is what
    /// [`StrictIdentity::location`](super::StrictIdentity::location) binds, and
    /// the server's **entity tag**, which is the stronger of the two
    /// modification signals
    /// [`StrictIdentity::time`](super::StrictIdentity::time) binds.
    Remote {
        origin: String,
        etag: Option<String>,
        /// `Last-Modified`, in [`SourceIdentity::LocalFile`]'s on-disk
        /// vocabulary. `None` where the server sent none — which is what a
        /// substituted epoch is read as (`pgdt_io::io`'s `weak_identity`).
        last_modified: Option<(u64, u32)>,
        stored_size: u64,
    },
}

impl SourceIdentity {
    pub async fn observe(source: &dyn ByteRangeSource) -> Result<Self> {
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
    pub(super) fn stored_size(&self) -> u64 {
        match self {
            SourceIdentity::LocalFile { stored_size, .. }
            | SourceIdentity::Remote { stored_size, .. } => *stored_size,
        }
    }

    /// The modification time this identity carries, as a `SystemTime`: the model's
    /// on-disk `(seconds, nanoseconds)` is backed out at the boundary, so
    /// what leaves this module is what the trait answered and what an
    /// embedder formats.
    pub(super) fn modified(&self) -> Option<SystemTime> {
        let stamp = match self {
            SourceIdentity::LocalFile { mtime, .. }
            | SourceIdentity::Remote { last_modified: mtime, .. } => *mtime,
        };
        stamp.map(|(s, n)| UNIX_EPOCH + Duration::new(s, n))
    }

    /// The server's entity tag, where this identity has one at all.
    pub(super) fn etag(&self) -> Option<&str> {
        match self {
            SourceIdentity::LocalFile { .. } => None,
            SourceIdentity::Remote { etag, .. } => etag.as_deref(),
        }
    }

    /// Where the object was fetched from, or `None` for a source that was not
    /// fetched from anywhere (D87).
    pub(super) fn origin(&self) -> Option<&str> {
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
    /// `Last-Modified` — the reason [`pgdt_io::io`] pins no read by it either —
    /// and decides only where neither side states one. **There it can show a
    /// change and never confirm one**: opaque tags that differ, with either
    /// side's `W/` set aside, are [`WeakIdentity::TagDiffers`], and equal ones
    /// are [`WeakIdentity::Unconfirmed`] — the same reason RFC 9110, §13.1.5
    /// forbids a weak tag in `If-Range`. Rejected: comparing them weakly
    /// (§8.8.3.2) and agreeing, which answers equivalence where the map asks
    /// for bytes; exact equality, `W/` being the tag's strength and not part
    /// of it.
    pub(super) fn weak_against(&self, live: &Self) -> WeakIdentity {
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

    /// What this identity's origin says against `live`'s — the second advisory
    /// answer, and the one
    /// [`StrictIdentity::location`](super::StrictIdentity::location) binds
    /// (`docs/design/decisions.md`, "D87").
    ///
    /// **Two sources that were both fetched from nowhere agree**, which is
    /// what makes `location` inert on a local file rather than a refusal
    /// nobody asked for (D87).
    pub(super) fn origin_against(&self, live: &Self) -> OriginMatch {
        match (self.origin(), live.origin()) {
            (cached, live) if cached == live => OriginMatch::Agrees,
            (cached, live) => OriginMatch::Differs {
                cached: cached.map(ToOwned::to_owned),
                live: live.map(ToOwned::to_owned),
            },
        }
    }

    /// How a run that started from this identity and now sees `live` says what
    /// moved —
    /// [`Error::SourceChangedWhileRead`](pgdt_core::Error::SourceChangedWhileRead)'s
    /// evidence clause. Never called where the two are equal, so it is never
    /// empty.
    pub(super) fn differences(&self, live: &Self) -> String {
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
