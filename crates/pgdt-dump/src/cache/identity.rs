//! What a cache records of the dump file it was saved from, and how that
//! record compares with a live source's — see the parent module's docs. Not
//! the cache's API: the persisted record is reached through `internal`, for
//! the tests that write a cache by hand.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{OriginMatch, WeakIdentity};
use pgdt_core::Result;
use pgdt_io::{ByteRangeSource, internal::is_weak_tag};

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
