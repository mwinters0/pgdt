//! The remote source, over `object_store`'s HTTP backend; what an
//! [`Origin`] needs to name one; and opening a remote origin — recognising
//! its compression, and the seek-table walk an uncached `.xz` pays, announced
//! before it starts. Everything here is behind the `http` feature.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;
use object_store::ObjectStore as _;

use pgdt_core::{Error, Result};

use crate::cancel::Cancellation;

use super::origin::{
    KnownCompression, Location, ORIGIN_LEADING_BYTES, Origin, OriginProbe, Recognized, Verdict,
    over_known_table,
};
use super::xz::{FetchedXzSource, walk_seek_table};
use super::{ByteRangeSource, RemoteIdentity, is_weak_tag};

/// How long a remote request may go without delivering a byte before it is
/// abandoned.
///
/// **It replaces `object_store`'s *total* request timeout**, which limits link
/// speed rather than liveness: that one counts the response body, so a large
/// ranged GET over a slow-but-working link fails while still progressing. A read timeout resets on every byte that arrives, so a slow
/// transfer completes and only a dead connection fails.
///
/// **Unmeasured, and chosen rather than tuned**: a link that has delivered
/// nothing for this long is not a slow link. The phase that tunes the network
/// is where a reading would replace it.
pub const REMOTE_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A URL and the client that reads it — an [`Origin`]'s remote half.
///
/// **The client is built once, here.** The probe and every later read go
/// through this one store, so they share a connection pool and cannot
/// disagree about timeouts or retries.
#[derive(Debug)]
pub(super) struct RemoteObject {
    pub(super) url: url::Url,
    store: object_store::http::HttpStore,
    /// What the probe's response said the object was: kept whole rather than
    /// projected, because the precondition every later request carries is
    /// built from the validators **as the server stated them**
    /// (`docs/design/decisions.md`, "D21"), and a date
    /// round-tripped through a [`SystemTime`] is a second spelling of one of
    /// them. Set by [`RemoteObject::probe`], which [`Origin::probe`] runs at
    /// most once.
    meta: std::sync::OnceLock<object_store::ObjectMeta>,
}

impl RemoteObject {
    /// Build the client for `url`, whose scheme [`Origin::remote`] has already
    /// accepted.
    pub(super) fn open(url: url::Url, read_timeout: Duration) -> Result<Self> {
        // Four client settings, two of them the crate's own: the retry
        // configuration and `http1_only` are left alone, `allow_http` follows
        // the scheme the user typed — which is their statement that plaintext
        // is acceptable — and the timeouts are ours.
        let options = object_store::ClientOptions::new()
            .with_timeout_disabled()
            .with_read_timeout(read_timeout)
            .with_allow_http(url.scheme() == "http");
        let store = object_store::http::HttpBuilder::new()
            .with_url(url.as_str())
            .with_client_options(options)
            .build()
            .map_err(|e| remote_failure(&url, &e))?;
        Ok(Self { url, store, meta: std::sync::OnceLock::new() })
    }

    /// The identity precondition a ranged GET carries, so that an object
    /// rewritten mid-scan comes back as a refusal instead of as bytes from
    /// two versions of the file mixed together
    /// (`docs/design/decisions.md`, "D21").
    ///
    /// **The server does the comparing**, which is why the cadence is every
    /// request here where it is every cache save locally: the check rides on
    /// a request the read was already making (D21).
    ///
    /// The entity tag is preferred, and `If-Unmodified-Since` is the fallback
    /// for a server that sends none **or only a weak one**: `If-Match`
    /// compares strongly (RFC 9110, §13.1.1), so a weak tag is refused by a
    /// server that follows the RFC however little has changed, and matched by
    /// one that does not — a pin whose answer is the server's reading rather
    /// than the object's. A `Last-Modified` this build reads as
    /// absence is not sent — `object_store` substitutes the epoch for a
    /// response that carries none (`docs/design/runtime-invariants.md`,
    /// "RT16"), and asking a server to confirm that nothing has changed since
    /// 1970 refuses every read. With neither usable the read goes unpinned,
    /// which a run binding the in-flight identity refuses before it reads
    /// ([`RemoteObject::unpinned`]).
    fn precondition(&self) -> object_store::GetOptions {
        let mut options = object_store::GetOptions::default();
        let Some(meta) = self.meta.get() else { return options };
        match meta.e_tag.as_deref().filter(|tag| !is_weak_tag(tag)) {
            Some(tag) => options.if_match = Some(tag.to_string()),
            None => {
                let stated = weak_identity(
                    meta.last_modified.timestamp(),
                    meta.last_modified.timestamp_subsec_nanos(),
                );
                if stated.is_some() {
                    options.if_unmodified_since = Some(meta.last_modified);
                }
            }
        }
        options
    }

    /// Why [`RemoteObject::precondition`] pins nothing, or `None` where it
    /// pins a read — read off the precondition itself, so the rule stays
    /// stated once. A source over this object freezes its size and
    /// modification time at the probe, so where nothing is pinned nothing in
    /// the run can see a rewrite ([`ByteRangeSource::in_flight_unchecked`]).
    fn unpinned(&self) -> Option<String> {
        let pinned = self.precondition();
        if pinned.if_match.is_some() || pinned.if_unmodified_since.is_some() {
            return None;
        }
        let tag = self.meta.get().and_then(|meta| meta.e_tag.as_deref());
        Some(match tag {
            Some(_) => "the server states only a weak entity tag and no `Last-Modified`, so no \
                        read can be pinned to the version this run opened on"
                .to_string(),
            None => "the server states neither an entity tag nor a `Last-Modified`, so no read \
                     can be pinned to the version this run opened on"
                .to_string(),
        })
    }

    /// The object a request addresses: the store's own base URL with nothing
    /// appended, since the store was built from the whole URL. That is the
    /// form that **preserves a query string**, which is what lets a presigned
    /// URL work with no credential handling of our own.
    fn object(&self) -> object_store::path::Path {
        object_store::path::Path::default()
    }

    /// One ranged GET of the leading bytes, answering all three of the probe's
    /// questions: a 206 carries the object's **total** size in `Content-Range`
    /// and the response carries the validators beside it
    /// (`docs/design/runtime-invariants.md`, "RT13"). So the probe is one
    /// round trip, not a `HEAD` and a `GET`.
    pub(super) async fn probe(&self) -> Result<OriginProbe> {
        let options = object_store::GetOptions {
            range: Some(object_store::GetRange::Bounded(0..ORIGIN_LEADING_BYTES as u64)),
            ..Default::default()
        };
        let got = self
            .store
            .get_opts(&self.object(), options)
            .await
            .map_err(|e| remote_failure(&self.url, &e))?;
        let stored_size = got.meta.size;
        let modified = weak_identity(
            got.meta.last_modified.timestamp(),
            got.meta.last_modified.timestamp_subsec_nanos(),
        );
        // Kept before the body is drained, so a probe that fails to read its
        // own bytes leaves no validators for a later request to pin against.
        let meta = got.meta.clone();
        let leading = got.bytes().await.map_err(|e| remote_failure(&self.url, &e))?;
        let _ = self.meta.set(meta);
        Ok(OriginProbe { stored_size, modified, leading: leading.to_vec() })
    }
}

/// What a server's `Last-Modified` is worth as a weak identity: the time it
/// sent, or `None` where it sent none.
///
/// **The Unix epoch reads as absence.** `object_store` substitutes the epoch
/// where a response carries no `Last-Modified`, so "the server said nothing"
/// and "the server said 1970-01-01" arrive identically
/// (`docs/design/runtime-invariants.md`, "RT16"); no dump was modified in
/// 1970, so reading it as silence is the honest half of that pair. A time
/// *before* the epoch reads as silence too, for the same reason and with no
/// second spelling to distinguish.
fn weak_identity(seconds: i64, nanos: u32) -> Option<SystemTime> {
    if seconds == 0 && nanos == 0 {
        return None;
    }
    u64::try_from(seconds).ok().map(|s| UNIX_EPOCH + Duration::new(s, nanos))
}

/// A URL's last path segment, or `None` where it has none — a bare host, or a
/// path ending in `/`. Left exactly as the URL spells it, percent-escapes
/// included: the point of the derived cache name is that a user can predict it
/// by reading the URL (`docs/design/decisions.md`, "D87"), and a
/// decoded segment can hold a path separator where the written one cannot.
pub(super) fn remote_name_of(url: &url::Url) -> Option<&str> {
    url.path_segments()?.next_back().filter(|segment| !segment.is_empty())
}

/// A URL as a message may print it: with any username and password taken out.
///
/// The refusal of a credential-carrying URL is the one place a password could
/// reach a terminal or a redirected log, and echoing one back at a user who
/// has just been told it will not be sent is the wrong way to say it.
pub(super) fn without_credentials(url: &url::Url) -> String {
    let mut shown = url.clone();
    // Both setters fail only on a URL that cannot have a host, which one with
    // an `http` scheme always can. A failure leaves the field as it was, so
    // the fallback below is the redaction failing safe rather than silently.
    if shown.set_username("").is_err() || shown.set_password(None).is_err() {
        return format!("{}://{}", url.scheme(), url.host_str().unwrap_or("(no host)"));
    }
    shown.to_string()
}

/// `object_store`'s wording, named against the URL it is about rather than
/// surfaced raw (`docs/design/decisions.md`, "D62").
fn remote_failure(url: &url::Url, error: &object_store::Error) -> Error {
    Error::Remote { url: url.to_string(), message: error.to_string() }
}

/// A dump an HTTP server answers ranged GETs for.
///
/// **Almost every advisory trait member is left at its default, and that is
/// the decision rather than an omission**
/// (`docs/design/decisions.md`, "D2"): one worker, no
/// partitioning advice, no memory recommendation, no read-size hint — this
/// source recycles no buffer, so there is nothing for one to size — and an
/// exact size, which is also the stored size. Each is the conservative answer,
/// and the phase that tunes the network is where a reading would replace one.
///
/// **Size and weak identity come from the origin's probe**, so neither costs a
/// round trip and the identity a run opens on is the one the cache claim was
/// settled against.
#[derive(Debug)]
pub struct RemoteSource {
    object: Arc<RemoteObject>,
    size: u64,
    modified: Option<SystemTime>,
    /// What the server called this version of the object at the probe — the
    /// half of a remote cache's identity a file has no equivalent of
    /// ([`ByteRangeSource::remote_identity`]).
    etag: Option<String>,
    /// The caller's ask that reading stop, where one has been announced
    /// ([`ByteRangeSource::hint_cancellation`]). Behind a lock because a hint
    /// arrives through `&self`, as every other hint does.
    cancel: Mutex<Option<Arc<Cancellation>>>,
    /// Whether every ranged GET pins the object to the version the probe saw
    /// ([`ByteRangeSource::hint_in_flight_identity`]). **It starts bound**:
    /// the hint arrives once a run has opened its watch, and a read taken
    /// before then must already be pinned.
    precondition: AtomicBool,
}

impl RemoteSource {
    fn new(object: Arc<RemoteObject>, probe: &OriginProbe) -> Self {
        let etag = object.meta.get().and_then(|meta| meta.e_tag.clone());
        Self {
            object,
            size: probe.stored_size(),
            modified: probe.modified(),
            etag,
            cancel: Mutex::new(None),
            precondition: AtomicBool::new(true),
        }
    }

    /// The URL this source reads, which is how a message names it.
    pub fn url(&self) -> &url::Url {
        &self.object.url
    }

    /// What a failed request means, named against the URL it is about.
    ///
    /// **A refused precondition is the one failure that is not about the
    /// network**: the server has just said the object is no longer the one
    /// this run opened on, which is exactly what a local `fstat` finds and is
    /// reported in the same words, remedies included
    /// (`docs/design/decisions.md`, "D21"). Everything
    /// else is [`Error::Remote`].
    fn failed(&self, error: &object_store::Error) -> Error {
        match error {
            object_store::Error::Precondition { .. } => Error::SourceChangedWhileRead {
                differences: "the server refused a read of the version this run opened on"
                    .to_string(),
            },
            other => remote_failure(&self.object.url, other),
        }
    }

    /// One ranged GET, and exactly `len` bytes back.
    ///
    /// **The request pins the object to the version the probe saw**, unless a
    /// caller has said the in-flight identity does not bind, so a rewrite
    /// mid-scan is a refusal on the first read after it rather than bytes from
    /// two files ([`RemoteObject::precondition`]).
    async fn get(&self, offset: u64, len: usize) -> Result<Bytes> {
        let mut options = if self.precondition.load(Ordering::Relaxed) {
            self.object.precondition()
        } else {
            object_store::GetOptions::default()
        };
        options.range = Some(object_store::GetRange::Bounded(offset..offset + len as u64));
        let got = self
            .object
            .store
            .get_opts(&self.object.object(), options)
            .await
            .map_err(|e| self.failed(&e))?;
        let bytes = got.bytes().await.map_err(|e| self.failed(&e))?;
        // A server answering a *prefix* of the range it was asked for is a
        // legal response and a different fault: `object_store` compares the
        // `Content-Range` it got against the one it asked for and refuses
        // before the body is read. What can reach here short is the end of the
        // object, which a caller reading past it hears about in the words a
        // local `read_exact_at` would use.
        if bytes.len() != len {
            return Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "{} answered {} byte(s) of the {len} asked for at offset {offset}",
                    self.object.url,
                    bytes.len()
                ),
            )));
        }
        Ok(bytes)
    }
}

impl ByteRangeSource for RemoteSource {
    /// **The request is raced against the cancellation rather than polled
    /// beside it.** A network read's wait is bounded by the retry schedule and
    /// not by one `pread`, so a reader that only polled would answer a Ctrl-C
    /// minutes late; losing the race drops the request where it stands
    /// (`docs/design/decisions.md`, "D26").
    fn read_range(
        &self,
        offset: u64,
        len: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Bytes>> + Send + '_>> {
        Box::pin(async move {
            if len == 0 {
                return Ok(Bytes::new());
            }
            let cancel = self.cancel.lock().unwrap().clone();
            match cancel {
                Some(cancel) => match cancel.until_cancelled(self.get(offset, len)).await {
                    Some(read) => read,
                    None => Err(Error::ScanCancelled { scanned_through: offset }),
                },
                None => self.get(offset, len).await,
            }
        })
    }

    /// The origin's probe answered this, so it costs no round trip and cannot
    /// disagree with the size the cache claim was settled against.
    fn size(&self) -> Pin<Box<dyn Future<Output = Result<u64>> + Send + '_>> {
        Box::pin(async move { Ok(self.size) })
    }

    fn modified(&self) -> Pin<Box<dyn Future<Output = Result<Option<SystemTime>>> + Send + '_>> {
        Box::pin(async move { Ok(self.modified) })
    }

    fn hint_cancellation(&self, cancel: Arc<Cancellation>) {
        *self.cancel.lock().unwrap() = Some(cancel);
    }

    /// The URL and the entity tag, which is what a remote cache records
    /// beyond the size and modification time every source answers.
    fn remote_identity(&self) -> Option<RemoteIdentity> {
        // The same string `Origin`'s `Display` prints, which is what a
        // refusal naming an origin has to match; a URL carrying a credential
        // never reaches here, `Origin::remote` having refused it.
        Some(RemoteIdentity::new(self.object.url.to_string(), self.etag.clone()))
    }

    /// **The only thing that stops pinning the object**, which is what
    /// `--strict-identity=none` reaches
    /// (`docs/design/decisions.md`, "D21").
    fn hint_in_flight_identity(&self, binds: bool) {
        self.precondition.store(binds, Ordering::Relaxed);
    }

    /// Whatever the precondition cannot pin, since this source's size and
    /// modification time are the probe's and the watch would compare the
    /// probe with itself.
    fn in_flight_unchecked(&self) -> Option<String> {
        self.object.unpinned()
    }
}

/// Open `origin`, which must name a remote object, as a [`ByteRangeSource`].
///
/// [`open_local`](super::origin::open_local)'s sibling, acting on the same
/// [`Verdict`] over the probe's magic bytes and `known`.
///
/// **A remote `.xz` is read exactly as a local one is**, through
/// [`FetchedXzSource`] over the plain source rather than through [`XzSource`](crate::XzSource),
/// which pulls from a file (`docs/design/decisions.md`, "D14" and "D18").
/// Where the cache holds the seek table there is no walk at all; where
/// it does not, the walk is announced before it is paid for
/// ([`announce_remote_walk`]).
///
/// Deficiency register: `deficiency: KD51` — a run that
/// `pgdt_dump::cache::SourceWatch::open` will refuse for a server nothing can pin
/// ([`ByteRangeSource::in_flight_unchecked`]) pays that walk first, the watch
/// opening only over the source this returns. Refusing here would need the
/// strictness passed to every open for the sake of one pairing — an uncached
/// `.xz` from a server stating no usable validator. **(c) unowned**; promoted
/// by a user who meets it on a dump of many streams.
pub(crate) async fn open_remote(origin: &Origin, known: KnownCompression) -> Result<Recognized> {
    let Location::Remote(object) = &origin.location else {
        return Err(Error::SourceNotReadable {
            origin: origin.to_string(),
            why: "it is not a URL, so there is nothing to fetch".to_string(),
        });
    };
    let probe = origin.probe().await?;
    let fetched =
        || -> Arc<dyn ByteRangeSource> { Arc::new(RemoteSource::new(Arc::clone(object), probe)) };
    match Verdict::of(probe.leading(), known) {
        // The saving, and the reason the cold walk below is worth announcing
        // rather than refusing: a table from a previous walk of this object
        // costs no round trip at all (`docs/design/decisions.md`, "D18").
        Verdict::XzKnown(table) => {
            over_known_table(FetchedXzSource::with_table(fetched(), probe.stored_size(), table))
        }
        Verdict::XzUnknown => {
            let source = fetched();
            announce_remote_walk(origin);
            let (table, fetches) =
                walk_seek_table(&*source, probe.stored_size(), probe.leading()).await?;
            tracing::info!(
                source = %origin,
                streams = table.stream_count(),
                blocks = table.block_count(),
                fetches,
                "seek table build complete",
            );
            Ok(Recognized::Source(Arc::new(FetchedXzSource::with_table(
                source,
                probe.stored_size(),
                table,
            )?)))
        }
        Verdict::Mismatch => Ok(Recognized::Mismatch),
        Verdict::Plain => Ok(Recognized::Source(fetched())),
    }
}

/// Say, **before** it is paid for, what walking a fetched `.xz` file's stream
/// footers is about to cost, and what the two ways out of it are
/// (`docs/design/decisions.md`, "D18").
///
/// **Not refused, announced.** A threshold above which a cold walk was
/// rejected would be a tuned number this phase has said it produces none of,
/// and it would deny a user a command that works; the precedent is exact —
/// `docs/design/decisions.md`, "D19" warns about a one-block `.xz` and refuses
/// nothing, because only the user can judge one decode-from-zero. The same
/// sentence holds with round trips in place of a decode.
///
/// **On the status channel rather than as a `Diagnostic`.** The division is
/// the one `pgdt_model::diagnostic`'s module doc draws: `tracing` carries cost and
/// progress as a run meets them, a `Diagnostic` a structured finding about the
/// file, drained from a result. This line is the first kind — a
/// `pgdt_model::Diagnostic` rides a `DumpIndex` or a `CacheStatus`, both built from
/// a source that already exists, which is *after* the walk, and the whole
/// value of this line is that it arrives before. It is also where the local
/// source's own "seek table build started" goes
/// (`docs/design/decisions.md`, "D64"), so both providers announce a walk
/// alike.
///
/// **The number itself is not owed to that channel.** The stream count is a
/// property of the file, and [`pgdt_dump::cache::CompressionShape`] is the
/// projection that carries it to a caller as data — `pgdt info --detail` and
/// `--json` both print it — beside the source's own [`ByteRangeSource::seek_table`].
/// What this line adds is neither of those, but its timing.
fn announce_remote_walk(origin: &Origin) {
    tracing::warn!(
        source = %origin,
        "seek table build started: a fetched `.xz` file's stream footers are walked one \
         request at a time, which is one round trip per stream — thousands, on a dump \
         compressed in many small streams. It is paid once: keep the cache this run \
         writes, or parse a local copy and read that.",
    );
}
