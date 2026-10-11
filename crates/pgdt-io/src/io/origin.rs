//! Where a dump is, the one probe everything before a source reads from,
//! and recognition: which source a local origin's bytes call for.

use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "http")]
use std::time::Duration;
use std::time::SystemTime;

use pgdt_core::{Error, Result};

use super::local::LocalFileSource;
#[cfg(feature = "http")]
use super::remote::{
    REMOTE_READ_TIMEOUT, RemoteObject, open_remote, remote_name_of, without_credentials,
};
use super::source::ByteRangeSource;
use super::xz::XzSource;

/// The six bytes every `.xz` stream opens with
/// (`docs/design/decisions.md`, "D14").
pub(super) const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// How many leading bytes an [`Origin`] probe brings back: the longest magic
/// recognition compares, which is [`XZ_MAGIC`]'s. A source whose whole
/// content is shorter brings back fewer, which is not an error — it simply
/// matches no magic.
pub(super) const ORIGIN_LEADING_BYTES: usize = XZ_MAGIC.len();

/// Where a dump is, as a caller's `--source` argument resolved it, together
/// with the one cheap probe of it that everything before the source reads
/// from.
///
/// **It exists because two things are settled before a source is built, and
/// both were reading a `&Path`.** A cache recorded against another stored
/// size is refused before the dump is read past its magic
/// (`docs/design/decisions.md`, "D20"), and recognition has to know what the
/// file's first bytes are in order to choose which source to build
/// (`docs/design/decisions.md`, "D14"). Doing either through the filesystem
/// directly makes both of them statements about local files, which is the
/// assumption a source reached over the network does not meet.
///
/// So the origin answers exactly what is needed that early — **stored size,
/// weak identity, and the leading magic bytes** — and nothing else. It never
/// opens a source, never walks an `.xz` file's stream footers, and never
/// reads past the longest magic recognition compares.
///
/// **The probe runs at most once.** Locally that saves nothing worth naming;
/// the reason it is cached is that a source whose probe is a round trip must
/// answer all three from one. [`Origin::probe`] is therefore the only way to
/// an answer, and a caller reads whichever of them it needs.
///
/// A probe that fails is **not** cached: the failure belongs to whoever is in
/// a position to say something about it, which is the open that follows
/// (see [`pgdt_dump::cache::claim`]).
#[derive(Debug)]
pub struct Origin {
    pub(super) location: Location,
    probed: tokio::sync::OnceCell<OriginProbe>,
}

/// The kinds of place a dump can be, and private: what a caller reads is
/// [`OriginProbe`], which is the same three answers whichever kind produced
/// them.
#[derive(Debug)]
pub(super) enum Location {
    LocalFile(PathBuf),
    /// An object a server answers ranged GETs for. It carries the client as
    /// well as the URL, so the probe's round trip and every later read share
    /// one connection pool and one configuration.
    #[cfg(feature = "http")]
    Remote(Arc<RemoteObject>),
}

/// What one probe of an [`Origin`] answered.
///
/// The weak identity is the modification time, which is the local source's —
/// the half of identity a mismatch in only warns about
/// (`docs/design/decisions.md`, "D21"). The stored size is the half that
/// refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginProbe {
    pub(super) stored_size: u64,
    pub(super) modified: Option<SystemTime>,
    pub(super) leading: Vec<u8>,
}

impl OriginProbe {
    /// Bytes as stored, the number `cache::claim` compares against what a
    /// cache recorded — [`ByteRangeSource::stored_size`] without a source.
    pub fn stored_size(&self) -> u64 {
        self.stored_size
    }

    /// The weak identity, `None` where the origin exposes none.
    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    /// The source's first bytes, as far as the longest magic recognition
    /// compares — fewer for a source shorter than that.
    pub fn leading(&self) -> &[u8] {
        &self.leading
    }
}

impl Origin {
    /// The dump at `path` on this filesystem.
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self { location: Location::LocalFile(path.into()), probed: tokio::sync::OnceCell::new() }
    }

    /// The dump an HTTP server holds at `url`, read over ranged GETs.
    ///
    /// **This is where "HTTP only" is enforced rather than merely stated**: the scheme must be
    /// `http` or `https`, and a URL carrying a username or password is refused
    /// by name rather than silently stripped, so nobody believes a credential
    /// was sent. A presigned URL needs none — the signature rides in the
    /// query string, which this preserves.
    ///
    /// The client is built here and reading nothing: the first round trip is
    /// [`Origin::probe`]'s.
    #[cfg(feature = "http")]
    pub fn remote(url: &url::Url) -> Result<Self> {
        Self::remote_with_read_timeout(url, REMOTE_READ_TIMEOUT)
    }

    /// [`Origin::remote`] with the liveness deadline stated rather than taken
    /// from [`REMOTE_READ_TIMEOUT`].
    ///
    /// **A knob the library offers and the CLI does not**: what a person at a
    /// terminal must state is governed, and an embedder on a link this project
    /// has never seen is not that person. The stalled-origin
    /// assertion is its first user rather than its reason.
    #[cfg(feature = "http")]
    pub fn remote_with_read_timeout(url: &url::Url, read_timeout: Duration) -> Result<Self> {
        let refuse = |why: &str| Error::SourceNotReadable {
            origin: without_credentials(url),
            why: why.to_string(),
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(refuse("only `http:` and `https:` URLs are fetched over the network"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refuse(
                "it carries a username or password, and this build sends no credentials — use a \
                 presigned URL, whose signature rides in the query string",
            ));
        }
        if remote_name_of(url).is_none() {
            return Err(refuse(
                "it names no object — a bare host, or a path ending in `/`, is a place rather \
                 than a dump, and there is nothing there to read or to name a cache after",
            ));
        }
        let object = RemoteObject::open(url.clone(), read_timeout)?;
        Ok(Self {
            location: Location::Remote(Arc::new(object)),
            probed: tokio::sync::OnceCell::new(),
        })
    }

    /// What a `--source` argument names: **a URL first and a path second**.
    ///
    /// `http` and `https` select the network; `file:` is a local path, because
    /// a user who has seen `--source https://…` work may reasonably conclude
    /// that a local file now needs `file://`, and being right about how the
    /// tool works should not be a way to get an error. An empty or
    /// `localhost` host is this filesystem; any other host is refused by name,
    /// this build speaking no network file protocol. Every other scheme is
    /// refused by name, which is what a mistyped one deserves.
    ///
    /// **Anything that is not a URL is a path**, exactly as before this phase.
    #[cfg(feature = "http")]
    pub fn resolve(argument: &str) -> Result<Self> {
        let refuse = |why: String| Error::SourceNotReadable { origin: argument.to_string(), why };
        let Ok(url) = url::Url::parse(argument) else {
            return Ok(Self::local(argument));
        };
        match url.scheme() {
            "http" | "https" => Self::remote(&url),
            // `to_file_path` refuses two things, and they are different
            // mistakes: a host naming another machine, and a path that is not
            // absolute.
            "file" => url.to_file_path().map(Self::local).map_err(|()| {
                refuse(match url.host_str() {
                    Some(host) if host != "localhost" => format!(
                        "a `file:` URL names a path on this machine, so its host must be empty \
                         or `localhost`, not `{host}`"
                    ),
                    _ => "a `file:` URL names an absolute path, so its path must begin with `/`"
                        .to_string(),
                })
            }),
            scheme => Err(refuse(format!(
                "the `{scheme}:` scheme is not one this build reads — `http:`, `https:` and \
                 `file:` are, and a path whose first segment holds a colon is \
                 written `./{argument}`"
            ))),
        }
    }

    /// The path this origin names on this filesystem, or `None` for one that
    /// is not a file — what a caller deriving a name *beside* the dump needs,
    /// there being no such place for an object fetched over the network.
    pub fn local_path(&self) -> Option<&Path> {
        match &self.location {
            Location::LocalFile(path) => Some(path),
            #[cfg(feature = "http")]
            Location::Remote(_) => None,
        }
    }

    /// What the object this origin names is called, for a caller deriving a
    /// file name from it: a URL's **last path segment**, exactly as written
    /// there — the working-directory cache name a remote dump gets, there
    /// being no place beside it for one to sit
    /// (`docs/design/decisions.md`, "D87").
    ///
    /// `None` for a local file, whose caller has [`Origin::local_path`] and
    /// the colocated rule. A remote origin always has one: a URL that names
    /// no object is refused when the origin is built.
    pub fn remote_name(&self) -> Option<&str> {
        match &self.location {
            Location::LocalFile(_) => None,
            #[cfg(feature = "http")]
            Location::Remote(object) => remote_name_of(&object.url),
        }
    }

    /// Probe this origin, or hand back the answer a previous call got.
    pub async fn probe(&self) -> Result<&OriginProbe> {
        self.probed
            .get_or_try_init(|| async {
                match &self.location {
                    Location::LocalFile(path) => probe_local_file(path).await,
                    #[cfg(feature = "http")]
                    Location::Remote(object) => object.probe().await,
                }
            })
            .await
    }
}

impl std::fmt::Display for Origin {
    /// How a message to the user names this source.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.location {
            Location::LocalFile(path) => write!(f, "{}", path.display()),
            #[cfg(feature = "http")]
            Location::Remote(object) => write!(f, "{}", object.url),
        }
    }
}

/// One `stat` and one short read, on a blocking thread as a local source's
/// reads are; opening an `.xz` source, its footer walk included, is not.
async fn probe_local_file(path: &Path) -> Result<OriginProbe> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<OriginProbe> {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)?;
        let metadata = file.metadata()?;
        let mut buf = [0u8; ORIGIN_LEADING_BYTES];
        let mut filled = 0;
        while filled < buf.len() {
            match file.read(&mut buf[filled..]) {
                Ok(0) => break,
                Ok(n) => filled += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::Io(e)),
            }
        }
        Ok(OriginProbe {
            stored_size: metadata.len(),
            modified: Some(metadata.modified()?),
            leading: buf[..filled].to_vec(),
        })
    })
    .await
    .map_err(Error::join)?
}

/// What a caller already knows about a file's compression layer before
/// [`open_local`] has looked at it — normally read out of a cache written from
/// that same file (`pgdt_dump::cache::claim`), and the reason an `.xz` source need
/// not re-walk its stream footers (`docs/design/decisions.md`, "D18").
///
/// **A bare [`xz_seek::SeekTable`], not a cache.** What loads it is the
/// caller's business, which keeps `io.rs` naming nothing in `pgdt_dump::cache`.
///
/// Three states, not an `Option`: "the cache says this file is plain" is a
/// claim recognition can *contradict* (`docs/design/decisions.md`, "D18").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KnownCompression {
    /// Nothing is known — recognition reads the file's own bytes and, for
    /// `.xz`, pays the walk. The only state that can never produce a
    /// [`Recognized::Mismatch`].
    #[default]
    Unknown,
    /// Known to have no compression layer at all.
    Plain,
    /// Known to be `.xz`, with the seek table a previous walk of this file
    /// produced.
    Xz(xz_seek::SeekTable),
}

/// What [`open_local`] made of a file, given what its caller claimed to know.
pub enum Recognized {
    /// The source, ready to read.
    Source(Arc<dyn ByteRangeSource>),
    /// The [`KnownCompression`] handed in does not describe this file: its
    /// compression layer disagrees with the file's own first bytes, or its
    /// seek table was refused by `xz_seek`'s validation. Nothing beyond the
    /// magic was read and no source was built, so the caller still has both
    /// choices — pay the walk with [`KnownCompression::Unknown`], or report
    /// the cache it came from as unusable
    /// (`docs/design/decisions.md`, "D18").
    ///
    /// The claim it contradicts and the cache's span index were written by
    /// one `save` from one file, so this condemns that index too: a caller
    /// that goes on to scan must not also resume from it.
    Mismatch,
}

/// Open whatever `origin` names, choosing the source kind by where the dump
/// is and its compression layer by what its first bytes are.
///
/// **The entry point above [`open_local`] and [`open_remote`]**, and the one a
/// caller with an origin in hand wants: the two below it each answer for one
/// kind of place and refuse the other, so dispatching here is what keeps a
/// caller from having to know which it has. Every argument and every answer is
/// the same whichever it dispatches to.
pub async fn open(origin: &Origin, known: KnownCompression) -> Result<Recognized> {
    match &origin.location {
        Location::LocalFile(_) => open_local(origin, known).await,
        #[cfg(feature = "http")]
        Location::Remote(_) => open_remote(origin, known).await,
    }
}

/// Open `origin` as a [`ByteRangeSource`], choosing between
/// [`LocalFileSource`] and [`XzSource`] by **content**, not by name
/// (`docs/design/decisions.md`, "D14"): the first six bytes are checked
/// against `.xz`'s magic, whatever the file is called. A file that really is
/// `.xz`-compressed is recognised however it is named or extensionless; a file
/// merely *named* `.xz` whose bytes don't match opens as plain.
///
/// **The magic comes from [`Origin::probe`], not from a read of its own.**
/// Recognition is told what the source holds, which is what keeps it from
/// being a statement about files; by the time a caller gets here the probe
/// has normally already run, for the cache claim.
///
/// `known` is what a caller read out of a cache for this same file, and it is
/// checked rather than believed: recognition still compares the magic, and a
/// claim the file contradicts is [`Recognized::Mismatch`] rather than a silent
/// fallback. [`KnownCompression::Unknown`] is the no-knowledge case and always
/// yields a source.
///
/// **An uncached `.xz` is opened on the calling task**, its footer walk
/// included: only a first open reaches the walk, before anything else is
/// scheduled, and it says `seek table build started` and `… complete` around
/// itself (`crates/pgdt/tests/status_output.rs`), so the wait does not read as a
/// hang. An embedder opening one inside a busy runtime would want it on a
/// blocking thread; the DataFusion provider never walks, reading the table
/// its complete cache holds.
pub async fn open_local(origin: &Origin, known: KnownCompression) -> Result<Recognized> {
    // The kind is settled before the probe, so an origin this cannot open is
    // refused without a round trip being spent on it. The match has a second
    // arm only where the remote source is compiled in; without it there is one
    // kind of place and the refusal is unreachable.
    #[allow(clippy::infallible_destructuring_match)]
    let path = match &origin.location {
        Location::LocalFile(path) => path,
        #[cfg(feature = "http")]
        Location::Remote(_) => {
            return Err(Error::SourceNotReadable {
                origin: origin.to_string(),
                why: "it is not a file on this filesystem".to_string(),
            });
        }
    };
    let is_xz = origin.probe().await?.leading().starts_with(&XZ_MAGIC);
    match (is_xz, known) {
        // The saving: a table from a previous walk of this file, validated
        // against the file's length and its own internal consistency without
        // reading a byte, and never walked for.
        (true, KnownCompression::Xz(table)) => match XzSource::with_table(path, table) {
            Ok(source) => Ok(Recognized::Source(Arc::new(source))),
            Err(error) if matches!(error.xz_cause(), Some(xz_seek::Error::InvalidTable { .. })) => {
                Ok(Recognized::Mismatch)
            }
            Err(e) => Err(e),
        },
        (true, KnownCompression::Unknown) => {
            Ok(Recognized::Source(Arc::new(XzSource::open(path)?)))
        }
        // Both directions of "the cache describes a different file": a
        // compression index for a file whose bytes are plain, and a plain
        // cache for a file whose bytes are `.xz`.
        (true, KnownCompression::Plain) | (false, KnownCompression::Xz(_)) => {
            Ok(Recognized::Mismatch)
        }
        (false, KnownCompression::Unknown | KnownCompression::Plain) => {
            Ok(Recognized::Source(Arc::new(LocalFileSource::open(path)?)))
        }
    }
}
